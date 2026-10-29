//! Spec 72 D3 — 予定の後判定（Spec 46）と完了後要約（Spec 28）は、根のターンが
//! `run` の承認待ちで止まったときは見送る。計器 1 行で残し、再依頼は起こさない。
//!
//! 診断ログの宛先はプロセスで 1 つなので、ログを読むテストはこのファイルで 1 本。
//! 予定の土台は `schedule_acceptance.rs` と同じ（tick は手動・固定時刻）。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fuseforks_core::event::CoreEvent;
use fuseforks_core::llm::{
    ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, ToolCall, Usage,
};
use fuseforks_core::model::{AgentId, AgentSpec, Endpoint, ModelTemplate};
use fuseforks_core::orchestrator::ProbeApprovals;
use fuseforks_core::schedule::{Acceptance, Recurrence, ScheduleOptions, Weekday};
use fuseforks_core::schedule_probe::{PROBE_TIMEOUT_DEFAULT, ScheduleProbe, SessionMode};
use fuseforks_core::tools::run::RunTool;
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-approval-acceptance-log-{}",
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

struct ApproveAll;

impl ProbeApprovals for ApproveAll {
    fn is_approved(&self, _key: &str) -> bool {
        true
    }
}

/// 毎回 `run` を 1 本呼ぶ（Unknown のまま）。再依頼が来てもまた止まる形。
struct AlwaysUnknown;

#[async_trait::async_trait]
impl LlmBackend for AlwaysUnknown {
    fn name(&self) -> &str {
        "always-unknown"
    }
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let has_run = req.tools.iter().any(|t| t.name == "run");
        let answered = req.messages.iter().any(|m| m.role == fuseforks_core::llm::Role::Tool);
        let tool_calls = if has_run && !answered {
            vec![ToolCall {
                id: "call_0".into(),
                name: "run".into(),
                args: serde_json::json!({ "command": "git", "args": ["status"] }),
                extra: None,
            }]
        } else {
            Vec::new()
        };
        Ok(ChatResponse {
            text: Some(if tool_calls.is_empty() { "終わりました".into() } else { String::new() }),
            finish: if tool_calls.is_empty() { Finish::Stop } else { Finish::ToolUse },
            tool_calls,
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

fn jst_at(y: i32, m: u32, d: u32, h: u32, min: u32, s: u32) -> chrono::DateTime<chrono::FixedOffset> {
    use chrono::TimeZone;
    chrono::FixedOffset::east_opt(9 * 3600)
        .unwrap()
        .with_ymd_and_hms(y, m, d, h, min, s)
        .single()
        .unwrap()
}

/// 常に不一致を返す検収（`NO` を印字・期待は `OK`）。
fn mismatching_acceptance() -> Acceptance {
    let (command, args) = if cfg!(windows) {
        ("cmd".to_owned(), vec!["/C".to_owned(), "echo NO".to_owned()])
    } else {
        ("sh".to_owned(), vec!["-c".to_owned(), "echo NO".to_owned()])
    };
    Acceptance {
        probe: ScheduleProbe {
            command,
            args,
            expect: "OK".to_owned(),
            timeout_secs: PROBE_TIMEOUT_DEFAULT,
            cwd: None,
        },
        max_attempts: 2,
    }
}

#[tokio::test]
async fn an_approval_stop_skips_the_acceptance_and_does_not_redeliver() {
    let dir = TempDir::new();
    let log = dir.0.join("fuseforks.log");
    fuseforks_core::open_log(&log).expect("開けること");

    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(Arc::new(AlwaysUnknown))),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig {
            schedule_interval: Duration::from_secs(3600),
            ..OrchestratorConfig::default()
        },
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
    orchestrator.create_agent(spec).await.unwrap();
    orchestrator.start_agent(&agent).await.unwrap();
    orchestrator
        .set_probe_approvals(Arc::new(ApproveAll) as Arc<dyn ProbeApprovals>)
        .await;
    let task = orchestrator
        .create_schedule(
            agent.clone(),
            "見張って".into(),
            Recurrence::Weekly {
                weekday: Weekday::Thu,
                hour: 17,
                minute: 0,
            },
            ScheduleOptions {
                probe: None,
                session_mode: SessionMode::Continue,
                summarize_after: true,
                acceptance: Some(mismatching_acceptance()),
                auto_approve_plans: false,
            },
        )
        .await
        .unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator
        .run_schedule_tick(jst_at(2026, 7, 30, 17, 0, 29))
        .await;
    let mut events = Vec::new();
    while let Ok(Ok(event)) = tokio::time::timeout(Duration::from_millis(900), rx.recv()).await {
        events.push(event);
    }

    let deliveries = events
        .iter()
        .filter(|e| match e {
            CoreEvent::MessageSent { message } => {
                matches!(message.from, Endpoint::System) && message.content.starts_with("【定期実行: ")
            }
            _ => false,
        })
        .count();
    assert_eq!(deliveries, 1, "再依頼は起きない（承認待ちを「終わった」と読まない）: {events:?}");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, CoreEvent::ApprovalStopped { stopped: true, .. })),
        "根のターンは承認待ちで止まっている: {events:?}"
    );

    let text = std::fs::read_to_string(&log).expect("ログが読めること");
    let skipped: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("acceptance:") && l.contains("outcome=skipped"))
        .collect();
    assert_eq!(skipped.len(), 1, "見送った事実は 1 行で残す（黙って飛ばさない）: {text}");
    assert!(
        skipped[0].contains(&format!("schedule={} attempt=1/2", task.id))
            && skipped[0].contains("reason=approval_pending")
            && skipped[0].contains("summarize=skipped"),
        "予定・試行・理由・要約の見送りが読める: {}",
        skipped[0]
    );
    assert!(
        !text.lines().any(|l| l.contains("acceptance:") && l.contains("outcome=no_match")),
        "検収コマンドは走っていない: {text}"
    );
    assert!(
        !text.contains("summarize:"),
        "要約も走っていない: {text}"
    );
    assert!(
        !orchestrator.acceptance_reports().await.contains_key(&task.id),
        "直近の結末も書かない（走っていないので）"
    );
}
