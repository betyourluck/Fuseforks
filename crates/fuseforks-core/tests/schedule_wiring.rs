//! Spec 07（スケジュール実行）の配線層の結合テスト。
//!
//! 発火規則そのものの検証は `src/schedule.rs` の単体テスト（純関数）が持つ。
//! ここで確かめるのは**副作用の配線**: 配送・記録・消化・保存・回収。
//!
//! tick はすべて手動（[`Orchestrator::run_schedule_tick`]）で、固定時刻を渡す。
//! 実ティッカーは `schedule_interval` を 1 時間に引き延ばして遠ざける —
//! `Local::now()` で並走されると判定が競合する。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fuseforks_core::event::CoreEvent;
use fuseforks_core::model::{AgentId, AgentSpec, Endpoint, ModelTemplate};
use fuseforks_core::schedule::{Recurrence, ScheduleOptions, Weekday};
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};
use tokio::sync::broadcast::Receiver;

/// テスト用の一時ディレクトリ。終了時に破棄する。
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-sched-{tag}-{}",
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

/// 実ティッカーをテストから遠ざけた設定。
fn config() -> OrchestratorConfig {
    OrchestratorConfig {
        schedule_interval: Duration::from_secs(3600),
        ..OrchestratorConfig::default()
    }
}

/// テンプレート 1 件だけ登録済みのオーケストレーターを組む。
async fn setup(dir: &TempDir) -> Orchestrator {
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::echo("[echo]")),
        Arc::new(InMemorySecretStore::new()),
        config(),
    )
    .await
    .expect("bootstrap できること");
    // ホストの OS ロケールに依存させない（CI は en・開発機は ja — Spec 35 で
    // 言語がコアの挙動の入力になった。時計を引数で受けるのと同じ規律）。
    orchestrator.set_language(fuseforks_core::world::Language::Ja).await.unwrap();

    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();

    orchestrator
}

/// 一定時間静かになるまでイベントを集める（tests/orchestrator.rs と同じ流儀）。
async fn drain_until_quiet(rx: &mut Receiver<CoreEvent>, quiet: Duration) -> Vec<CoreEvent> {
    let mut events = Vec::new();
    while let Ok(Ok(event)) = tokio::time::timeout(quiet, rx.recv()).await {
        events.push(event);
    }
    events
}

/// 発話イベントだけを抜き出す。
fn messages(events: &[CoreEvent]) -> Vec<&fuseforks_core::AgentMessage> {
    events
        .iter()
        .filter_map(|e| match e {
            CoreEvent::MessageSent { message } => Some(message),
            _ => None,
        })
        .collect()
}

/// テストで使う固定タイムゾーン（JST）。実行機の設定に依存しない。
fn jst_at(
    y: i32,
    m: u32,
    d: u32,
    h: u32,
    min: u32,
    s: u32,
) -> chrono::DateTime<chrono::FixedOffset> {
    use chrono::TimeZone;
    chrono::FixedOffset::east_opt(9 * 3600)
        .expect("JST は妥当なオフセット")
        .with_ymd_and_hms(y, m, d, h, min, s)
        .single()
        .expect("テストの時刻は一意に決まる")
}

const THU_17: Recurrence = Recurrence::Weekly {
    weekday: Weekday::Thu,
    hour: 17,
    minute: 0,
};

/// 発火 → 由来つきの本文が hop 0 で配送され、消化される。同じ週に二度は飛ばない。
#[tokio::test]
async fn fires_once_with_origin_prefix() {
    let dir = TempDir::new("fire");
    let orchestrator = setup(&dir).await;
    let agent = AgentId::from("agent_01");
    orchestrator
        .create_agent(AgentSpec::new(agent.clone(), "ロボットくん", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&agent).await.unwrap();

    orchestrator
        .create_schedule(agent.clone(), "今の時刻を言って".into(), THU_17, ScheduleOptions::default())
        .await
        .unwrap();

    let mut rx = orchestrator.subscribe();

    // 2026-07-30 は木曜。17:00:29 = tick 30 秒での最初の観測点。
    orchestrator
        .run_schedule_tick(jst_at(2026, 7, 30, 17, 0, 29))
        .await;
    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    let sent = messages(&events);

    let fired: Vec<_> = sent
        .iter()
        .filter(|m| m.content.starts_with("【定期実行: 毎週 木曜 17:00】"))
        .collect();
    assert_eq!(fired.len(), 1, "定期実行の配送が 1 通だけあること: {sent:?}");
    assert_eq!(fired[0].hop, 0, "予定発火は新しい起点なので hop 0");
    assert!(
        matches!(fired[0].from, Endpoint::System),
        "送り手は System であること"
    );
    assert!(
        fired[0].content.contains("今の時刻を言って"),
        "依頼本文が保たれていること"
    );

    // 応答（echo）はユーザーへ返る — 会話ペインが観測点になる。
    assert!(
        sent.iter()
            .any(|m| m.to == Endpoint::User && m.content.contains("[echo]")),
        "最終出力が User へ返ること: {sent:?}"
    );

    // 消化の記録。lastConsumedDueMs は「消化した予定時刻」で 17:00:00 ちょうど。
    let due_ms = jst_at(2026, 7, 30, 17, 0, 0).timestamp_millis() as u64;
    let tasks = orchestrator.schedules().await;
    assert_eq!(tasks[0].last_consumed_due_ms, Some(due_ms));

    // 同じ週の再判定では何も起きない。
    let mut rx = orchestrator.subscribe();
    orchestrator
        .run_schedule_tick(jst_at(2026, 7, 30, 17, 1, 0))
        .await;
    orchestrator
        .run_schedule_tick(jst_at(2026, 7, 31, 9, 0, 0))
        .await;
    let events = drain_until_quiet(&mut rx, Duration::from_millis(300)).await;
    assert!(
        messages(&events).is_empty(),
        "消化済みの週に再配送が無いこと"
    );
}

/// 停止中の宛先へは撒かず、会話ログへ 1 行だけ残して消化する。
#[tokio::test]
async fn skips_stopped_target_with_single_notice() {
    let dir = TempDir::new("stopped");
    let orchestrator = setup(&dir).await;
    let agent = AgentId::from("agent_01");
    orchestrator
        .create_agent(AgentSpec::new(agent.clone(), "ロボットくん", "tpl"))
        .await
        .unwrap();
    // start しない = 停止中。

    orchestrator
        .create_schedule(agent.clone(), "今の時刻を言って".into(), THU_17, ScheduleOptions::default())
        .await
        .unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator
        .run_schedule_tick(jst_at(2026, 7, 30, 17, 0, 29))
        .await;
    let events = drain_until_quiet(&mut rx, Duration::from_millis(300)).await;
    let sent = messages(&events);

    let notices: Vec<_> = sent
        .iter()
        .filter(|m| m.content.contains("飛ばしました（停止中）"))
        .collect();
    assert_eq!(notices.len(), 1, "通知が 1 行だけあること: {sent:?}");
    assert!(
        matches!(notices[0].from, Endpoint::System) && notices[0].to == Endpoint::User,
        "通知は System 発・User 宛であること"
    );
    assert!(
        notices[0].content.contains("agent_01（ロボットくん）"),
        "id と表示名の併記（Spec 06 の規律）: {}",
        notices[0].content
    );

    // 消化済みなので、次の tick で二度目の通知は出ない（毎 tick 積まれない）。
    let mut rx = orchestrator.subscribe();
    orchestrator
        .run_schedule_tick(jst_at(2026, 7, 30, 17, 1, 0))
        .await;
    let events = drain_until_quiet(&mut rx, Duration::from_millis(300)).await;
    assert!(messages(&events).is_empty(), "通知は 1 回だけであること");
}

/// 猶予超過の消化は会話ログへ何も出さない（debug ログのみ）。
#[tokio::test]
async fn grace_expiry_is_silent_in_conversation() {
    let dir = TempDir::new("grace");
    let orchestrator = setup(&dir).await;
    let agent = AgentId::from("agent_01");
    orchestrator
        .create_agent(AgentSpec::new(agent.clone(), "ロボットくん", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&agent).await.unwrap();

    orchestrator
        .create_schedule(
            agent.clone(),
            "点検".into(),
            Recurrence::Daily { hour: 9, minute: 0 },
            ScheduleOptions::default(),
        )
        .await
        .unwrap();

    let mut rx = orchestrator.subscribe();
    // 9:00 の予定を 15:00 に観測 = 猶予 5 分を大きく超過。
    orchestrator
        .run_schedule_tick(jst_at(2026, 7, 30, 15, 0, 0))
        .await;
    let events = drain_until_quiet(&mut rx, Duration::from_millis(300)).await;
    assert!(
        messages(&events).is_empty(),
        "猶予超過は会話ログに何も出さないこと（本物の通知を埋めない）"
    );

    let due_ms = jst_at(2026, 7, 30, 9, 0, 0).timestamp_millis() as u64;
    let tasks = orchestrator.schedules().await;
    assert_eq!(
        tasks[0].last_consumed_due_ms,
        Some(due_ms),
        "発火しないが消化はされること"
    );
}

/// 登録の入口で不正を弾く。
#[tokio::test]
async fn create_rejects_invalid_input() {
    let dir = TempDir::new("validate");
    let orchestrator = setup(&dir).await;
    let agent = AgentId::from("agent_01");
    orchestrator
        .create_agent(AgentSpec::new(agent.clone(), "ロボットくん", "tpl"))
        .await
        .unwrap();

    let err = orchestrator
        .create_schedule(
            agent.clone(),
            "x".into(),
            Recurrence::Daily { hour: 99, minute: 0 },
            ScheduleOptions::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), "INVALID_SCHEDULE");

    let err = orchestrator
        .create_schedule(AgentId::from("ghost"), "x".into(), THU_17, ScheduleOptions::default())
        .await
        .unwrap_err();
    assert_eq!(err.code(), "AGENT_NOT_FOUND", "未登録の宛先は登録時点で弾く");

    let err = orchestrator
        .delete_schedule("no-such-id")
        .await
        .unwrap_err();
    assert_eq!(err.code(), "SCHEDULE_NOT_FOUND");
}

/// enabled=false は発火も消化もしない。再開後は直近の予定から拾える。
#[tokio::test]
async fn disabled_schedule_is_dormant_but_recoverable() {
    let dir = TempDir::new("disabled");
    let orchestrator = setup(&dir).await;
    let agent = AgentId::from("agent_01");
    orchestrator
        .create_agent(AgentSpec::new(agent.clone(), "ロボットくん", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&agent).await.unwrap();

    let task = orchestrator
        .create_schedule(agent.clone(), "点検".into(), THU_17, ScheduleOptions::default())
        .await
        .unwrap();
    orchestrator
        .set_schedule_enabled(&task.id, false)
        .await
        .unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator
        .run_schedule_tick(jst_at(2026, 7, 30, 17, 0, 29))
        .await;
    let events = drain_until_quiet(&mut rx, Duration::from_millis(300)).await;
    assert!(messages(&events).is_empty(), "停止中の予定は発火しない");
    assert_eq!(
        orchestrator.schedules().await[0].last_consumed_due_ms,
        None,
        "消化もしないこと（再開時に直近の予定から拾うため）"
    );

    // 再開 → 翌週の予定時刻で発火する。
    orchestrator
        .set_schedule_enabled(&task.id, true)
        .await
        .unwrap();
    let mut rx = orchestrator.subscribe();
    orchestrator
        .run_schedule_tick(jst_at(2026, 8, 6, 17, 0, 10))
        .await;
    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    assert!(
        messages(&events)
            .iter()
            .any(|m| m.content.starts_with("【定期実行: ")),
        "再開後は発火すること"
    );
}

/// エージェント削除でその宛先の予定も消える。
#[tokio::test]
async fn delete_agent_removes_its_schedules() {
    let dir = TempDir::new("delete-agent");
    let orchestrator = setup(&dir).await;
    let a = AgentId::from("agent_a");
    let b = AgentId::from("agent_b");
    for (id, name) in [(&a, "アルファ"), (&b, "ブラボー")] {
        orchestrator
            .create_agent(AgentSpec::new(id.clone(), name, "tpl"))
            .await
            .unwrap();
    }
    orchestrator
        .create_schedule(a.clone(), "a の予定".into(), THU_17, ScheduleOptions::default())
        .await
        .unwrap();
    orchestrator
        .create_schedule(
            b.clone(),
            "b の予定".into(),
            Recurrence::Interval { every_minutes: 10 },
            ScheduleOptions::default(),
        )
        .await
        .unwrap();

    orchestrator.delete_agent(&a).await.unwrap();

    let tasks = orchestrator.schedules().await;
    assert_eq!(tasks.len(), 1, "a 宛の予定だけが消えること");
    assert_eq!(tasks[0].to, b);

    // ディスクにも反映されている。
    let text = std::fs::read_to_string(dir.0.join("schedules.json")).unwrap();
    let rows: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["to"], "agent_b");
}

/// 再起動を跨いで予定が生き残り、壊れた 1 件と宛先不明の 1 件だけが落ちる。
#[tokio::test]
async fn schedules_survive_restart_and_broken_rows_are_dropped() {
    let dir = TempDir::new("restart");
    let agent = AgentId::from("agent_01");
    {
        let orchestrator = setup(&dir).await;
        orchestrator
            .create_agent(AgentSpec::new(agent.clone(), "ロボットくん", "tpl"))
            .await
            .unwrap();
        orchestrator
            .create_schedule(agent.clone(), "点検".into(), THU_17, ScheduleOptions::default())
            .await
            .unwrap();
    }

    // 手で壊す: 不正な 1 件（hour 99）と宛先不明の 1 件を注入する。
    let path = dir.0.join("schedules.json");
    let mut rows: Vec<serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    rows.push(serde_json::json!({
        "id": "broken", "to": "agent_01", "message": "x",
        "recurrence": { "kind": "daily", "hour": 99, "minute": 0 },
        "createdAtMs": 0, "lastConsumedDueMs": null, "enabled": true
    }));
    rows.push(serde_json::json!({
        "id": "dangling", "to": "ghost", "message": "x",
        "recurrence": { "kind": "daily", "hour": 9, "minute": 0 },
        "createdAtMs": 0, "lastConsumedDueMs": null, "enabled": true
    }));
    std::fs::write(&path, serde_json::to_string_pretty(&rows).unwrap()).unwrap();

    let orchestrator = setup(&dir).await;
    let tasks = orchestrator.schedules().await;
    assert_eq!(
        tasks.len(),
        1,
        "壊れた 1 件と宛先不明の 1 件だけが落ちること: {tasks:?}"
    );
    assert_eq!(tasks[0].message, "点検");
}

/// schedules.json 全体が JSON として読めないときは、起動は続くが書き込みを拒否する。
#[tokio::test]
async fn corrupt_file_blocks_writes_but_not_boot() {
    let dir = TempDir::new("corrupt");
    std::fs::write(dir.0.join("schedules.json"), "{ これは JSON ではない").unwrap();

    let orchestrator = setup(&dir).await;
    assert!(
        orchestrator.schedules().await.is_empty(),
        "予定なしで起動すること"
    );

    let agent = AgentId::from("agent_01");
    orchestrator
        .create_agent(AgentSpec::new(agent.clone(), "ロボットくん", "tpl"))
        .await
        .unwrap();
    let err = orchestrator
        .create_schedule(agent, "x".into(), THU_17, ScheduleOptions::default())
        .await
        .unwrap_err();
    assert_eq!(
        err.code(),
        "SCHEDULE_STORE_BLOCKED",
        "読めなかったファイルを上書きしない（直せば戻る予定を消さない）"
    );

    // 壊れたファイルはそのまま残っている。
    let text = std::fs::read_to_string(dir.0.join("schedules.json")).unwrap();
    assert!(text.contains("これは JSON ではない"));
}
