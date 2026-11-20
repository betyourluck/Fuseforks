//! 打ち切りの後に、まとめの LLM 呼び出しを始めない（Spec 77 #15）。
//!
//! # 何が起きていたか
//!
//! 打ち切り（Spec 10）の検査点は周の頭だけで、最終周（ツール実行の上限）の LLM 呼び出しやツール実行の
//! 最中に打ち切られると、ループを抜けた後の「まとめ呼び出し」へそのまま進んでいた。Spec 10 は
//! 「まとめの LLM 呼び出しはしない（打ち切りの直後にもう 1 回課金しない）」と決めており、優先順位も
//! cancel が先（`data_contract` の `token_budget.precedence`）。実際には 1 回余分に送って課金し、
//! ターンは打ち切りではなく上限到達として閉じていた（打ち切りの System 行も `TurnInterrupted` も出ない）。
//!
//! # 何を確かめるか
//!
//! 上限 1 周の個体に、ツールを呼ぶ応答を返す。その 1 周目の LLM 呼び出しの最中に打ち切る。
//! 呼び出しは完走させ（飛行中の呼び出しは止めない）、ツールも実行される。その後にまとめの
//! 2 回目が呼ばれず、ターンが打ち切りとして閉じることを見る。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use fuseforks_core::event::CoreEvent;
use fuseforks_core::llm::{ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, ToolCall, Usage};
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
            "fuseforks-interrupt-summary-{tag}-{}",
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

/// 即答ツール。最終周で実際に 1 本実行させるための燃料。
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

/// ツールを呼ぶ応答だけを返す（本文は空）。テストが許可するまで応答を返さない — 打ち切りが
/// 飛行中の呼び出しの最中に届くことを壁時計に依存させない。
struct GatedToolBackend {
    calls: AtomicU32,
    started: tokio::sync::mpsc::UnboundedSender<u32>,
    gate: Arc<tokio::sync::Semaphore>,
}

#[async_trait::async_trait]
impl LlmBackend for GatedToolBackend {
    fn name(&self) -> &str {
        "gated-tool"
    }

    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.started.send(n);
        self.gate.acquire().await.expect("gate は閉じない").forget();
        Ok(ChatResponse {
            text: Some(String::new()),
            tool_calls: vec![ToolCall {
                id: format!("call_{n}"),
                name: "busy_probe".into(),
                args: serde_json::json!({ "round": n }),
                extra: None,
            }],
            finish: Finish::ToolUse,
            usage: Usage { prompt: 1, completion: 1, cache_read: 0, cache_write: 0, cache_write_1h: 0, reasoning: 0 },
            grounding: Default::default(),
            reasoning_summary: Vec::new(),
            replay: None,
        })
    }
}

/// 静穏窓は統計の周期（1 秒）より**短く**取る（#86）。
async fn collect_until_quiet(rx: &mut Receiver<CoreEvent>, quiet: Duration) -> Vec<CoreEvent> {
    let mut events = Vec::new();
    while let Ok(Ok(event)) = tokio::time::timeout(quiet, rx.recv()).await {
        events.push(event);
    }
    events
}

#[tokio::test]
async fn an_interrupt_during_the_last_round_skips_the_summary_call() {
    let dir = TempDir::new("last-round");
    let (tx, mut started) = tokio::sync::mpsc::unbounded_channel();
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let backend = Arc::new(GatedToolBackend {
        calls: AtomicU32::new(0),
        started: tx,
        gate: Arc::clone(&gate),
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
    orchestrator.register_tool(Arc::new(BusyTool)).await;
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "default", "mock-model"))
        .await
        .unwrap();
    let id = AgentId::from("agent_01");
    let mut spec = AgentSpec::new(id.clone(), "Bob", "tpl");
    // 上限 1 周 = 最初の周が最終周。ツールを呼んだ応答でループを抜け、まとめの分岐へ進む形。
    spec.max_tool_iterations = Some(1);
    orchestrator.create_agent(spec).await.unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "keep researching").await.unwrap();
    started.recv().await.expect("最終周の呼び出しが始まること");
    orchestrator.interrupt_turn(&id).await;
    // 飛行中の呼び出しは完走させる。許可を多めに出すのは、まとめの呼び出しが走ってしまったときに
    // テストが止まらず「2 回呼ばれた」で赤くなるため。
    gate.add_permits(5);
    let events = collect_until_quiet(&mut rx, Duration::from_millis(400)).await;

    assert_eq!(
        backend.calls.load(Ordering::SeqCst),
        1,
        "打ち切りの後に、まとめの LLM 呼び出しを始めない（打ち切りの直後にもう 1 回課金しない）"
    );
    assert!(
        events.iter().any(|e| matches!(e, CoreEvent::TurnInterrupted { .. })),
        "ターンは打ち切りとして閉じる（上限到達として閉じない）: {events:?}"
    );
}
