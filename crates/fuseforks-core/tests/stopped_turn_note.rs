//! 止まったターンの注記は「依頼は未処理」と書かず、村の言語で書く（2026-11-11）。
//!
//! # 何が起きていたか
//!
//! 打ち切り（Spec 10）・予算切れ（Spec 11）・失敗の 3 つの出口は、依頼と注記の往復を履歴へ積む。
//! その注記が「依頼は未処理のまま残っています」と書いていた。ところが止めるのは**周の境目**なので、
//! それまでの周のツール（ファイルの書き込み・黒板の移動・コマンド）は実行済みで、次のターンのモデルが
//! 同じ書き込みや追記をやり直す余地があった。Spec 72 の承認待ちの注記だけは「未処理と書かない」と
//! 明記して直してあり、兄弟の 3 つに伝わっていなかった。
//!
//! さらに打ち切りと予算切れの注記は**日本語だけ**で、英語と简体中文の村でも日本語の文がモデルの
//! 履歴に入っていた（失敗の注記は 2026-10-19 に村の言語へ直してある — `failed_turn_note_language.rs`）。
//!
//! 見つけた契機は openai/codex の読み比べ（`core/src/context/turn_aborted.rs` — 打ち切った後に
//! 「途中まで実行されたかもしれない」と伝えている）。
//!
//! # 何を確かめるか
//!
//! 止まったターンの**次のターン**のリクエストに載った注記（履歴の応答側）を見る。1 周目でツールを
//! 1 本実行してから止めるので、「ツールは実行済み」が実際に成り立つ形で確かめている。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fuseforks_core::event::CoreEvent;
use fuseforks_core::llm::{
    ChatMessage, ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, Role, ToolCall, Usage,
};
use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::world::Language;
use fuseforks_core::{
    AgentTool, ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator,
    OrchestratorConfig, ToolContext,
};
use tokio::sync::broadcast::Receiver;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-stopped-note-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 即答ツール。1 周目で実際に 1 本実行させるための燃料。
struct BusyTool;

#[async_trait::async_trait]
impl AgentTool for BusyTool {
    fn name(&self) -> &str {
        "busy_probe"
    }
    fn description(&self, _language: Language) -> String {
        "test tool".into()
    }
    fn parameters(&self, _language: Language) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    async fn call(&self, _ctx: &ToolContext, _args: &serde_json::Value) -> fuseforks_core::CoreResult<String> {
        Ok("ok".into())
    }
}

/// ツールを呼び続け、`plain` が立ったら本文だけ返すバックエンド。`gate` があれば、テストが
/// 許可するまで応答を返さない（打ち切りがどの周の前に届いたかを壁時計に依存させない）。
struct LoopingBackend {
    calls: AtomicU32,
    started: tokio::sync::mpsc::UnboundedSender<u32>,
    gate: Option<Arc<tokio::sync::Semaphore>>,
    plain: AtomicBool,
    last_request: Mutex<Vec<ChatMessage>>,
}

fn usage() -> Usage {
    Usage { prompt: 1, completion: 1, cache_read: 0, cache_write: 0, cache_write_1h: 0, reasoning: 0 }
}

#[async_trait::async_trait]
impl LlmBackend for LoopingBackend {
    fn name(&self) -> &str {
        "looping"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        *self.last_request.lock().unwrap() = req.messages.clone();
        let _ = self.started.send(n);
        if let Some(gate) = &self.gate {
            gate.acquire().await.expect("gate は閉じない").forget();
        }
        if self.plain.load(Ordering::SeqCst) {
            return Ok(ChatResponse {
                text: Some("done".into()),
                tool_calls: Vec::new(),
                finish: Finish::Stop,
                usage: usage(),
                grounding: Default::default(),
                reasoning_summary: Vec::new(),
                replay: None,
            });
        }
        // 毎回違う引数で呼ぶ（RepeatGuard に先に止めさせない）。
        Ok(ChatResponse {
            text: Some(String::new()),
            tool_calls: vec![ToolCall {
                id: format!("call_{n}"),
                name: "busy_probe".into(),
                args: serde_json::json!({ "round": n }),
                extra: None,
            }],
            finish: Finish::ToolUse,
            usage: usage(),
            grounding: Default::default(),
            reasoning_summary: Vec::new(),
            replay: None,
        })
    }
}

/// 静穏窓は統計の周期（1 秒）より**短く**取る（#86）。
async fn drain_until_quiet(rx: &mut Receiver<CoreEvent>, quiet: Duration) {
    while let Ok(Ok(_)) = tokio::time::timeout(quiet, rx.recv()).await {}
}

struct Village {
    _dir: TempDir,
    orchestrator: Orchestrator,
    backend: Arc<LoopingBackend>,
    started: tokio::sync::mpsc::UnboundedReceiver<u32>,
    gate: Arc<tokio::sync::Semaphore>,
    id: AgentId,
}

/// 1 体の村を組む。`budget` があれば bootstrap の前に `world.json` へ天井を書く（起動時の天井は
/// world の読み込みで入る）。`gated` なら応答はテストが許可するまで返らない。
async fn village(tag: &str, language: Language, budget: Option<u64>, gated: bool) -> Village {
    let dir = TempDir::new(tag);
    if let Some(ceiling) = budget {
        std::fs::write(dir.0.join("world.json"), format!(r#"{{ "tokenBudget": {ceiling} }}"#)).unwrap();
    }
    let (tx, started) = tokio::sync::mpsc::unbounded_channel();
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let backend = Arc::new(LoopingBackend {
        calls: AtomicU32::new(0),
        started: tx,
        gate: gated.then(|| Arc::clone(&gate)),
        plain: AtomicBool::new(false),
        last_request: Mutex::new(Vec::new()),
    });
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(backend.clone())),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig {
            schedule_interval: Duration::from_secs(3600),
            ..OrchestratorConfig::default()
        },
    )
    .await
    .expect("bootstrap できること");
    orchestrator.set_language(language).await.unwrap();
    orchestrator.register_tool(Arc::new(BusyTool)).await;
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "default", "mock-model"))
        .await
        .unwrap();
    let id = AgentId::from("agent_01");
    orchestrator.create_agent(AgentSpec::new(id.clone(), "Bob", "tpl")).await.unwrap();
    orchestrator.start_agent(&id).await.unwrap();
    Village { _dir: dir, orchestrator, backend, started, gate, id }
}

/// 止まったターンの次のターンを送り、そのリクエストの履歴にある注記（応答側）を返す。
async fn note_seen_by_the_next_turn(v: &Village, rx: &mut Receiver<CoreEvent>) -> String {
    v.backend.plain.store(true, Ordering::SeqCst);
    v.gate.add_permits(10);
    v.orchestrator.send_user_message(&v.id, "please continue").await.unwrap();
    drain_until_quiet(rx, Duration::from_millis(400)).await;
    let request = v.backend.last_request.lock().unwrap().clone();
    let notes: Vec<String> = request
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .map(|m| m.content.clone())
        .collect();
    assert_eq!(notes.len(), 1, "履歴の応答側は止まったターンの注記 1 本だけ: {request:#?}");
    notes.into_iter().next().unwrap()
}

/// 打ち切りで止めたターンの注記。1 周目（ツール 1 本）を完走させてから 2 周目の前で切る。
async fn interrupted_note(tag: &str, language: Language) -> String {
    let mut v = village(tag, language, None, true).await;
    let mut rx = v.orchestrator.subscribe();
    v.orchestrator.send_user_message(&v.id, "keep researching").await.unwrap();
    v.started.recv().await.expect("1 周目が始まること");
    v.orchestrator.interrupt_turn(&v.id).await;
    // 飛行中の呼び出しは完走させる（ツール 1 本が実行される）。
    v.gate.add_permits(1);
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    assert_eq!(v.backend.calls.load(Ordering::SeqCst), 1, "打ち切り後は次の周を呼ばない");
    note_seen_by_the_next_turn(&v, &mut rx).await
}

/// 予算切れで止めたターンの注記。1 周 = 実効 5（prompt 1 ×1 + completion 1 ×4）なので、
/// 天井 5 は 1 周目（ツール 1 本）で使い切り、2 周目の前で止まる。
async fn budget_note(tag: &str, language: Language) -> String {
    let v = village(tag, language, Some(5), false).await;
    let mut rx = v.orchestrator.subscribe();
    v.orchestrator.send_user_message(&v.id, "keep researching").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    assert_eq!(v.backend.calls.load(Ordering::SeqCst), 1, "予算切れの後は次の周を呼ばない");
    note_seen_by_the_next_turn(&v, &mut rx).await
}

/// ひらがな・カタカナ・CJK 統合漢字・全角の括弧と句読点のどれかを含むか。
fn has_japanese(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(c, '\u{3040}'..='\u{30ff}' | '\u{4e00}'..='\u{9fff}' | '\u{3000}'..='\u{303f}' | '\u{ff00}'..='\u{ffef}')
    })
}

#[tokio::test]
async fn an_interrupted_turn_does_not_claim_nothing_ran_in_japanese() {
    let note = interrupted_note("int-ja", Language::Ja).await;
    assert!(note.contains("ユーザーの指示で打ち切られました"), "打ち切りの注記: {note}");
    assert!(!note.contains("未処理"), "ツールは実行済みなので「未処理」と書かない: {note}");
    assert!(note.contains("その結果は取り消されていません"), "実行済みの結果が残ることを書く: {note}");
}

#[tokio::test]
async fn an_english_village_gets_the_interrupt_note_in_english() {
    let note = interrupted_note("int-en", Language::En).await;
    assert!(!has_japanese(&note), "英語の村の注記に日本語を入れない: {note}");
    assert!(note.contains("stopped by the user"), "打ち切りの注記: {note}");
    assert!(!note.contains("unprocessed"), "「未処理」と書かない: {note}");
    assert!(note.contains("not undone"), "実行済みの結果が残ることを書く: {note}");
}

#[tokio::test]
async fn a_budget_stopped_turn_does_not_claim_nothing_ran_in_japanese() {
    let note = budget_note("bud-ja", Language::Ja).await;
    assert!(note.contains("トークン予算の上限で打ち切られました"), "予算切れの注記: {note}");
    assert!(!note.contains("未処理"), "ツールは実行済みなので「未処理」と書かない: {note}");
    assert!(note.contains("その結果は取り消されていません"), "実行済みの結果が残ることを書く: {note}");
}

#[tokio::test]
async fn an_english_village_gets_the_budget_note_in_english() {
    let note = budget_note("bud-en", Language::En).await;
    assert!(!has_japanese(&note), "英語の村の注記に日本語を入れない: {note}");
    assert!(note.contains("token budget"), "予算切れの注記: {note}");
    assert!(!note.contains("unprocessed"), "「未処理」と書かない: {note}");
    assert!(note.contains("not undone"), "実行済みの結果が残ることを書く: {note}");
}
