//! **依頼主が読む結果の文は村の言語で書く**（2026-11-12。`orchestrator/reply_text.rs`）。
//!
//! それまでは `ask` / `plan` のツール結果と、依頼主へ返す返信（`Reply`）の本文が日本語に固定されて
//! おり、英語の村でも進行役のモデルは日本語の結果を読んでいた（`failures.md` #156 で「同じ網の外」と
//! 書いて残していたもの）。文そのものは `reply_text` の単体テストが言語ごとに留める。ここで留めるのは
//! **使う場所が村の言語を読んでいること**で、経路ごとに 1 本:
//!
//! 1. 停止中の相手への `ask`（`deliver_and_wait` の配送の失敗）
//! 2. 本文を返さなかったワーカーの返事（コアが代わりに書く文 → `Reply` で依頼主へ）
//! 3. 接続先でない宛先への `plan`（静的な不正）
//! 4. 束ねの見出し（`## agent_id（表示名）` の枠ごと言語で分ける）
//!
//! 各経路を英語と日本語の 2 回走らせる — 英語の側だけだと「常に英語で書く実装」と区別が付かない。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fuseforks_core::llm::{ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, ToolCall, Usage};
use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::world::Language;
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "fuseforks-replytext-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
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

fn usage() -> Usage {
    Usage {
        prompt: 1,
        completion: 0,
        cache_read: 0,
        cache_write: 0,
        cache_write_1h: 0,
        reasoning: 0,
    }
}

fn response(text: Option<&str>, tool_calls: Vec<ToolCall>) -> ChatResponse {
    let finish = if tool_calls.is_empty() { Finish::Stop } else { Finish::ToolUse };
    ChatResponse {
        text: text.map(str::to_owned),
        tool_calls,
        finish,
        usage: usage(),
        grounding: Default::default(),
        reasoning_summary: Vec::new(),
        replay: None,
    }
}

/// 進行役が 1 周目に呼ぶもの。
#[derive(Clone)]
enum Script {
    /// `ask_agent_02` を 1 回。
    Ask,
    /// `plan` を 1 回（宛先の列）。
    Plan(Vec<&'static str>),
}

/// 進行役（`ask_` か `plan` を提示された周）は 1 周目に台本どおり呼び、2 周目でツール結果を
/// 控えて終える。ワーカー（どちらも提示されない周）は `silent` なら本文を返さず、それ以外は答える。
struct Village {
    script: Script,
    silent: bool,
    results: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl LlmBackend for Village {
    fn name(&self) -> &str {
        "reply-text-village"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let coordinator = req
            .tools
            .iter()
            .any(|t| t.name.starts_with("ask_") || t.name == "plan");
        if !coordinator {
            return Ok(response(if self.silent { None } else { Some("ok") }, Vec::new()));
        }
        let results: Vec<String> = req
            .messages
            .iter()
            .filter(|m| m.tool_call_id.is_some())
            .map(|m| m.content.clone())
            .collect();
        if !results.is_empty() {
            self.results.lock().unwrap().extend(results);
            return Ok(response(Some("done"), Vec::new()));
        }
        let call = match &self.script {
            Script::Ask => ToolCall {
                id: "call_1".into(),
                name: "ask_agent_02".into(),
                args: serde_json::json!({ "message": "look into it" }),
                extra: None,
            },
            Script::Plan(targets) => ToolCall {
                id: "call_1".into(),
                name: "plan".into(),
                args: serde_json::json!({
                    "tasks": targets
                        .iter()
                        .map(|to| serde_json::json!({ "to": to, "message": "look into it" }))
                        .collect::<Vec<_>>()
                }),
                extra: None,
            },
        };
        Ok(response(Some(""), vec![call]))
    }
}

/// 進行役 `agent_01`（接続先は `agent_02` と `agent_03` — `plan` は 2 体以上で生える）と、
/// ワーカー 2 体の村。`start_worker = false` なら `agent_02` を起動しない。
/// 進行役が受け取ったツール結果を返す。
async fn run(language: Language, script: Script, silent: bool, start_worker: bool) -> String {
    let dir = TempDir::new("run");
    let results = Arc::new(Mutex::new(Vec::new()));
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(Arc::new(Village {
            script,
            silent,
            results: Arc::clone(&results),
        }))),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig {
            schedule_interval: Duration::from_secs(3600),
            ..OrchestratorConfig::default()
        },
    )
    .await
    .expect("bootstrap できること");
    orchestrator.set_language(language).await.unwrap();
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "default", "mock-model"))
        .await
        .unwrap();

    let worker = AgentId::from("agent_02");
    let third = AgentId::from("agent_03");
    for (id, name) in [(&worker, "Worker"), (&third, "Third")] {
        orchestrator
            .create_agent(AgentSpec::new(id.clone(), name, "tpl"))
            .await
            .unwrap();
    }
    let coordinator = AgentId::from("agent_01");
    let mut spec = AgentSpec::new(coordinator.clone(), "Coordinator", "tpl");
    spec.connected_agents = vec![worker.clone(), third.clone()];
    orchestrator.create_agent(spec).await.unwrap();

    orchestrator.start_agent(&third).await.unwrap();
    if start_worker {
        orchestrator.start_agent(&worker).await.unwrap();
    }
    orchestrator.start_agent(&coordinator).await.unwrap();

    orchestrator
        .send_user_message(&coordinator, "go")
        .await
        .unwrap();

    // 静かになるまで待つのではなく、進行役がツール結果を受け取るまで待つ（期限つき）。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(result) = results.lock().unwrap().first() {
            return result.clone();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "進行役がツール結果を受け取らないまま 20 秒が過ぎた"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn has_japanese(text: &str) -> bool {
    text.chars()
        .any(|c| matches!(c, '\u{3000}'..='\u{30ff}' | '\u{4e00}'..='\u{9fff}' | '\u{ff00}'..='\u{ffef}'))
}

/// 1. 停止中の相手への `ask`。理由の後ろはコアのエラー文（日本語に固定 — Spec 13）なので、
///    確かめるのは依頼主の文の枠だけ。
#[tokio::test]
async fn asking_a_stopped_agent_reports_in_the_village_language() {
    let en = run(Language::En, Script::Ask, false, false).await;
    assert!(en.starts_with("Could not ask them: "), "英語の村: {en}");

    let ja = run(Language::Ja, Script::Ask, false, false).await;
    assert!(ja.starts_with("相手に尋ねられませんでした: "), "日本語の村: {ja}");
}

/// 2. ワーカーが本文を返さなかった — コアが代わりに書いた返事が、`Reply` で依頼主へ届く。
#[tokio::test]
async fn a_reply_written_by_the_core_reaches_the_requester_in_the_village_language() {
    let en = run(Language::En, Script::Ask, true, true).await;
    assert_eq!(en, "(The model returned no text. Please ask again.)");

    let ja = run(Language::Ja, Script::Ask, true, true).await;
    assert_eq!(ja, "（モデルから本文が返りませんでした。もう一度頼んでみてください。）");
}

/// 3. 接続先でない宛先への `plan` — 何も配送せずに差し戻す文。
#[tokio::test]
async fn a_plan_to_an_unconnected_target_is_refused_in_the_village_language() {
    let en = run(Language::En, Script::Plan(vec!["agent_09"]), false, true).await;
    assert_eq!(
        en,
        "The target \"agent_09\" of request 1 is not one of your connections. \
         You can ask Worker, Third. Nothing was delivered."
    );

    let ja = run(Language::Ja, Script::Plan(vec!["agent_09"]), false, true).await;
    assert_eq!(
        ja,
        "1 件目の宛先「agent_09」は、あなたの接続先ではありません。\
         頼めるのは Worker、Third です。何も配送していません。"
    );
}

/// 4. 束ね — 見出しの枠も言語で分ける。答えた本文（`ok`）はワーカーが書いたものでそのまま。
#[tokio::test]
async fn the_bundle_headings_follow_the_village_language() {
    let en = run(Language::En, Script::Plan(vec!["agent_02", "agent_03"]), false, true).await;
    assert_eq!(en, "## agent_02 (Worker)\nok\n\n## agent_03 (Third)\nok");
    assert!(!has_japanese(&en), "英語の束ねに日本語が無い: {en}");

    let ja = run(Language::Ja, Script::Plan(vec!["agent_02", "agent_03"]), false, true).await;
    assert_eq!(ja, "## agent_02（Worker）\nok\n\n## agent_03（Third）\nok");
}
