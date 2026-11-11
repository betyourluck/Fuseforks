//! `tool:` 行の `elapsed_ms=`（2026-11-11）— ツール 1 本の実行にかかった時間。
//!
//! # なぜ足したか
//!
//! 周の中のツールは直列に走る（`turn.rs` の `for call in &calls`）。openai/codex は読み取り系の
//! ツールを同時に走らせており（`core/src/tools/parallel.rs`）、Fuseforks のログでは 7,045 周の
//! うち 1,585 周（22%）が複数のツールを呼び、全呼び出しの 45% がそこにあった。ただし `tool:` 行に
//! 時間が無いので、**並列にして何秒縮むかが測れなかった**。本体（並列化）を Spec にする前の計器。
//!
//! # 何を確かめるか
//!
//! 測っているのが**そのツールの実行**であること — 150 ms 眠るツールは 150 以上、続けて呼ぶ即答の
//! ツールはそれより小さい（ターンの頭から測る実装だと、後のツールにも前の 150 ms が乗る）。
//!
//! **診断の出口はプロセスで 1 つ**（`OnceLock`）なので、ログを読むテストはこのファイルに 1 つだけ。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fuseforks_core::event::CoreEvent;
use fuseforks_core::llm::{ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, Role, ToolCall, Usage};
use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::world::Language;
use fuseforks_core::{
    AgentTool, ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator,
    OrchestratorConfig, ToolContext,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("fuseforks-tool-elapsed-log-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 150 ms 眠ってから答えるツール。
struct SlowTool;

#[async_trait::async_trait]
impl AgentTool for SlowTool {
    fn name(&self) -> &str {
        "slow"
    }
    fn description(&self, _language: Language) -> String {
        "test tool".into()
    }
    fn parameters(&self, _language: Language) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    async fn call(&self, _ctx: &ToolContext, _args: &serde_json::Value) -> fuseforks_core::CoreResult<String> {
        tokio::time::sleep(Duration::from_millis(150)).await;
        Ok("slept".into())
    }
}

/// 即答するツール（負の対照）。
struct FastTool;

#[async_trait::async_trait]
impl AgentTool for FastTool {
    fn name(&self) -> &str {
        "fast"
    }
    fn description(&self, _language: Language) -> String {
        "test tool".into()
    }
    fn parameters(&self, _language: Language) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    async fn call(&self, _ctx: &ToolContext, _args: &serde_json::Value) -> fuseforks_core::CoreResult<String> {
        Ok("done".into())
    }
}

/// 1 周目に `slow` と `fast` を同じ周で呼び、2 周目で答える。
struct SlowThenFast;

#[async_trait::async_trait]
impl LlmBackend for SlowThenFast {
    fn name(&self) -> &str {
        "slow-then-fast"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let usage = Usage { prompt: 10, completion: 5, cache_read: 0, cache_write: 0, cache_write_1h: 0, reasoning: 0 };
        if req.messages.iter().any(|m| m.role == Role::Tool) {
            return Ok(ChatResponse {
                text: Some("done".into()),
                tool_calls: Vec::new(),
                finish: Finish::Stop,
                usage,
                grounding: Default::default(),
                reasoning_summary: Vec::new(),
                replay: None,
            });
        }
        let call = |id: &str, name: &str| ToolCall {
            id: id.into(),
            name: name.into(),
            args: serde_json::json!({}),
            extra: None,
        };
        Ok(ChatResponse {
            text: Some(String::new()),
            tool_calls: vec![call("call_slow", "slow"), call("call_fast", "fast")],
            finish: Finish::ToolUse,
            usage,
            grounding: Default::default(),
            reasoning_summary: Vec::new(),
            replay: None,
        })
    }
}

/// `tool:` 行から `elapsed_ms=` の値を取る。
fn elapsed_of(line: &str) -> u128 {
    let field = line
        .split_whitespace()
        .find_map(|part| part.strip_prefix("elapsed_ms="))
        .unwrap_or_else(|| panic!("`elapsed_ms=` が無い: {line}"));
    field.parse().unwrap_or_else(|_| panic!("数でない: {line}"))
}

#[tokio::test]
async fn the_tool_line_carries_the_time_of_that_tool_alone() {
    let dir = TempDir::new();
    let log_path = dir.0.join("fuseforks.log");
    fuseforks_core::open_log(&log_path).expect("ログを開けること");

    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(Arc::new(SlowThenFast))),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig {
            schedule_interval: Duration::from_secs(3600),
            ..OrchestratorConfig::default()
        },
    )
    .await
    .expect("bootstrap できること");
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "default", "mock-model"))
        .await
        .unwrap();
    orchestrator.register_tool(Arc::new(SlowTool)).await;
    orchestrator.register_tool(Arc::new(FastTool)).await;

    let agent = AgentId::from("agent_elapsed");
    orchestrator.create_agent(AgentSpec::new(agent.clone(), "Timer", "tpl")).await.unwrap();
    orchestrator.start_agent(&agent).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&agent, "time the tools").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let body = std::fs::read_to_string(&log_path).expect("ログが読めること");
    let line = |needle: &str| {
        body.lines()
            .find(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("`{needle}` の行が出ること:\n{body}"))
            .to_owned()
    };
    let slow = line("tool: agent=agent_elapsed round=1 name=slow ");
    let fast = line("tool: agent=agent_elapsed round=1 name=fast ");

    // 欄の位置は `reason_chars=` の直後（既存の欄の並びは変えない）。
    assert!(slow.ends_with(&format!(" elapsed_ms={}", elapsed_of(&slow))), "末尾の欄: {slow}");
    assert!(slow.contains(" reason_chars=0 elapsed_ms="), "reason_chars の直後: {slow}");

    let slow_ms = elapsed_of(&slow);
    let fast_ms = elapsed_of(&fast);
    assert!(slow_ms >= 150, "眠った 150 ms がそのツールの行に乗る: {slow}");
    // 即答のツールは前のツールの 150 ms を持ち越さない（ターンや周の頭から測る実装なら乗る）。
    // 負荷の高い CI でも即答のツールが 150 ms かかることは無いので、閾値は眠った時間と同じ値にした。
    assert!(fast_ms < 150, "即答のツールの行には前のツールの時間が乗らない: {fast}");
}

async fn drain_until_quiet(rx: &mut tokio::sync::broadcast::Receiver<CoreEvent>, quiet: Duration) {
    while let Ok(Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) =
        tokio::time::timeout(quiet, rx.recv()).await
    {}
}
