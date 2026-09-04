//! **コンテキスト使用率の輪の分子**（Spec 49 D1）— `AgentSnapshot.last_prompt_tokens` は
//! **直近の LLM 呼び出し 1 回ぶん**の入力で、ターンの合計でも累計でもない。
//!
//! 罠は `TurnSpend.prompt` / `TurnRecord.prompt` がターン内の全周の**和**であること —
//! 分子に使うと 6 周のターンで窓の何倍にもなる。ここで留めるのは 3 点:
//!
//! 1. 2 周のターンで、記録されるのは**最後の周**の `prompt`（`cache_read` 込み）で、
//!    2 周の和ではない。累計 `prompt_tokens` のほうは和になる（対照）
//! 2. `usage` が 1 度も返らない失敗（`LlmError::Config` = 鍵の不備）では**前回値が残る**
//!    （0 や `None` で上書きして輪が消えない — rev2 査読 B-3）
//! 3. 払ったと分かる失敗（`OutputTruncated`）は成功と同じ入口で記録される（#103 の棚）

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fuseforks_core::event::CoreEvent;
use fuseforks_core::llm::{ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, ToolCall, Usage};
use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};
use tokio::sync::broadcast::Receiver;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-ctxring-{tag}-{}",
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

fn usage(prompt: u64, cache_read: u64) -> Usage {
    Usage {
        prompt,
        completion: 5,
        cache_read,
        cache_write: 0,
        cache_write_1h: 0,
        reasoning: 0,
    }
}

fn stop(prompt: u64, cache_read: u64) -> ChatResponse {
    ChatResponse {
        text: Some("答えです".into()),
        tool_calls: Vec::new(),
        finish: Finish::Stop,
        usage: usage(prompt, cache_read),
        grounding: Default::default(),
        reasoning_summary: Vec::new(),
    }
}

/// 提示されていない道具を呼ぶ。orchestrator は `tool_result` で断って次の周へ進む
/// （L3。呼び出しが 1 周増える最も安い形）。
fn call_unknown(prompt: u64) -> ChatResponse {
    ChatResponse {
        text: Some(String::new()),
        tool_calls: vec![ToolCall {
            id: "call_1".into(),
            name: "no_such_tool".into(),
            args: serde_json::json!({}),
            extra: None,
        }],
        finish: Finish::ToolUse,
        usage: usage(prompt, 0),
        grounding: Default::default(),
        reasoning_summary: Vec::new(),
    }
}

/// 台本どおりに 1 呼び出しずつ返す。台本が尽きたら `Config` の失敗。
struct Scripted {
    script: Mutex<Vec<Result<ChatResponse, LlmError>>>,
}

impl Scripted {
    fn new(script: Vec<Result<ChatResponse, LlmError>>) -> Self {
        let mut script = script;
        script.reverse();
        Self {
            script: Mutex::new(script),
        }
    }
}

#[async_trait::async_trait]
impl LlmBackend for Scripted {
    fn name(&self) -> &str {
        "scripted"
    }

    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.script
            .lock()
            .unwrap()
            .pop()
            .unwrap_or_else(|| Err(LlmError::Config("台本が尽きた".into())))
    }
}

async fn boot(dir: &TempDir, backend: Arc<dyn LlmBackend>) -> (Orchestrator, AgentId) {
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
    orchestrator
        .set_language(fuseforks_core::world::Language::Ja)
        .await
        .unwrap();
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();
    let id = AgentId::from("agent_01");
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "ジェミー", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();
    (orchestrator, id)
}

/// 静穏窓は統計の周期（1 秒）より**短く**取る（#86 — 長いと永久に返らない）。
async fn drain_until_quiet(rx: &mut Receiver<CoreEvent>, quiet: Duration) {
    while let Ok(Ok(_)) = tokio::time::timeout(quiet, rx.recv()).await {}
}

async fn ask(orchestrator: &Orchestrator, id: &AgentId, text: &str) {
    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(id, text).await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
}

#[tokio::test]
async fn last_prompt_is_the_final_round_not_the_turn_sum() {
    let dir = TempDir::new("final-round");
    let backend = Arc::new(Scripted::new(vec![
        Ok(call_unknown(100)),
        Ok(stop(250, 200)),
    ]));
    let (orchestrator, id) = boot(&dir, backend).await;

    let fresh = orchestrator.snapshot(&id).await.unwrap();
    assert_eq!(fresh.last_prompt_tokens, None, "1 度も呼んでいなければ None（0 ではない）");

    ask(&orchestrator, &id, "調べて").await;

    let after = orchestrator.snapshot(&id).await.unwrap();
    assert_eq!(
        after.last_prompt_tokens,
        Some(250),
        "最後の周の prompt（cache_read 200 込み）。2 周の和 350 ではない"
    );
    assert_eq!(after.prompt_tokens, 350, "対照: 累計のほうは 2 周の和");

    let _ = orchestrator.stop_agent(&id).await;
}

#[tokio::test]
async fn a_turn_without_usage_keeps_the_previous_value() {
    let dir = TempDir::new("no-usage");
    let backend = Arc::new(Scripted::new(vec![
        Ok(stop(250, 0)),
        // 2 ターン目: 鍵の不備 = usage が 1 度も返らない失敗
        Err(LlmError::Config("鍵が無い".into())),
        // 3 ターン目: 払ったと分かる失敗（切れた応答）
        Err(LlmError::OutputTruncated {
            limit: 10,
            usage: usage(777, 0),
        }),
    ]));
    let (orchestrator, id) = boot(&dir, backend).await;

    ask(&orchestrator, &id, "1 本目").await;
    assert_eq!(orchestrator.snapshot(&id).await.unwrap().last_prompt_tokens, Some(250));

    ask(&orchestrator, &id, "2 本目（失敗）").await;
    let after_failure = orchestrator.snapshot(&id).await.unwrap();
    assert_eq!(
        after_failure.last_prompt_tokens,
        Some(250),
        "usage の無い失敗では前回値が残る（輪が消えない）"
    );
    assert_eq!(after_failure.prompt_tokens, 250, "対照: 累計も増えていない（払いが不明）");

    // `Config` の失敗は fatal で個体が落ちる（`failed` は `start_agent` で戻せる —
    // 2026-08-06 の裁定）。記録は個体の再起動で消えない。
    orchestrator.start_agent(&id).await.unwrap();
    assert_eq!(
        orchestrator.snapshot(&id).await.unwrap().last_prompt_tokens,
        Some(250),
        "再起動しても直近の値は残る（消えるのはアプリの再起動だけ — D6）"
    );

    ask(&orchestrator, &id, "3 本目（切れた）").await;
    assert_eq!(
        orchestrator.snapshot(&id).await.unwrap().last_prompt_tokens,
        Some(777),
        "払ったと分かる失敗（OutputTruncated）は成功と同じ入口で記録される"
    );

    let _ = orchestrator.stop_agent(&id).await;
}
