//! ツール結果の即時圧縮の配線を機械で留める（Spec 59 P1 / `tool_prune_contract`）。
//!
//! **偽の採点器で経路を丸ごと試す** — コアは [`ParagraphScorer`] しか知らないので、
//! HTTP も Jev も無しで「掛かる場所・RepeatGuard の引数・`omitted` の寿命・
//! 既定 OFF でのバイト等価」を確かめられる。
//!
//! **ここで最も重いのは `omitted` の寿命**（Spec 59 rev1 の骨格の誤り）。
//! 圧縮は Round 1、`omitted` の呼び出しは Round 2 なので、生本文を `CallRunner` の
//! フィールドに置くと必ず `not_found` になる。**2 周またぐテストでしか出ない。**

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use fuseforks_core::llm::{
    ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, Role, ToolCall, Usage,
};
use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::prune::{ParagraphScorer, ScoreError, ScoreReport};
use fuseforks_core::tool::{AgentTool, ToolContext};
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};

/// 段落数。`MIN_CHARS`(4,000) を超えるように 1 段落 400 字 × 12。
const PARAS: usize = 12;
const PARA_CHARS: usize = 400;

struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-prune-{tag}-{}",
            std::process::id()
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

/// 圧縮の対象になるツール（`prunable` が真）。毎回同じ長い本文を返す。
#[derive(Default)]
struct BigTool {
    runs: AtomicUsize,
}

fn big_body() -> String {
    (0..PARAS)
        .map(|i| format!("段落 {i}。{}", "あ".repeat(PARA_CHARS)))
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[async_trait::async_trait]
impl AgentTool for BigTool {
    fn name(&self) -> &str {
        "big_probe"
    }
    fn description(&self, _l: fuseforks_core::world::Language) -> String {
        "テスト用。長い本文を返す".into()
    }
    fn parameters(&self, _l: fuseforks_core::world::Language) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    fn prunable(&self) -> bool {
        true
    }
    async fn call(
        &self,
        _ctx: &ToolContext,
        _args: &serde_json::Value,
    ) -> fuseforks_core::CoreResult<String> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        Ok(big_body())
    }
}

/// 対象**外**のツール（`prunable` は既定の偽）。同じ長さの本文を返す。
#[derive(Default)]
struct PlainTool;

#[async_trait::async_trait]
impl AgentTool for PlainTool {
    fn name(&self) -> &str {
        "plain_probe"
    }
    fn description(&self, _l: fuseforks_core::world::Language) -> String {
        "テスト用。圧縮の対象外".into()
    }
    fn parameters(&self, _l: fuseforks_core::world::Language) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    async fn call(
        &self,
        _ctx: &ToolContext,
        _args: &serde_json::Value,
    ) -> fuseforks_core::CoreResult<String> {
        Ok(big_body())
    }
}

/// 偽の採点器。**先頭の 1 段落だけ残す。**
struct KeepFirst {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl ParagraphScorer for KeepFirst {
    async fn score(&self, _basis: &str, paragraphs: &[&str]) -> Result<ScoreReport, ScoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ScoreReport {
            scores: (0..paragraphs.len())
                .map(|i| Some(if i == 0 { 0.9 } else { 0.01 }))
                .collect(),
            calls: 1,
            tokens: 123,
        })
    }
    fn threshold(&self) -> f32 {
        0.2
    }
}

/// 常に失敗する採点器（fail-open の確認）。
struct AlwaysFails;

#[async_trait::async_trait]
impl ParagraphScorer for AlwaysFails {
    async fn score(&self, _basis: &str, _paragraphs: &[&str]) -> Result<ScoreReport, ScoreError> {
        Err(ScoreError::Failed("probe".into()))
    }
    fn threshold(&self) -> f32 {
        0.2
    }
}

/// 1 周目で `tool` を呼び、2 周目で `omitted` を呼び、3 周目で答える。
///
/// **2 周またぐのが要点** — 生本文の寿命がターンでないと 2 周目が `not_found`。
struct CallThenOmit {
    tool: &'static str,
    /// `omitted` を呼ぶか（対象外ツールのテストでは呼ばない）。
    omit: bool,
}

#[async_trait::async_trait]
impl LlmBackend for CallThenOmit {
    fn name(&self) -> &str {
        "call-then-omit"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let usage = Usage {
            prompt: 10,
            completion: 5,
            cache_read: 0,
            cache_write: 0,
            cache_write_1h: 0,
            reasoning: 0,
        };
        let results = req.messages.iter().filter(|m| m.role == Role::Tool).count();
        let call = |id: &str, name: &str, args: serde_json::Value| ChatResponse {
            text: Some(String::new()),
            tool_calls: vec![ToolCall {
                id: id.into(),
                name: name.into(),
                args,
                extra: None,
            }],
            finish: Finish::ToolUse,
            usage,
            grounding: Default::default(),
            reasoning_summary: Vec::new(),
        };
        match results {
            0 => Ok(call("c1", self.tool, serde_json::json!({}))),
            1 if self.omit => Ok(call(
                "c2",
                "omitted",
                serde_json::json!({ "id": "P1", "from": 1, "to": 11 }),
            )),
            _ => Ok(ChatResponse {
                text: Some("終わりました".into()),
                tool_calls: Vec::new(),
                finish: Finish::Stop,
                usage,
                grounding: Default::default(),
                reasoning_summary: Vec::new(),
            }),
        }
    }
}

async fn village(
    dir: &TempDir,
    backend: Arc<dyn LlmBackend>,
) -> (Orchestrator, AgentId) {
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(backend)),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig {
            schedule_interval: Duration::from_secs(3600),
            ..OrchestratorConfig::default()
        },
    )
    .await
    .expect("bootstrap できること");
    // ホストの OS ロケールに依存させない（#101）。
    orchestrator
        .set_language(fuseforks_core::world::Language::Ja)
        .await
        .unwrap();
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();
    let id = AgentId::from("agent_1");
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "調べ役", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();
    (orchestrator, id)
}

/// 最後にモデルが受け取ったツール結果（= 圧縮後の本文）を取り出す。
fn last_tool_result(messages: &[fuseforks_core::llm::ChatMessage]) -> String {
    messages
        .iter()
        .rev()
        .find(|m| m.role == Role::Tool)
        .map(|m| m.content.clone())
        .unwrap_or_default()
}

/// 送ったプロンプトを覗くためのバックエンド（最後のツール結果を保存する）。
struct Spy {
    inner: CallThenOmit,
    seen: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl LlmBackend for Spy {
    fn name(&self) -> &str {
        "spy"
    }
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let body = last_tool_result(&req.messages);
        if !body.is_empty() {
            self.seen.lock().unwrap().push(body);
        }
        self.inner.chat(req).await
    }
}

/// 圧縮が掛かり、`omitted` が**次の周**で逐語を返す。
#[tokio::test]
async fn a_prunable_tool_is_compressed_and_omitted_reads_it_back_next_round() {
    let dir = TempDir::new("ok");
    let spy = Arc::new(Spy {
        inner: CallThenOmit { tool: "big_probe", omit: true },
        seen: std::sync::Mutex::new(Vec::new()),
    });
    let (orchestrator, id) = village(&dir, spy.clone()).await;
    let tool = Arc::new(BigTool::default());
    orchestrator.register_tool(tool.clone()).await;
    let scorer = Arc::new(KeepFirst { calls: AtomicUsize::new(0) });
    orchestrator.set_paragraph_scorer(Some(scorer.clone())).await;

    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&id, "この依頼は 20 字以上あるので基準として通ります")
        .await
        .unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    assert_eq!(scorer.calls.load(Ordering::SeqCst), 1, "採点は 1 回");
    let seen = spy.seen.lock().unwrap().clone();
    assert!(seen.len() >= 2, "ツール結果が 2 回モデルへ渡ること: {}", seen.len());

    // 1 通目 = 圧縮された本文。
    let pruned = &seen[0];
    assert!(pruned.contains("関連度で段落を省略"), "印が入る: {pruned}");
    assert!(pruned.contains("id=P1"));
    assert!(pruned.contains("段落 0。"), "残した段落は逐語");
    assert!(!pruned.contains("段落 5。"), "落とした段落は消える");
    assert!(
        pruned.chars().count() < big_body().chars().count(),
        "圧縮後のほうが短いこと"
    );

    // 2 通目 = `omitted` が返した逐語。**次の周で読めることが要点。**
    let back = &seen[1];
    assert!(back.contains("段落 5。"), "落とした段落が逐語で返る: {back}");
    assert!(back.contains("［段落 1］"), "段落番号が付く");
    assert!(!back.contains("段落 0。"), "保持した段落は返さない（生ログの再取得にしない）");
}

/// **対象外のツールは 1 バイトも変わらない**（`prunable` の既定は偽）。
#[tokio::test]
async fn a_tool_that_does_not_opt_in_is_untouched() {
    let dir = TempDir::new("plain");
    let spy = Arc::new(Spy {
        inner: CallThenOmit { tool: "plain_probe", omit: false },
        seen: std::sync::Mutex::new(Vec::new()),
    });
    let (orchestrator, id) = village(&dir, spy.clone()).await;
    orchestrator.register_tool(Arc::new(PlainTool)).await;
    let scorer = Arc::new(KeepFirst { calls: AtomicUsize::new(0) });
    orchestrator.set_paragraph_scorer(Some(scorer.clone())).await;

    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&id, "この依頼は 20 字以上あるので基準として通ります")
        .await
        .unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    assert_eq!(scorer.calls.load(Ordering::SeqCst), 0, "採点器を 1 度も呼ばないこと");
    let seen = spy.seen.lock().unwrap().clone();
    assert_eq!(seen[0], big_body(), "本文がバイト等価であること");
}

/// **採点器が無ければ機構ごと存在しない**（既定 OFF。Goal 4）。
#[tokio::test]
async fn without_a_scorer_nothing_changes() {
    let dir = TempDir::new("off");
    let spy = Arc::new(Spy {
        inner: CallThenOmit { tool: "big_probe", omit: false },
        seen: std::sync::Mutex::new(Vec::new()),
    });
    let (orchestrator, id) = village(&dir, spy.clone()).await;
    orchestrator.register_tool(Arc::new(BigTool::default())).await;
    // 採点器を差し込まない。

    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&id, "この依頼は 20 字以上あるので基準として通ります")
        .await
        .unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let seen = spy.seen.lock().unwrap().clone();
    assert_eq!(seen[0], big_body(), "既定 OFF ではバイト等価であること");
}

/// **採点が失敗したら全文を通す**（fail-open。D8）。
#[tokio::test]
async fn a_failing_scorer_falls_back_to_the_full_text() {
    let dir = TempDir::new("fail");
    let spy = Arc::new(Spy {
        inner: CallThenOmit { tool: "big_probe", omit: false },
        seen: std::sync::Mutex::new(Vec::new()),
    });
    let (orchestrator, id) = village(&dir, spy.clone()).await;
    orchestrator.register_tool(Arc::new(BigTool::default())).await;
    orchestrator.set_paragraph_scorer(Some(Arc::new(AlwaysFails))).await;

    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&id, "この依頼は 20 字以上あるので基準として通ります")
        .await
        .unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let seen = spy.seen.lock().unwrap().clone();
    assert_eq!(seen[0], big_body(), "失敗したら全文がそのまま渡ること");
}

/// **基準が 20 字未満なら圧縮しない**（`no_basis`。D5）。
#[tokio::test]
async fn a_short_request_skips_pruning() {
    let dir = TempDir::new("basis");
    let spy = Arc::new(Spy {
        inner: CallThenOmit { tool: "big_probe", omit: false },
        seen: std::sync::Mutex::new(Vec::new()),
    });
    let (orchestrator, id) = village(&dir, spy.clone()).await;
    orchestrator.register_tool(Arc::new(BigTool::default())).await;
    let scorer = Arc::new(KeepFirst { calls: AtomicUsize::new(0) });
    orchestrator.set_paragraph_scorer(Some(scorer.clone())).await;

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "了解").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    assert_eq!(scorer.calls.load(Ordering::SeqCst), 0, "基準が短いので採点しない");
    let seen = spy.seen.lock().unwrap().clone();
    assert_eq!(seen[0], big_body(), "全文がそのまま渡ること");
}

/// イベントが止むまで集める（`tests/orchestrator.rs` と同じ作法）。
///
/// 窓は**この系で最も短い定期イベントの周期より短く**する（`failures.md` #86 —
/// 統計は 1 秒周期なので、1 秒以上にすると永久に閉じない）。
/// 決して返らない採点器。**打ち切りが待ちを切ること**を確かめるための土台。
struct NeverReturns {
    /// 採点に入った合図（ここで打ち切りを撃つ）。
    started: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl ParagraphScorer for NeverReturns {
    async fn score(&self, _basis: &str, _paragraphs: &[&str]) -> Result<ScoreReport, ScoreError> {
        self.started.notify_one();
        std::future::pending().await
    }
    fn threshold(&self) -> f32 {
        0.2
    }
}

/// **打ち切りは締め切り（20 秒）を待たずに採点を切る**（D8）。
///
/// 締め切りと打ち切りの網は `prune::with_deadline` に 1 実装あり、単体で
/// 留めてある。ここで見るのは**配線** — `turn.token` を渡していなければ
/// この走行は 20 秒掛かる（単体テストでは原理的に出ない形）。
#[tokio::test]
async fn an_interrupted_turn_stops_waiting_for_the_scorer() {
    let dir = TempDir::new("cancel");
    let spy = Arc::new(Spy {
        inner: CallThenOmit { tool: "big_probe", omit: false },
        seen: std::sync::Mutex::new(Vec::new()),
    });
    let (orchestrator, id) = village(&dir, spy.clone()).await;
    orchestrator.register_tool(Arc::new(BigTool::default())).await;
    let started = Arc::new(tokio::sync::Notify::new());
    orchestrator
        .set_paragraph_scorer(Some(Arc::new(NeverReturns { started: started.clone() })))
        .await;

    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&id, "この依頼は 20 字以上あるので基準として通ります")
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .expect("採点まで到達すること");
    orchestrator.interrupt_turn(&id).await;

    // **ターンが終わったことを実際に観測する。** 「静かになるまで待つ」では
    // 判定にならない — 採点で固まったままでもイベントは流れないので静かになる
    // （`failures.md` #132 と同じ「動いていないのに緑」の形。1 度踏んだ）。
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match rx.recv().await {
                Ok(fuseforks_core::event::CoreEvent::AgentTyping { agent_id, active: false })
                    if agent_id == id =>
                {
                    return;
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => return,
            }
        }
    })
    .await;
    assert!(
        ended.is_ok(),
        "打ち切りでターンが終わること（締め切り {:?} を待っていない）",
        fuseforks_core::prune::DEADLINE
    );
}
async fn drain_until_quiet(
    rx: &mut tokio::sync::broadcast::Receiver<fuseforks_core::event::CoreEvent>,
    quiet: Duration,
) {
    loop {
        match tokio::time::timeout(quiet, rx.recv()).await {
            Ok(Ok(_)) => {}
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
            _ => break,
        }
    }
}
