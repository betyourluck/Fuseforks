//! **再試行の待ちの最中に打ち切られたターンは、失敗ではなく打ち切りとして終わる**
//! （Spec 52 D4 と `token_budget.precedence` = cancel が最優先）。
//!
//! # 起点は実機
//!
//! 2026-09-08 01:21:13 — 60 秒の `Retry-After` 待ちに入っていた個体で「■ 停止」を押すと、
//! 待ちは **2 ms** で切れた（機構は効いた）が、ターンは `stop=failed:LLM_API` と
//! 「API エラー (status=429)」の System 行で終わった。`chat_cancellable` は切れると
//! **いま受けた失敗をそのまま返す**設計で、`turn.rs` の `Err` の腕は周回境界の
//! `is_cancelled()` を通らずに `Err` として抜けていた。人が止めたターンが失敗を名乗る形。
//!
//! # スタブの作り
//!
//! `chat_cancellable` で token が切れるまで待ち、切れたら `HttpLlmBackend` と同じ形で
//! `Err(429)` を返すバックエンド。HTTP は立てない — 見たいのは `turn.rs` の分類で、
//! `HttpLlmBackend` 側の `select!` は `tests/retry_hint_log.rs` が留めている。
//!
//! 診断の出口はプロセスで 1 つ（`OnceLock`）なので、ログを読むテストはこのファイルに 1 つ。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use fuseforks_core::event::CoreEvent;
use fuseforks_core::llm::{ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, Usage};
use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};
use tokio_util::sync::CancellationToken;

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-interrupt-retry-wait-{}",
            std::process::id()
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

/// 待ちの最中に切られた `HttpLlmBackend` と同じ返り方をするスタブ。
struct WaitsForCancel {
    calls: AtomicUsize,
    /// `chat_cancellable` に token が渡ってきたか（ターンループの配線の証拠）。
    got_token: AtomicBool,
    /// 呼び出しが始まったことを test 側へ知らせる。
    started: tokio::sync::Notify,
}

#[async_trait]
impl LlmBackend for WaitsForCancel {
    fn name(&self) -> &str {
        "waits-for-cancel"
    }

    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, LlmError> {
        // ターンループはこちらを呼ばないはず。呼ばれたら配線が外れている。
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ChatResponse {
            text: Some("token なしで呼ばれた".into()),
            tool_calls: Vec::new(),
            finish: Finish::Stop,
            usage: Usage {
                prompt: 1,
                completion: 1,
                cache_read: 0,
                cache_write: 0,
                cache_write_1h: 0,
                reasoning: 0,
            },
            grounding: Default::default(),
            reasoning_summary: Vec::new(),
        })
    }

    async fn chat_cancellable(
        &self,
        _req: ChatRequest,
        cancel: Option<CancellationToken>,
    ) -> Result<ChatResponse, LlmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        let Some(token) = cancel else {
            return Err(LlmError::api(500, "token が渡っていない"));
        };
        self.got_token.store(true, Ordering::SeqCst);
        // 「60 秒の Retry-After 待ちの最中」— 切れるまで返らない。
        token.cancelled().await;
        // `HttpLlmBackend::chat_with_backoff` と同じ: いま受けた失敗をそのまま返す。
        Err(LlmError::api(429, r#"{"error":{"status":"RESOURCE_EXHAUSTED"}}"#))
    }
}

async fn drain(
    rx: &mut tokio::sync::broadcast::Receiver<CoreEvent>,
    quiet: Duration,
) -> Vec<CoreEvent> {
    let mut events = Vec::new();
    loop {
        match tokio::time::timeout(quiet, rx.recv()).await {
            Ok(Ok(event)) => events.push(event),
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            _ => return events,
        }
    }
}

#[tokio::test]
async fn an_interrupt_during_the_retry_wait_ends_the_turn_as_interrupted_not_failed() {
    let dir = TempDir::new();
    let log_path = dir.0.join("fuseforks.log");
    fuseforks_core::open_log(&log_path).expect("ログを開けること");

    let backend = Arc::new(WaitsForCancel {
        calls: AtomicUsize::new(0),
        got_token: AtomicBool::new(false),
        started: tokio::sync::Notify::new(),
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
        .create_agent(AgentSpec::new(id.clone(), "ザリ", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&id, "調べて")
        .await
        .unwrap();

    // 呼び出しが「待ち」に入った = 飛行中。ここで停止を押す。
    backend.started.notified().await;
    assert!(
        backend.got_token.load(Ordering::SeqCst),
        "ターンループは token を渡している（D4 の配線）"
    );
    let pressed = std::time::Instant::now();
    orchestrator.interrupt_turn(&id).await;

    let events = drain(&mut rx, Duration::from_millis(400)).await;
    let cut = pressed.elapsed();

    assert_eq!(backend.calls.load(Ordering::SeqCst), 1, "再送していない");
    let interrupted = events
        .iter()
        .filter(|e| matches!(e, CoreEvent::TurnInterrupted { .. }))
        .count();
    assert_eq!(interrupted, 1, "TurnInterrupted は 1 本: {events:?}");
    assert!(
        !events.iter().any(|e| matches!(e, CoreEvent::AgentFailed { .. })),
        "打ち切りは失敗ではない: {events:?}"
    );
    assert!(cut < Duration::from_secs(1), "1 秒以内に切れる: {cut:?}");

    // 打ち切りの出口は `turn interrupted:` の行で閉じる（4 出口の 1 つ。`turn:` の
    // `stop=failed:LLM_API` は出ない）。実機 2026-09-08 01:21:13 の形は
    // `turn: … stop=failed:LLM_API` + `turn failed:` だった — その 2 行が無いことが本体。
    let log = std::fs::read_to_string(&log_path).expect("読めること");
    assert!(
        log.contains("turn interrupted: agent=agent_01"),
        "打ち切りとして閉じる:\n{log}"
    );
    assert!(
        !log.contains("stop=failed:") && !log.contains("turn failed: agent=agent_01"),
        "人が止めたターンが失敗を名乗らない:\n{log}"
    );
}
