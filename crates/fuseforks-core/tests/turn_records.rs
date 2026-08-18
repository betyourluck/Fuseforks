//! **ターンの 4 出口すべてが `Record::Turn` を 1 件ずつ書く**ことを機械で留める
//! （Spec 39 P1）。`stop` の 5 値（完走 / 失敗 / 割り込み / 予算切れ / 見積もり不足）
//! で 1 本ずつ — 5 本が**別々の出口**を守っている（残る 2 値 `repeat` / `tool_limit` は
//! 完走と同じ関数の同じ行で、`stop` の値だけが違う）。
//!
//! 読み口は **`export_session` の JSONL**（`{"seq":N,"kind":"turn",...}`）。redb は
//! バイナリなので、人が読める出口が機構の一部（Spec 12）— ここでそれを使うことで、
//! 「JSONL に `kind: "turn"` が出る」も同時に留まる。
//!
//! ミューテーションで赤を確かめる（P1 の Tasks）:
//! - `settle_turn` からレコードの書き込みを外す → 5 本とも赤
//! - 失敗の出口だけ `settle_turn` を通さない → 失敗の 1 本だけ赤
//! - `budget_stop_reason` を `BudgetExhausted` 固定にする → `reserve_short` の 1 本だけ赤

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use fuseforks_core::event::CoreEvent;
use fuseforks_core::llm::{
    ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, ToolCall, Usage,
};
use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::{
    AgentTool, ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator,
    OrchestratorConfig, ToolContext,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-turn-records-{tag}-{}",
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

/// テスト用の即答ツール。ループするバックエンドが呼び続ける相手。
struct BusyTool;

#[async_trait::async_trait]
impl AgentTool for BusyTool {
    fn name(&self) -> &str {
        "busy_probe"
    }
    fn description(&self, _language: fuseforks_core::world::Language) -> String {
        "テスト用の即答ツール".into()
    }
    fn parameters(&self, _language: fuseforks_core::world::Language) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    async fn call(&self, _ctx: &ToolContext, _args: &serde_json::Value) -> fuseforks_core::CoreResult<String> {
        Ok("ok".into())
    }
}

fn response(text: &str, usage: Usage) -> ChatResponse {
    ChatResponse {
        text: Some(text.to_owned()),
        tool_calls: Vec::new(),
        finish: Finish::Stop,
        usage,
        grounding: Default::default(),
        reasoning_summary: Vec::new(),
    }
}

/// 毎回違う引数で `busy_probe` を呼び続ける（RepeatGuard に掛けない）。
/// `delay` は 1 周目の応答を遅らせる時間（割り込みの窓を作るため）。
struct LoopingBackend {
    usage: Usage,
    delay: Duration,
    calls: AtomicU32,
}

#[async_trait::async_trait]
impl LlmBackend for LoopingBackend {
    fn name(&self) -> &str {
        "looping"
    }
    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if n == 1 && !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        Ok(ChatResponse {
            text: Some(String::new()),
            tool_calls: vec![ToolCall {
                id: format!("call_{n}"),
                name: "busy_probe".into(),
                args: serde_json::json!({ "round": n }),
                extra: None,
            }],
            finish: Finish::ToolUse,
            usage: self.usage,
            grounding: Default::default(),
            reasoning_summary: Vec::new(),
        })
    }
}

/// 1 回で答えるバックエンド。
struct PlainBackend(Usage);

#[async_trait::async_trait]
impl LlmBackend for PlainBackend {
    fn name(&self) -> &str {
        "plain"
    }
    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, LlmError> {
        Ok(response("済みました", self.0))
    }
}

/// 払ったうえで出力上限に当たる（`OutputTruncated` に usage を載せて返す — #103 の経路）。
struct TruncatedBackend(Usage);

#[async_trait::async_trait]
impl LlmBackend for TruncatedBackend {
    fn name(&self) -> &str {
        "truncated"
    }
    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, LlmError> {
        Err(LlmError::OutputTruncated {
            limit: 64,
            usage: self.0,
        })
    }
}

async fn setup(dir: &TempDir, backend: Arc<dyn LlmBackend>, ceiling: Option<u64>) -> Orchestrator {
    if let Some(ceiling) = ceiling {
        std::fs::write(
            dir.0.join("world.json"),
            format!(r#"{{ "tokenBudget": {ceiling} }}"#),
        )
        .unwrap();
    }
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
    // ホストの OS ロケールに依存させない（CI は en・開発機は ja）。
    orchestrator
        .set_language(fuseforks_core::world::Language::Ja)
        .await
        .unwrap();
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();
    orchestrator.register_tool(Arc::new(BusyTool)).await;
    let id = AgentId::from("agent_01");
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "ザリ", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();
    orchestrator
}

async fn drain(rx: &mut tokio::sync::broadcast::Receiver<CoreEvent>, quiet: Duration) -> Vec<CoreEvent> {
    let mut events = Vec::new();
    while let Ok(Ok(event)) = tokio::time::timeout(quiet, rx.recv()).await {
        events.push(event);
    }
    events
}

/// 今の会話を JSONL へ書き出し、`kind: "turn"` の行だけを返す。
async fn turn_records(orchestrator: &Orchestrator, dir: &TempDir) -> Vec<serde_json::Value> {
    let out = dir.0.join("export.jsonl");
    let session = orchestrator.current_session();
    orchestrator.export_session(&session, &out).await.expect("書き出せること");
    let body = std::fs::read_to_string(&out).unwrap();
    body.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|v| v.get("kind").and_then(|k| k.as_str()) == Some("turn"))
        .collect()
}

fn stop_kind(record: &serde_json::Value) -> &str {
    record["stop"]["kind"].as_str().unwrap_or("?")
}

/// 完走: 1 件・全欄・`TurnRecorded` が 1 通（id だけを運ぶ）。
#[tokio::test]
async fn a_completed_turn_writes_one_turn_record_and_one_event() {
    let dir = TempDir::new("completed");
    let usage = Usage { prompt: 10, completion: 4, cache_read: 3, cache_write: 0, cache_write_1h: 0, reasoning: 1 };
    let orchestrator = setup(&dir, Arc::new(PlainBackend(usage)), None).await;
    let id = AgentId::from("agent_01");

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "こんにちは").await.unwrap();
    let events = drain(&mut rx, Duration::from_millis(400)).await;

    let records = turn_records(&orchestrator, &dir).await;
    assert_eq!(records.len(), 1, "完走のターンは Turn を 1 件書く: {records:#?}");
    let r = &records[0];
    assert_eq!(stop_kind(r), "completed");
    assert_eq!(r["agentId"], "agent_01");
    assert_eq!(r["hop"], 0);
    assert_eq!(r["rounds"], 1);
    assert_eq!(r["waves"], 0);
    assert_eq!(r["prompt"], 10);
    assert_eq!(r["cached"], 3);
    // completion は書く側が total − prompt を 1 箇所で引く（Spec 39 D1）。
    assert_eq!(r["completion"], 4, "completion = (10 + 4) − 10");
    assert_eq!(r["reasoning"], 1);
    assert_eq!(r["model"], "mock-model", "テンプレートのモデル名（Option ではない）");
    assert_eq!(r["backend"], "plain", "ワイヤ名");
    assert!(r["tsMs"].as_u64().unwrap_or(0) > 0, "開始の壁時計: {r}");
    assert!(r["elapsedMs"].is_u64(), "経過: {r}");

    let recorded: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            CoreEvent::TurnRecorded { agent_id, session_id } => Some((agent_id, session_id)),
            _ => None,
        })
        .collect();
    assert_eq!(recorded.len(), 1, "TurnRecorded は 1 通: {events:#?}");
    assert_eq!(*recorded[0].0, id);
    assert_eq!(*recorded[0].1, orchestrator.current_session(), "保存先の会話 id を運ぶ");
}

/// 失敗（`Err` の出口）: `stop = failed{code}`、払った量が全欄に入る。
#[tokio::test]
async fn a_failed_turn_writes_a_turn_record_with_the_error_code() {
    let dir = TempDir::new("failed");
    let usage = Usage { prompt: 2_000, completion: 125, cache_read: 0, cache_write: 0, cache_write_1h: 0, reasoning: 125 };
    let orchestrator = setup(&dir, Arc::new(TruncatedBackend(usage)), None).await;
    let id = AgentId::from("agent_01");

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "調べて").await.unwrap();
    let _ = drain(&mut rx, Duration::from_millis(400)).await;

    let records = turn_records(&orchestrator, &dir).await;
    assert_eq!(records.len(), 1, "失敗のターンも Turn を 1 件書く（#103 の延長）: {records:#?}");
    let r = &records[0];
    assert_eq!(stop_kind(r), "failed");
    assert_eq!(r["stop"]["code"], "LLM_OUTPUT_TRUNCATED");
    assert_eq!(r["prompt"], 2_000);
    assert_eq!(r["completion"], 125, "completion = 2,125 − 2,000");
    assert_eq!(r["reasoning"], 125);
    assert_eq!(r["rounds"], 1, "払ったと分かる失敗の周も数える");
    assert_eq!(r["waves"], 0, "失敗経路では波を数えない（契約どおり 0）");
    assert_eq!(r["backend"], "truncated");
}

/// 割り込み: `stop = interrupted`。周回境界で切られた 1 周ぶんの払いが入る。
#[tokio::test]
async fn an_interrupted_turn_writes_a_turn_record() {
    let dir = TempDir::new("interrupted");
    let usage = Usage { prompt: 7, completion: 2, cache_read: 0, cache_write: 0, cache_write_1h: 0, reasoning: 0 };
    let backend = Arc::new(LoopingBackend {
        usage,
        delay: Duration::from_millis(300),
        calls: AtomicU32::new(0),
    });
    let orchestrator = setup(&dir, backend.clone(), None).await;
    let id = AgentId::from("agent_01");

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "ずっと調べ続けて").await.unwrap();
    // 1 周目の呼び出しが飛行中（300 ms 眠っている）のあいだに割り込む。
    tokio::time::sleep(Duration::from_millis(80)).await;
    orchestrator.interrupt_turn(&id).await;
    let events = drain(&mut rx, Duration::from_millis(500)).await;

    assert!(
        events.iter().any(|e| matches!(e, CoreEvent::TurnInterrupted { .. })),
        "対照: 割り込みが実際に起きたこと: {events:#?}"
    );
    let records = turn_records(&orchestrator, &dir).await;
    assert_eq!(records.len(), 1, "割り込みのターンも Turn を 1 件書く: {records:#?}");
    let r = &records[0];
    assert_eq!(stop_kind(r), "interrupted");
    assert_eq!(r["rounds"], 1, "飛行中の 1 呼び出しは完走させてから切る");
    assert_eq!(r["prompt"], 7);
    assert_eq!(r["completion"], 2);
    assert_eq!(r["backend"], "looping");
}

/// 予算切れ（残額 0）: `stop = budget_exhausted`。
///
/// 天井 1,000 実効。1 周目は床 1,000 で予約が通り、実費 5,000（未キャッシュ ×1）で
/// 残額は 0 に飽和。2 周目の予約が `exhausted` で落ちる。
#[tokio::test]
async fn a_budget_exhausted_turn_writes_a_turn_record() {
    let dir = TempDir::new("exhausted");
    let usage = Usage { prompt: 5_000, completion: 0, cache_read: 0, cache_write: 0, cache_write_1h: 0, reasoning: 0 };
    let backend = Arc::new(LoopingBackend {
        usage,
        delay: Duration::ZERO,
        calls: AtomicU32::new(0),
    });
    let orchestrator = setup(&dir, backend, Some(1_000)).await;
    let id = AgentId::from("agent_01");

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "調べ続けて").await.unwrap();
    let _ = drain(&mut rx, Duration::from_millis(400)).await;

    let records = turn_records(&orchestrator, &dir).await;
    assert_eq!(records.len(), 1, "予算の出口も Turn を 1 件書く: {records:#?}");
    let r = &records[0];
    assert_eq!(stop_kind(r), "budget_exhausted", "残額 0 は exhausted: {r}");
    assert_eq!(r["rounds"], 1);
    assert_eq!(r["prompt"], 5_000);
}

/// 見積もり不足（残額 > 0 だが次の 1 呼び出しぶんに届かない）: `stop = reserve_short`。
///
/// 天井 3,000 実効。1 周目は床 1,000 で予約が通り、実費 2,400 で残額 600。
/// 2 周目の見積もりは max(直前実測 2,400, 床 1,000) = 2,400 > 600 で `reserve_short`。
/// **`budget_exhausted` と同じ関数の同じ行から出る**ので、理由を引数で運ぶ変更が
/// 無いとこの 1 本だけが赤になる（ミューテーション 3）。
#[tokio::test]
async fn a_reserve_short_turn_writes_a_turn_record() {
    let dir = TempDir::new("reserve-short");
    let usage = Usage { prompt: 2_400, completion: 0, cache_read: 0, cache_write: 0, cache_write_1h: 0, reasoning: 0 };
    let backend = Arc::new(LoopingBackend {
        usage,
        delay: Duration::ZERO,
        calls: AtomicU32::new(0),
    });
    let orchestrator = setup(&dir, backend, Some(3_000)).await;
    let id = AgentId::from("agent_01");

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "調べ続けて").await.unwrap();
    let _ = drain(&mut rx, Duration::from_millis(400)).await;

    let records = turn_records(&orchestrator, &dir).await;
    assert_eq!(records.len(), 1, "見積もり不足の出口も Turn を 1 件書く: {records:#?}");
    let r = &records[0];
    assert_eq!(stop_kind(r), "reserve_short", "残額 600 > 0 は reserve_short: {r}");
    assert_eq!(r["rounds"], 1);
    assert_eq!(r["prompt"], 2_400);
}

/// `session_stats`（集める → `aggregate` の 2 段）が保存した `Turn` から数字を出す。
/// `session` と `all` の両スコープ、存在しない会話は `SESSION_NOT_FOUND`。
#[tokio::test]
async fn session_stats_reads_the_saved_turns_in_both_scopes() {
    use fuseforks_core::stats::StatsScope;

    let dir = TempDir::new("stats");
    let usage = Usage { prompt: 1_000, completion: 50, cache_read: 200, cache_write: 0, cache_write_1h: 0, reasoning: 0 };
    let orchestrator = setup(&dir, Arc::new(PlainBackend(usage)), None).await;
    let id = AgentId::from("agent_01");

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "こんにちは").await.unwrap();
    orchestrator.send_user_message(&id, "もう一度").await.unwrap();
    let _ = drain(&mut rx, Duration::from_millis(400)).await;

    let session = orchestrator.current_session();
    let report = orchestrator
        .session_stats(StatsScope::Session { session_id: session.clone() })
        .await
        .expect("集計できること");
    assert_eq!(report.totals.turns, 2);
    // (1000 − 200) ×1 + 200 ×0.1 + 50 ×4 = 1,020 × 2 ターン
    assert_eq!(report.totals.effective, 2_040, "実効は budget.rs の重み");
    assert!(report.scope_meta.recorded_since.is_some(), "記録がある会話");
    assert_eq!(report.scope_meta.sessions.len(), 1);
    assert_eq!(report.scope_meta.sessions[0].session_id, session);
    assert_eq!(report.by_agent.len(), 1);
    assert_eq!(report.by_agent[0].model, "mock-model");
    assert_eq!(report.series.as_ref().map(|s| s.points.len()), Some(2));

    let all = orchestrator.session_stats(StatsScope::All).await.unwrap();
    assert_eq!(all.totals.turns, 2);
    assert!(all.series.is_none(), "all では series を出さない");
    assert!(
        all.scope_meta.sessions.iter().any(|s| s.session_id == session && s.turns == 2),
        "会話ごとの合計表に今の会話が居る: {:#?}",
        all.scope_meta.sessions
    );

    let missing = orchestrator
        .session_stats(StatsScope::Session { session_id: "no-such".into() })
        .await;
    assert!(
        matches!(missing, Err(fuseforks_core::CoreError::SessionNotFound(_))),
        "存在しない会話は名指しで失敗: {missing:?}"
    );
}
