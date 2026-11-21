//! 同梱の `fetch` を**配布物の構築**で確かめる（Spec 78 P1 / `web_fetch_contract`）。
//!
//! - **本番の構築はループバックへ 1 本も繋がない**（契約 9）。IP の直書き（127.0.0.1）は送る前の判定が、
//!   名前（localhost）は解決器が断る。スタブを立てて**接続が 1 本も来ない**ことまで見る — 断った文が返るだけだと、
//!   繋いでから断る実装でも緑になる
//! - **計器の行**（契約 15）— 測っていない欄は `-`、本文は書かない
//! - **提示の門**（契約 1）— 既定集合の外。`enabledTools: null` の個体には出ず、チェックした個体にだけ出る
//!
//! ループバックのスタブへ**繋ぐ**テストは `tools/fetch/mod.rs` の中の単体だけ（ループバックを許す構築は
//! `#[cfg(test)]` の中にしか無く、ここからは見えない）。
//!
//! **診断の出口はプロセスで 1 つ**（`OnceLock`）なので、ログを読むテストはこのファイルに 1 つだけ。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fuseforks_core::event::CoreEvent;
use fuseforks_core::llm::{ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, Usage};
use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::tool::{AgentTool, ToolContext};
use fuseforks_core::tools::{FetchTool, BUNDLED_TOOL_NAMES, DEFAULT_ENABLED_TOOLS, WORK_DIR_TOOL_NAMES};
use fuseforks_core::world::Language;
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!("fuseforks-web-fetch-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn ctx() -> ToolContext {
    ToolContext {
        agent_id: AgentId::from("agent_x"),
        work_dir: None,
        cancel: None,
        rag_roots: Vec::new(),
        agent_names: Vec::new(),
        uses_blackboard: false,
        language: Language::Ja,
        run_approval: fuseforks_core::command::RunApproval::default(),
        judges_dir: None,
        turn_images: None,
    }
}

/// 接続を数えるだけのスタブ（何も返さない）。
async fn counting_stub() -> (u16, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&accepted);
    tokio::spawn(async move {
        while let Ok((_socket, _)) = listener.accept().await {
            count.fetch_add(1, Ordering::SeqCst);
        }
    });
    (port, accepted)
}

/// 本番の構築はループバックへ 1 本も繋がず、断った理由を返し、`fetch:` の行を 1 本ずつ残す。
#[tokio::test]
async fn the_production_build_never_connects_to_the_loopback() {
    let dir = TempDir::new("log");
    let log_path = dir.0.join("fuseforks.log");
    fuseforks_core::open_log(&log_path).expect("ログを開けること");

    let (port, accepted) = counting_stub().await;
    let tool = FetchTool::new("9.9.9");
    let literal = tool
        .call_with_media(&ctx(), &serde_json::json!({ "url": format!("http://127.0.0.1:{port}/") }))
        .await
        .unwrap();
    assert!(literal.text.contains("ループバックの住所なので読めません"), "{}", literal.text);
    let named = tool
        .call_with_media(&ctx(), &serde_json::json!({ "url": format!("http://localhost:{port}/"), "links": true }))
        .await
        .unwrap();
    assert!(
        named.text.contains("localhost（") && named.text.contains("ループバックの住所なので読めません"),
        "名前は解決器が断る（送信の失敗の連鎖から拾えている）: {}",
        named.text
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(accepted.load(Ordering::SeqCst), 0, "スタブへ 1 本も繋がない（robots.txt も含めて）");

    let body = std::fs::read_to_string(&log_path).expect("ログが読めること");
    let lines: Vec<&str> = body.lines().filter(|l| l.contains("fetch: agent=agent_x")).collect();
    assert_eq!(lines.len(), 2, "{body}");
    assert!(
        lines[0].contains(
            "fetch: agent=agent_x host=127.0.0.1 status=- bytes=- chars=- mode=- links=no share=- encoding=- defused=- redirects=0 elapsed_ms="
        ),
        "測っていない欄は -: {}",
        lines[0]
    );
    assert!(lines[0].ends_with("outcome=blocked_address"), "{}", lines[0]);
    assert!(lines[1].contains("host=localhost status=- ") && lines[1].contains(" links=yes "), "{}", lines[1]);
    assert!(lines[1].ends_with("outcome=blocked_address"), "{}", lines[1]);
}

#[test]
fn fetch_is_bundled_but_outside_the_default_set() {
    assert!(BUNDLED_TOOL_NAMES.contains(&"fetch"));
    assert!(!DEFAULT_ENABLED_TOOLS.contains(&"fetch"), "更新しただけで送信先を増やさない");
    assert!(!WORK_DIR_TOOL_NAMES.contains(&"fetch"), "作業フォルダを使わない");
    let mut sorted = BUNDLED_TOOL_NAMES;
    sorted.sort_unstable();
    assert_eq!(sorted, BUNDLED_TOOL_NAMES, "名前順");
}

struct Script {
    requests: Mutex<Vec<ChatRequest>>,
}

#[async_trait::async_trait]
impl LlmBackend for Script {
    fn name(&self) -> &str {
        "fetch-script"
    }
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.requests.lock().unwrap().push(req);
        Ok(ChatResponse {
            text: Some("読みました".into()),
            tool_calls: Vec::new(),
            finish: Finish::Stop,
            usage: Usage { prompt: 10, completion: 5, cache_read: 0, cache_write: 0, cache_write_1h: 0, reasoning: 0 },
            grounding: Default::default(),
            reasoning_summary: Vec::new(),
            replay: None,
        })
    }
}

async fn drain_until_quiet(rx: &mut tokio::sync::broadcast::Receiver<CoreEvent>, quiet: Duration) {
    while let Ok(Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) =
        tokio::time::timeout(quiet, rx.recv()).await
    {}
}

/// 提示した道具の名前（1 回目の要求）。
async fn offered(enabled_tools: Option<Vec<String>>, tag: &str) -> Vec<String> {
    let dir = TempDir::new(tag);
    let backend = Arc::new(Script { requests: Mutex::new(Vec::new()) });
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(dir.0.join("ws")),
        Arc::new(FixedBackendFactory::new(backend.clone())),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig { schedule_interval: Duration::from_secs(3600), ..OrchestratorConfig::default() },
    )
    .await
    .unwrap();
    orchestrator.set_language(Language::Ja).await.unwrap();
    orchestrator.upsert_template(ModelTemplate::new("tpl", "既定", "mock-model")).await.unwrap();
    orchestrator.register_tool(Arc::new(FetchTool::new("9.9.9"))).await;
    let agent = AgentId::from("agent_reader");
    let mut spec = AgentSpec::new(agent.clone(), "読み手", "tpl");
    spec.enabled_tools = enabled_tools;
    orchestrator.create_agent(spec).await.unwrap();
    orchestrator.start_agent(&agent).await.unwrap();
    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&agent, "このページを読んで").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(500)).await;
    let requests = backend.requests.lock().unwrap();
    requests[0].tools.iter().map(|t| t.name.clone()).collect()
}

/// 提示の門（契約 1）— `enabledTools: null` の個体には出ず、チェックした個体にだけ出る。
#[tokio::test]
async fn fetch_is_offered_only_to_agents_that_check_it() {
    let default = offered(None, "null").await;
    assert!(!default.iter().any(|n| n == "fetch"), "既定に従う個体には出さない: {default:?}");
    let checked = offered(Some(vec!["fetch".to_owned()]), "checked").await;
    assert!(checked.iter().any(|n| n == "fetch"), "チェックした個体には出す: {checked:?}");
}
