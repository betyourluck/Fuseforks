//! **`RepeatGuard` が止めなかった繰り返しが `turn:` 行に出る**ことを機械で留める
//! （2026-09-22 の計器）。
//!
//! `RepeatGuard` は「同じ引数 + **同じ結果**」が 2 回続いたときだけ 3 回目を止める
//! （`failures.md` #41 の処方 1）。だから `curl` のように**出力が毎回変わる**呼び出しは、
//! 同じ場所を何周回っても止まらないし、どの計器にも出なかった。
//!
//! 実測（`fuseforks.log` 2026-08-09〜09-22・1,445 ターン）で `rounds 10+` は 106 本
//! （7.3%・prompt の 30.6%）あり、そのうち同じ引数を 3 回以上呼んだのは 14 本。
//! **その 14 本を名指しするのがこの欄の目的で、何も止めない。**
//!
//! **負の対照を同じログで取る**（`failures.md` #90 の処方）— 進行役は `ask_*` を
//! 1 回呼ぶだけなので `repeat_max=1` になる。これを見ないと「常に 4 を出す実装」でも
//! 「ツールの本数をそのまま出す実装」でも緑になる。
//!
//! **診断の出口はプロセスで 1 つ**（`OnceLock`）なので、ログを読むテストは
//! このファイルに 1 つだけ（`tests/truncated_note.rs` / `failed_turn_settlement.rs`
//! と同じ理由で別バイナリに分けてある）。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use fuseforks_core::event::CoreEvent;
use fuseforks_core::llm::{
    ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, Role, ToolCall, Usage,
};
use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::tool::{AgentTool, ToolContext};
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};

/// ワーカーが同じ引数で呼ぶ回数。`RepeatGuard` の上限（2 回続いたら 3 回目を止める）
/// より**大きく**しておかないと、「止めなかった」ことが読めない。
const DRIFT_CALLS: usize = 4;

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("fuseforks-repeat-max-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 呼ばれるたびに**違う本文**を返すツール（`curl` / `git log` の形）。
///
/// `RepeatGuard` は本文の完全一致で数えるので、これは何回呼んでも止まらない。
#[derive(Default)]
struct DriftingTool {
    runs: AtomicUsize,
}

#[async_trait::async_trait]
impl AgentTool for DriftingTool {
    fn name(&self) -> &str {
        "drift_probe"
    }
    fn description(&self, _language: fuseforks_core::world::Language) -> String {
        "テスト用。呼ぶたびに違う本文を返す".into()
    }
    fn parameters(&self, _language: fuseforks_core::world::Language) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    async fn call(
        &self,
        _ctx: &ToolContext,
        _args: &serde_json::Value,
    ) -> fuseforks_core::CoreResult<String> {
        let n = self.runs.fetch_add(1, Ordering::SeqCst) + 1;
        // 毎回違う = RepeatGuard の `count` は 1 のまま。
        Ok(format!("200 / {n}ms"))
    }
}

/// 進行役は 1 度だけ委譲する。ワーカーは同じ引数で `drift_probe` を
/// [`DRIFT_CALLS`] 回呼んでから答える。
struct AskThenDrift;

#[async_trait::async_trait]
impl LlmBackend for AskThenDrift {
    fn name(&self) -> &str {
        "ask-then-drift"
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
        let done = |text: &str| ChatResponse {
            text: Some(text.to_owned()),
            tool_calls: Vec::new(),
            finish: Finish::Stop,
            usage,
            grounding: Default::default(),
            reasoning_summary: Vec::new(),
        };

        // 進行役（`ask_*` を提示されている側）。
        if let Some(ask) = req.tools.iter().find(|t| t.name.starts_with("ask_")) {
            if req.messages.iter().any(|m| m.role == Role::Tool) {
                return Ok(done("ワーカーの答えを受け取りました"));
            }
            return Ok(ChatResponse {
                text: Some(String::new()),
                tool_calls: vec![ToolCall {
                    id: "ask_1".into(),
                    name: ask.name.clone(),
                    args: serde_json::json!({ "message": "調べて" }),
                    extra: None,
                }],
                finish: Finish::ToolUse,
                usage,
                grounding: Default::default(),
                reasoning_summary: Vec::new(),
            });
        }

        // ワーカー。**引数は毎回同じ**で、結果だけが変わる。
        let calls = req.messages.iter().filter(|m| m.role == Role::Tool).count();
        if calls >= DRIFT_CALLS {
            return Ok(done("4 回叩きましたが同じ場所でした"));
        }
        Ok(ChatResponse {
            text: Some(String::new()),
            tool_calls: vec![ToolCall {
                id: format!("drift_{calls}"),
                name: "drift_probe".into(),
                args: serde_json::json!({ "url": "https://example.com" }),
                extra: None,
            }],
            finish: Finish::ToolUse,
            usage,
            grounding: Default::default(),
            reasoning_summary: Vec::new(),
        })
    }
}

#[tokio::test]
async fn a_repeat_the_guard_did_not_block_is_counted_in_the_turn_line() {
    let dir = TempDir::new();
    let log_path = dir.0.join("fuseforks.log");
    fuseforks_core::open_log(&log_path).expect("ログを開けること");

    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(Arc::new(AskThenDrift))),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig {
            schedule_interval: Duration::from_secs(3600),
            ..OrchestratorConfig::default()
        },
    )
    .await
    .expect("bootstrap できること");
    // ホストの OS ロケールに依存させない（CI は en・開発機は ja。#101）。
    orchestrator
        .set_language(fuseforks_core::world::Language::Ja)
        .await
        .unwrap();
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();

    let tool = Arc::new(DriftingTool::default());
    orchestrator.register_tool(tool.clone()).await;

    let lead = AgentId::from("agent_lead");
    let worker = AgentId::from("agent_w1");
    orchestrator
        .create_agent(AgentSpec::new(worker.clone(), "ワーカー", "tpl"))
        .await
        .unwrap();
    let mut spec = AgentSpec::new(lead.clone(), "進行役", "tpl");
    spec.connected_agents = vec![worker.clone()];
    orchestrator.create_agent(spec).await.unwrap();
    orchestrator.start_agent(&worker).await.unwrap();
    orchestrator.start_agent(&lead).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&lead, "調べて")
        .await
        .unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    // 1. 止まっていない。**ここが前提** — 止まっていたら数える対象が無い。
    assert_eq!(
        tool.runs.load(Ordering::SeqCst),
        DRIFT_CALLS,
        "結果が毎回変わる呼び出しは RepeatGuard に止められないこと"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, CoreEvent::ToolRepeatBlocked { .. })),
        "止めていないこと（止めたなら計器ではなく機構の話になる）: {events:?}"
    );

    let body = std::fs::read_to_string(&log_path).expect("ログが読めること");
    let line_of = |agent: &str| {
        body.lines()
            .find(|l| l.contains(&format!("turn: agent={agent} ")))
            .unwrap_or_else(|| panic!("`turn: agent={agent}` の行が出ること:\n{body}"))
            .to_owned()
    };

    // 2. ワーカーの行に、止めなかった繰り返しの回数が出る。
    let w = line_of("agent_w1");
    assert!(
        w.contains(&format!("repeat_max={DRIFT_CALLS}")),
        "同じ引数を呼んだ回数が読めること: {w}"
    );

    // 3. **負の対照**。進行役は `ask_*` を 1 回呼ぶだけ。
    //    これが無いと「常に 4 を出す実装」も「ツールの本数を出す実装」も緑になる。
    let l = line_of("agent_lead");
    assert!(
        l.contains("repeat_max=1"),
        "1 回しか呼ばなかったターンは 1 であること: {l}"
    );
}

/// イベントが止むまで集める（`tests/orchestrator.rs` と同じ作法）。
///
/// 窓は**この系で最も短い定期イベントの周期より短く**する（`failures.md` #86 —
/// 統計は 1 秒周期なので、1 秒以上にすると永久に閉じない）。
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
