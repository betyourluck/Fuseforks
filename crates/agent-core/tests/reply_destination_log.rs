//! 答えの宛先と転送がログに残る（計器）。
//!
//! **起点は診断できなかったこと**（2026-08-11）— 利用者から
//! 「委譲したのにユーザーへ返る」と報告されたが、**ログのどの行にも宛先が
//! 記録されておらず**、`transfer_to_*` に至っては `tool:` 行を出す前に
//! ループを抜けるので **grep が構造的に 0 件しか返さなかった**。
//! 「使われていない」と「見えていない」が同じ 0 に畳まれていた（#90 の再演）。
//!
//! ここで留めるのは 2 点で、**対で見る** — 転送した回に `handoff:` が出て、
//! その相手の答えが `reply: … to=user` になること。
//! 片方だけでは「常に出る実装」と区別が付かない。
//!
//! **診断の出口はプロセスで 1 つ**（`OnceLock`）なので、**このファイルは
//! 1 テストだけ**にする（`tests/diag.rs` と同じ制約）。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_core::llm::{ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, ToolCall, Usage};
use agent_core::model::{AgentId, AgentSpec, ModelTemplate};
use agent_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-replylog-{tag}-{}",
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

/// 転送ツールが提示されていれば 1 度だけ転送し、以後は本文を返す。
struct HandoffOnceBackend;

#[async_trait::async_trait]
impl LlmBackend for HandoffOnceBackend {
    fn name(&self) -> &str {
        "handoff-once"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let transfer = req.tools.iter().find(|t| t.name.starts_with("transfer_to_"));
        // 既に転送された側（提示された相手が居ない）なら本文を返す。
        let tool_calls = match transfer {
            Some(tool) => vec![ToolCall {
                id: "call_1".into(),
                name: tool.name.clone(),
                args: serde_json::json!({ "message": "任せた" }),
                extra: None,
            }],
            None => Vec::new(),
        };
        Ok(ChatResponse {
            text: Some("答えです".into()),
            finish: if tool_calls.is_empty() {
                Finish::Stop
            } else {
                Finish::ToolUse
            },
            tool_calls,
            usage: Usage {
                prompt: 1,
                completion: 1,
                cache_read: 0,
                reasoning: 0,
            },
            grounding: Default::default(),
            reasoning_summary: Vec::new(),
        })
    }
}

#[tokio::test]
async fn a_handoff_is_logged_and_the_reply_goes_to_the_user() {
    let dir = TempDir::new("handoff");
    let log_path = dir.0.join("fuseforks.log");
    agent_core::open_log(&log_path).expect("ログを開けること");

    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(Arc::new(
            HandoffOnceBackend,
        ))),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig {
            schedule_interval: Duration::from_secs(3600),
            ..OrchestratorConfig::default()
        },
    )
    .await
    .expect("bootstrap できること");
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();

    let coordinator = AgentId::from("agent_01");
    let worker = AgentId::from("agent_02");
    orchestrator
        .create_agent(AgentSpec::new(worker.clone(), "ワーカー", "tpl"))
        .await
        .unwrap();
    let mut spec = AgentSpec::new(coordinator.clone(), "進行役", "tpl");
    spec.connected_agents = vec![worker.clone()];
    orchestrator.create_agent(spec).await.unwrap();
    orchestrator.start_agent(&worker).await.unwrap();
    orchestrator.start_agent(&coordinator).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&coordinator, "調べて")
        .await
        .unwrap();
    while tokio::time::timeout(Duration::from_millis(400), rx.recv())
        .await
        .is_ok()
    {}

    let body = std::fs::read_to_string(&log_path).expect("ログが読めること");

    assert!(
        body.contains("handoff: agent=agent_01"),
        "転送がログに残ること（これが無いと診断できない）:
{body}"
    );
    // **本題**: 転送で来た依頼の答えはユーザーへ行く。
    assert!(
        body.contains("reply: agent=agent_02") && body.contains("to=user"),
        "転送された側の答えは reply_to を持たないので user へ返る:
{body}"
    );
}
