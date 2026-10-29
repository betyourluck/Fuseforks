//! Spec 72 の計器 — `turn cut: … reason=approval_pending unknown=K` と、同じターンの
//! `turn:` 行の `stop=approval_pending`（分母は上限・`repeat_max` は写してある）。
//!
//! 診断ログの宛先はプロセスで 1 つなので、ログを読むテストはこのファイルで 1 本。
//! 配線の副作用（System 行・印・Reply）は `approval_stops_turn.rs` が持つ。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fuseforks_core::event::CoreEvent;
use fuseforks_core::llm::{
    ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, ToolCall, Usage,
};
use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::tools::run::RunTool;
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-approval-stop-log-{}",
            std::process::id()
        ));
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

/// 最初の 1 回だけ `run` を 2 本（別々のコマンド）並べて呼ぶ。
struct TwoUnknowns {
    chats: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl LlmBackend for TwoUnknowns {
    fn name(&self) -> &str {
        "two-unknowns"
    }
    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let first = self.chats.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
        let tool_calls = if first {
            vec![
                ToolCall {
                    id: "call_0".into(),
                    name: "run".into(),
                    args: serde_json::json!({ "command": "git", "args": ["status"] }),
                    extra: None,
                },
                ToolCall {
                    id: "call_1".into(),
                    name: "run".into(),
                    args: serde_json::json!({ "command": "fd", "args": ["."] }),
                    extra: None,
                },
            ]
        } else {
            Vec::new()
        };
        Ok(ChatResponse {
            text: Some(if first { String::new() } else { "終わりました".into() }),
            tool_calls,
            finish: if first { Finish::ToolUse } else { Finish::Stop },
            usage: Usage {
                prompt: 10,
                completion: 2,
                cache_read: 0,
                cache_write: 0,
                cache_write_1h: 0,
                reasoning: 0,
            },
            grounding: Default::default(),
            reasoning_summary: Vec::new(),
            replay: None,
        })
    }
}

#[tokio::test]
async fn the_cut_line_counts_unknowns_and_the_turn_line_keeps_its_denominator() {
    let dir = TempDir::new();
    let log = dir.0.join("fuseforks.log");
    fuseforks_core::open_log(&log).expect("開けること");

    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(Arc::new(TwoUnknowns {
            chats: std::sync::atomic::AtomicUsize::new(0),
        }))),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig::default(),
    )
    .await
    .expect("bootstrap できること");
    orchestrator
        .set_language(fuseforks_core::world::Language::Ja)
        .await
        .unwrap();
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();
    orchestrator
        .register_tool(Arc::new(RunTool::new(ConfigStore::new(&dir.0))))
        .await;
    let agent = AgentId::from("agent_01");
    let mut spec = AgentSpec::new(agent.clone(), "ロボット", "tpl");
    spec.enabled_tools = Some(vec!["run".into()]);
    spec.max_tool_iterations = Some(12);
    orchestrator.create_agent(spec).await.unwrap();
    orchestrator.start_agent(&agent).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&agent, "2 つ試して").await.unwrap();
    while let Ok(Ok(event)) = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
        let _ = event;
    }
    // ログは非同期に書かれうるので、行が出るまで少し待つ。
    let text = {
        let mut text = String::new();
        for _ in 0..20 {
            text = std::fs::read_to_string(&log).expect("ログが読めること");
            if text.contains("turn cut:") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        text
    };

    let cuts: Vec<&str> = text.lines().filter(|l| l.contains("turn cut:")).collect();
    assert_eq!(cuts.len(), 1, "止めた周に 1 行だけ: {text}");
    assert!(
        cuts[0].contains("agent=agent_01 round=1 reason=approval_pending unknown=2"),
        "理由と、この周で Unknown を返した本数（pending の件数ではない）: {}",
        cuts[0]
    );
    let turns: Vec<&str> = text.lines().filter(|l| l.contains("turn: agent=agent_01")).collect();
    assert_eq!(turns.len(), 1, "使用量の行は 1 本（turn: の接頭辞 — 自前の行ではない）: {text}");
    assert!(
        turns[0].contains("rounds=1/12") && turns[0].contains("stop=approval_pending"),
        "分母は上限、stop は approval_pending（rounds=1/- の嘘を出さない）: {}",
        turns[0]
    );
    assert_eq!(
        text.lines().filter(|l| l.contains("turn start:")).count(),
        1,
        "2 本目のターンは起きない: {text}"
    );
    // 承認待ちのターンも使用量を記録する（#103 の系譜）。
    assert!(
        text.lines().filter(|l| l.contains("turn budget exhausted:") || l.contains("turn interrupted:")).count() == 0,
        "別の出口の行は出ない: {text}"
    );
    let _ = CoreEvent::ConversationCleared;
}
