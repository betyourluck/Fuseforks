//! **関数呼び出しの段の署名が検索の `toolCall` に付いた Gemini の応答が、ターンの層を通って
//! 次の周の要求に受け取ったまま載る**ことを機械で留める（`failures.md` #162）。
//!
//! 2026-11-17 / 11-21 の実機（`gemini-4-hydrogen`）では、検索と並列に出た関数呼び出しには署名が
//! 無く、decode が `toolCall` / `toolResponse` を落として組み直したため、次の周が 400
//! `Function call is missing a thought_signature in functionCall parts` で落ち、個体が止まった。
//!
//! バックエンドは筋書きの偽物だが、**応答は実物の `gemini::decode` で読み、要求は実物の
//! `gemini::encode` でワイヤの形へ写して記録する**（Spec 71 の `anthropic_web_search_turn.rs` と
//! 同じ作法）。応答の JSON は 2026-11-21 の probe の並びを縮めた形。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fuseforks_core::event::CoreEvent;
use fuseforks_core::llm::gemini::{self, GeminiSkills};
use fuseforks_core::llm::{
    ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, ToolChoice, Usage, wire,
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
            "fuseforks-gemini-replay-{}-{}",
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

/// 本文だけを返す村のツール（実機の `MCP_DOCKER__fetch` の代わり）。
struct FetchTool;

#[async_trait::async_trait]
impl AgentTool for FetchTool {
    fn name(&self) -> &str {
        "fetch"
    }
    fn description(&self, _language: Language) -> String {
        "テスト用。URL の本文を返す".into()
    }
    fn parameters(&self, _language: Language) -> serde_json::Value {
        json!({ "type": "object", "properties": { "url": { "type": "string" } } })
    }
    async fn call(
        &self,
        _ctx: &ToolContext,
        _args: &serde_json::Value,
    ) -> fuseforks_core::CoreResult<String> {
        Ok("# Announcing Rust\n本文".to_owned())
    }
}

/// 筋書き: 応答の JSON を順に返す（実物の decode で読む）。要求はワイヤの形（実物の encode）で残す。
struct Script {
    responses: Mutex<Vec<Value>>,
    requests: Mutex<Vec<Value>>,
}

impl Script {
    fn new(responses: Vec<Value>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().rev().collect()),
            requests: Mutex::new(Vec::new()),
        }
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
        "gemini-replay-script"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let skills = GeminiSkills {
            google_search: true,
            url_context: false,
        };
        let wire_body = serde_json::to_value(gemini::encode(&req, skills)).unwrap();
        self.requests.lock().unwrap().push(wire_body);
        if matches!(req.tool_choice, ToolChoice::None) {
            return Ok(plain("まとめました"));
        }
        let Some(raw) = self.responses.lock().unwrap().pop() else {
            return Ok(plain("答えました"));
        };
        let parsed: wire::GeminiResponse = serde_json::from_value(raw).unwrap();
        gemini::decode(parsed)
    }
}

/// 検索の `toolCall` と並列に関数呼び出しが出た応答（2026-11-21 の probe の並び）。
/// 段の署名は `toolCall` が持ち、関数呼び出しには署名が無い。
fn parallel_to_search() -> Value {
    json!({
        "candidates": [{
            "content": { "role": "model", "parts": [
                { "text": "検索して記事を取りに行く", "thought": true },
                { "toolCall": { "toolType": "GOOGLE_SEARCH_WEB",
                                "args": { "queries": ["Rust 1.95 release"] }, "id": "call_1" },
                  "thoughtSignature": "EtABCs-search-step" },
                { "functionCall": { "name": "fetch", "args": { "url": "https://blog.rust-lang.org/a" },
                                    "id": "call_fn_1" } },
                { "toolResponse": { "toolType": "GOOGLE_SEARCH_WEB",
                                    "response": { "search_suggestions": "<style>" }, "id": "call_1" },
                  "thoughtSignature": "EqXdAQ-search-result" }
            ]},
            "finishReason": "STOP"
        }],
        "usageMetadata": { "promptTokenCount": 504, "candidatesTokenCount": 87, "totalTokenCount": 591 }
    })
}

fn final_answer() -> Value {
    json!({
        "candidates": [{
            "content": { "role": "model", "parts": [
                { "text": "Rust 1.95 の目玉は…", "thoughtSignature": "Ep4BCp-final" }
            ]},
            "finishReason": "STOP"
        }],
        "usageMetadata": { "promptTokenCount": 900, "candidatesTokenCount": 40, "totalTokenCount": 940 }
    })
}

/// 村を 1 つ建て、1 体へ依頼して、イベントが止むまで待つ。
async fn run(backend: Arc<Script>) -> (Orchestrator, Vec<CoreEvent>, TempDir) {
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
        .upsert_template(ModelTemplate::new("tpl", "既定", "gemini-4-hydrogen"))
        .await
        .unwrap();
    orchestrator.register_tool(Arc::new(FetchTool)).await;
    let agent = AgentId::from("agent_gemini");
    orchestrator
        .create_agent(AgentSpec::new(agent.clone(), "ジェミー", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&agent).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&agent, "Rust の最新版を調べて")
        .await
        .unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    (orchestrator, events, dir)
}

/// 関数呼び出しの署名が `toolCall` にある応答は、**次の周の要求に受け取った順・受け取った形のまま**
/// 載る（署名の無い思考の part だけを除く）。続く発話は `functionResponse` 1 本だけ。
#[tokio::test]
async fn a_call_parallel_to_search_rides_back_with_the_step_signature() {
    let response = parallel_to_search();
    let backend = Arc::new(Script::new(vec![response.clone(), final_answer()]));
    let (_orchestrator, events, _dir) = run(backend.clone()).await;

    let requests = backend.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2, "関数呼び出しの周と、その続きの 2 要求");
    let contents = requests[1]["contents"].as_array().unwrap();
    assert_eq!(contents.len(), 3, "利用者 → 応答の写し → functionResponse: {contents:#?}");

    let expected: Vec<Value> = response["candidates"][0]["content"]["parts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| !(p["thought"] == true && p.get("thoughtSignature").is_none()))
        .cloned()
        .collect();
    assert_eq!(contents[1]["role"], "model");
    assert_eq!(
        contents[1]["parts"],
        Value::Array(expected),
        "段の署名を持つ toolCall ごと、受け取った順のまま返す"
    );

    assert_eq!(contents[2]["role"], "user");
    let reply = &contents[2]["parts"][0]["functionResponse"];
    assert_eq!(reply["id"], "call_fn_1");
    assert_eq!(reply["name"], "fetch");

    // 個体は止まらず、答えが利用者へ届く。
    assert!(
        events.iter().any(|e| matches!(
            e,
            CoreEvent::MessageSent { message } if message.content.contains("Rust 1.95")
        )),
        "答えが届く"
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
