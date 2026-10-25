//! **Anthropic の web 検索の写しが、ターンの層を通って次の要求に受け取ったまま載る**ことを
//! 機械で留める（Spec 71 P1 / `llm_wire.anthropic_server_tools`）。
//!
//! バックエンドは筋書きの偽物だが、**応答は実物の `anthropic::decode` で読み、要求は実物の
//! `anthropic::encode` でワイヤの形へ写して記録する** — decode → ターンの層 → encode の
//! 往復をまとめて通す。応答の JSON は P0 の実測（`specs/71` の「P0 実測記録」）を縮めた形。
//!
//! 見るのは 3 つ — 村のツールと同じ周の保留（D5）/ `pause_turn` の続き（D4）/
//! 最後の周で止まったときのまとめ呼び出し（D4）。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fuseforks_core::event::CoreEvent;
use fuseforks_core::llm::anthropic::{self, AnthropicSkills};
use fuseforks_core::llm::{
    ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, Role, ToolChoice, Usage, wire,
};
use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::tool::{AgentTool, ToolContext};
use fuseforks_core::world::Language;
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};
use serde_json::{Value, json};

/// テストごとに別のフォルダ。
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "fuseforks-anthropic-web-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
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

/// 本文だけを返す村のツール。
struct ClockTool;

#[async_trait::async_trait]
impl AgentTool for ClockTool {
    fn name(&self) -> &str {
        "clock"
    }
    fn description(&self, _language: Language) -> String {
        "テスト用。時刻を返す".into()
    }
    fn parameters(&self, _language: Language) -> serde_json::Value {
        json!({ "type": "object" })
    }
    async fn call(
        &self,
        _ctx: &ToolContext,
        _args: &serde_json::Value,
    ) -> fuseforks_core::CoreResult<String> {
        Ok("2026-10-25 06:30 (JST)".to_owned())
    }
}

/// 筋書き: 応答の JSON を順に返す（実物の decode で読む）。尽きたら・まとめ呼び出しには答える。
/// 要求は canonical のまま記録し、ワイヤの形（実物の encode）も一緒に残す。
struct Script {
    responses: Mutex<Vec<Value>>,
    requests: Mutex<Vec<(ChatRequest, Value)>>,
}

impl Script {
    fn new(responses: Vec<Value>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().rev().collect()),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<(ChatRequest, Value)> {
        self.requests.lock().unwrap().clone()
    }
}

fn plain(text: &str) -> ChatResponse {
    ChatResponse {
        text: Some(text.to_owned()),
        tool_calls: Vec::new(),
        finish: Finish::Stop,
        usage: Usage {
            prompt: 10,
            completion: 5,
            ..Usage::default()
        },
        grounding: Default::default(),
        reasoning_summary: Vec::new(),
        replay: None,
    }
}

#[async_trait::async_trait]
impl LlmBackend for Script {
    fn name(&self) -> &str {
        "anthropic-web-script"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let wire_body =
            serde_json::to_value(anthropic::encode(&req, AnthropicSkills { web_search: true }))
                .unwrap();
        self.requests.lock().unwrap().push((req.clone(), wire_body));
        if matches!(req.tool_choice, ToolChoice::None) {
            return Ok(plain("まとめました"));
        }
        let Some(raw) = self.responses.lock().unwrap().pop() else {
            return Ok(plain("答えました"));
        };
        let parsed: wire::AnthropicResponse = serde_json::from_value(raw).unwrap();
        anthropic::decode(parsed)
    }
}

fn usage(requests: u64) -> Value {
    let mut u = json!({ "input_tokens": 100, "output_tokens": 20 });
    if requests > 0 {
        u["server_tool_use"] = json!({ "web_search_requests": requests, "web_fetch_requests": 0 });
    }
    u
}

fn search_result() -> Value {
    json!({
        "type": "web_search_tool_result",
        "tool_use_id": "srvtoolu_1",
        "content": [{
            "type": "web_search_result",
            "url": "https://example.test/weather",
            "title": "東京の天気",
            "page_age": "0 days ago",
            "encrypted_content": "EqgfCioIARgB"
        }]
    })
}

fn cited_text() -> Value {
    json!({
        "type": "text",
        "text": "東京は晴れです。",
        "citations": [{
            "type": "web_search_result_location",
            "url": "https://example.test/weather",
            "title": "東京の天気",
            "cited_text": "晴れ",
            "encrypted_index": "Eo8BCioIAhgB"
        }]
    })
}

/// 村を 1 つ建て、1 体へ依頼して、イベントが止むまで待つ。
async fn run(
    backend: Arc<Script>,
    max_tool_iterations: Option<u8>,
) -> (Orchestrator, Vec<CoreEvent>, TempDir) {
    let dir = TempDir::new();
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
    orchestrator.set_language(Language::Ja).await.unwrap();
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();
    orchestrator.register_tool(Arc::new(ClockTool)).await;
    let agent = AgentId::from("agent_web");
    let mut spec = AgentSpec::new(agent.clone(), "調べ係", "tpl");
    spec.max_tool_iterations = max_tool_iterations;
    orchestrator.create_agent(spec).await.unwrap();
    orchestrator.start_agent(&agent).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&agent, "時刻と天気を教えて")
        .await
        .unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    (orchestrator, events, dir)
}

fn block_types(message: &Value) -> Vec<String> {
    message["content"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["type"].as_str().unwrap_or("?").to_owned())
        .collect()
}

/// D5 — 村のツールと同じ周の保留。**保留の応答が次の要求に受け取ったまま載り**、続く発話は
/// `tool_result` だけ。`tool_use` は写しの中の 1 つだけで、`tool_calls` から二重に作らない。
#[tokio::test]
async fn a_deferred_search_rides_back_verbatim_and_only_tool_results_follow() {
    let deferred = json!({
        "content": [
            { "type": "tool_use", "id": "toolu_1", "name": "clock", "input": {} },
            { "type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search",
              "input": { "query": "東京 天気 今日" } }
        ],
        "stop_reason": "tool_use",
        "usage": usage(0)
    });
    let finished = json!({
        "content": [search_result(), cited_text()],
        "stop_reason": "end_turn",
        "usage": usage(1)
    });
    let backend = Arc::new(Script::new(vec![deferred.clone(), finished]));
    let (_orchestrator, events, _dir) = run(backend.clone(), None).await;

    let requests = backend.requests();
    assert_eq!(requests.len(), 2, "保留の応答と、その続きの 2 要求");
    let messages = requests[1].1["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3, "利用者 → 保留の応答 → tool_result");
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(
        messages[1]["content"], deferred["content"],
        "保留の応答は受け取ったまま（並びも欄も）送り返す"
    );
    assert_eq!(block_types(&messages[1]), vec!["tool_use", "server_tool_use"]);
    assert_eq!(messages[2]["role"], "user");
    assert_eq!(
        block_types(&messages[2]),
        vec!["tool_result"],
        "続く発話は tool_result だけ（本文を足すと結果の無いサーバー側ツールの 400）"
    );

    // 出典は引用から写り、エンジンは anthropic。
    let reply = events
        .iter()
        .find_map(|e| match e {
            CoreEvent::MessageSent { message } if message.content.contains("晴れ") => {
                Some(message.clone())
            }
            _ => None,
        })
        .expect("答えが配られること");
    assert_eq!(reply.grounding.sources.len(), 1);
    assert_eq!(reply.grounding.sources[0].uri, "https://example.test/weather");
    assert_eq!(
        serde_json::to_value(reply.grounding.engine).unwrap(),
        json!("anthropic")
    );
}

/// D4 — `pause_turn` の続き。止まった応答を写しのまま積み、**利用者の発話を足さずに**
/// 次の周を回す（要求は assistant の発話で終わる）。
#[tokio::test]
async fn a_paused_response_continues_without_a_user_message() {
    let paused = json!({
        "content": [
            { "type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search",
              "input": { "query": "東京 天気 今日" } },
            search_result()
        ],
        "stop_reason": "pause_turn",
        "usage": usage(1)
    });
    let finished = json!({
        "content": [cited_text()],
        "stop_reason": "end_turn",
        "usage": usage(0)
    });
    let backend = Arc::new(Script::new(vec![paused.clone(), finished]));
    let (_orchestrator, events, _dir) = run(backend.clone(), None).await;

    let requests = backend.requests();
    assert_eq!(requests.len(), 2, "止まった応答と、その続きの 2 要求");
    let messages = requests[1].1["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2, "利用者 → 止まった応答（「続けて」を足さない）");
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[1]["content"], paused["content"]);
    assert!(
        events.iter().any(|e| matches!(
            e,
            CoreEvent::MessageSent { message } if message.content.contains("晴れ")
        )),
        "続きの周の答えが配られること"
    );
    assert!(
        !events.iter().any(|e| matches!(e, CoreEvent::ToolLimitReached { .. })),
        "最後の周でなければ上限の到達にしない"
    );
}

/// D4 — **最後の周で止まった応答は積まない**。まとめ呼び出しの要求が assistant の発話で
/// 終わると 5 世代の prefill 禁止に掛かりうる。上限の到達として扱う。
#[tokio::test]
async fn a_pause_on_the_last_round_is_not_carried_into_the_summary() {
    let paused = json!({
        "content": [
            { "type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search",
              "input": { "query": "東京 天気 今日" } },
            search_result()
        ],
        "stop_reason": "pause_turn",
        "usage": usage(1)
    });
    let backend = Arc::new(Script::new(vec![paused]));
    let (_orchestrator, events, _dir) = run(backend.clone(), Some(1)).await;

    let requests = backend.requests();
    assert_eq!(requests.len(), 2, "止まった 1 周と、まとめ呼び出し");
    let (summary_req, summary_wire) = &requests[1];
    assert!(matches!(summary_req.tool_choice, ToolChoice::None), "まとめ呼び出し");
    let last = summary_wire["messages"].as_array().unwrap().last().unwrap().clone();
    assert_ne!(last["role"], "assistant", "assistant の発話で終わらない");
    assert!(
        summary_req.messages.iter().all(|m| m.role != Role::Assistant),
        "止まった応答を積まない"
    );
    assert!(
        events.iter().any(|e| matches!(e, CoreEvent::ToolLimitReached { .. })),
        "上限の到達として扱う"
    );
}

/// イベントが止むまで集める（`tests/orchestrator.rs` と同じ作法）。
///
/// 窓は**この系で最も短い定期イベントの周期より短く**する（`failures.md` #86）。
async fn drain_until_quiet(
    rx: &mut tokio::sync::broadcast::Receiver<CoreEvent>,
    quiet: Duration,
) -> Vec<CoreEvent> {
    let mut out = Vec::new();
    loop {
        match tokio::time::timeout(quiet, rx.recv()).await {
            Ok(Ok(event)) => out.push(event),
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
            _ => break,
        }
    }
    out
}
