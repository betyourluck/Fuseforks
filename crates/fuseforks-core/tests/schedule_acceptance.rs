//! Spec 46（予定の後判定）の配線層の結合テスト。
//!
//! 判定そのもの（1 行目の照合・抜粋の規則・再依頼の分岐表）は
//! `src/schedule_probe.rs` / `src/schedule.rs` の単体テストが持つ。ここで
//! 確かめるのは**副作用の配線**: 因果の完了で検収が走るか・`no_match` だけが
//! 再依頼を生むか・試行が上限で止まるか・判定不能が fail-closed か・
//! 要約が確定まで抑止されるか（D2 の直列）。
//!
//! tick は手動（`Orchestrator::run_schedule_tick`）で固定時刻を渡す。
//! 完了の合図（`AgentTyping { active: false }`）はティッカーの購読が拾う —
//! `schedule_interval` を 1 時間にしてあるので、時刻の tick は混ざらない。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fuseforks_core::event::CoreEvent;
use fuseforks_core::model::{AgentId, AgentSpec, Endpoint, ModelTemplate};
use fuseforks_core::orchestrator::ProbeApprovals;
use fuseforks_core::schedule::{Acceptance, Recurrence, ScheduleOptions, Weekday};
use fuseforks_core::schedule_probe::{PROBE_TIMEOUT_DEFAULT, ScheduleProbe, SessionMode};
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};
use tokio::sync::broadcast::Receiver;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-acceptance-{tag}-{}",
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

/// 全部承認する差し込み（承認の門そのものは Spec 28 のテストが持つ）。
struct ApproveAll;

impl ProbeApprovals for ApproveAll {
    fn is_approved(&self, _key: &str) -> bool {
        true
    }
}

/// 何も承認しない差し込み。
struct ApproveNone;

impl ProbeApprovals for ApproveNone {
    fn is_approved(&self, _key: &str) -> bool {
        false
    }
}

fn config() -> OrchestratorConfig {
    OrchestratorConfig {
        schedule_interval: Duration::from_secs(3600),
        ..OrchestratorConfig::default()
    }
}

async fn setup(dir: &TempDir) -> Orchestrator {
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::echo("[echo]")),
        Arc::new(InMemorySecretStore::new()),
        config(),
    )
    .await
    .expect("bootstrap できること");
    // ホストの OS ロケールに依存させない（Spec 35 — 時計を引数で受けるのと同じ規律）。
    orchestrator
        .set_language(fuseforks_core::world::Language::Ja)
        .await
        .unwrap();
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();
    let agent = AgentId::from("agent_01");
    orchestrator
        .create_agent(AgentSpec::new(agent.clone(), "ロボットくん", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&agent).await.unwrap();
    orchestrator
}

/// 一定時間静かになるまでイベントを集める。
///
/// **窓は `stats_interval`（1 秒）より必ず短くする**（failures.md #86 —
/// 統計イベントが毎秒流れるので、1 秒以上の窓は原理的に閉じず、
/// テストが永久に返らない。**このファイルの初版が実際に踏んだ**）。
async fn drain_until_quiet(rx: &mut Receiver<CoreEvent>, quiet: Duration) -> Vec<CoreEvent> {
    let mut events = Vec::new();
    while let Ok(Ok(event)) = tokio::time::timeout(quiet, rx.recv()).await {
        events.push(event);
    }
    events
}

fn messages(events: &[CoreEvent]) -> Vec<&fuseforks_core::AgentMessage> {
    events
        .iter()
        .filter_map(|e| match e {
            CoreEvent::MessageSent { message } => Some(message),
            _ => None,
        })
        .collect()
}

fn jst_at(y: i32, m: u32, d: u32, h: u32, min: u32, s: u32) -> chrono::DateTime<chrono::FixedOffset> {
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

/// `signal` を 1 行だけ印字する検収コマンド。
fn echo_acceptance(signal: &str, expect: &str, max_attempts: u8) -> Acceptance {
    let (command, args) = if cfg!(windows) {
        (
            "cmd".to_owned(),
            vec!["/C".to_owned(), format!("echo {signal}")],
        )
    } else {
        (
            "sh".to_owned(),
            vec!["-c".to_owned(), format!("echo {signal}")],
        )
    };
    Acceptance {
        probe: ScheduleProbe {
            command,
            args,
            expect: expect.to_owned(),
            timeout_secs: PROBE_TIMEOUT_DEFAULT,
            cwd: None,
        },
        max_attempts,
    }
}

/// 後判定つきの予定を 1 件登録して発火まで回し、集めたイベントと予定 ID を返す。
async fn fire_with_acceptance(
    orchestrator: &Orchestrator,
    acceptance: Acceptance,
    summarize_after: bool,
    quiet: Duration,
) -> (Vec<CoreEvent>, String) {
    orchestrator
        .set_probe_approvals(Arc::new(ApproveAll) as Arc<dyn ProbeApprovals>)
        .await;
    let task = orchestrator
        .create_schedule(
            AgentId::from("agent_01"),
            "見張って".into(),
            THU_17,
            ScheduleOptions {
                probe: None,
                session_mode: SessionMode::Continue,
                summarize_after,
                acceptance: Some(acceptance),
            },
        )
        .await
        .unwrap();
    let mut rx = orchestrator.subscribe();
    orchestrator
        .run_schedule_tick(jst_at(2026, 7, 30, 17, 0, 29))
        .await;
    let events = drain_until_quiet(&mut rx, quiet).await;
    (events, task.id)
}

/// 予定発火として配送されたメッセージ（初回 + 再依頼）だけを抜く。
fn deliveries<'e>(sent: &[&'e fuseforks_core::AgentMessage]) -> Vec<&'e fuseforks_core::AgentMessage> {
    sent.iter()
        .filter(|m| m.content.starts_with("【定期実行: "))
        .copied()
        .collect()
}

/// 一致 → 再依頼なしで確定し、直近の結末が `match` で読める。
#[tokio::test]
async fn a_match_settles_without_redelivery() {
    let dir = TempDir::new("match");
    let orchestrator = setup(&dir).await;
    let (events, task_id) = fire_with_acceptance(
        &orchestrator,
        echo_acceptance("OK", "OK", 2),
        false,
        Duration::from_millis(900),
    )
    .await;

    let sent = messages(&events);
    let fired = deliveries(&sent);
    assert_eq!(fired.len(), 1, "一致したら配送は初回の 1 本だけ: {sent:?}");

    let reports = orchestrator.acceptance_reports().await;
    let report = reports.get(&task_id).expect("検収の結末が残ること");
    assert_eq!(report.outcome, "match");
}

/// 不一致 → 失敗注記つきで出し直し、**試行上限で止まる**（3 本目は無い）。
#[tokio::test]
async fn a_no_match_redelivers_with_the_excerpt_then_stops_at_the_limit() {
    let dir = TempDir::new("nomatch");
    let orchestrator = setup(&dir).await;
    // 常に不一致（NO を印字・期待は OK）。総試行 2 = 再依頼 1 回。
    let (events, task_id) = fire_with_acceptance(
        &orchestrator,
        echo_acceptance("NO", "OK", 2),
        false,
        Duration::from_millis(900),
    )
    .await;

    let sent = messages(&events);
    let fired = deliveries(&sent);
    assert_eq!(
        fired.len(),
        2,
        "再依頼は 1 回だけ（3 本目の配送が無いことが判定）: {sent:?}"
    );

    // 初回に注記は無い。
    assert!(
        !fired[0].content.contains("前回の検収が不一致でした"),
        "初回は素の依頼文: {}",
        fired[0].content
    );
    // 再依頼 = 初回の依頼文 + 最新の失敗注記 1 個（D4）。抜粋には検収コマンドの
    // stdout（1 行目込み）が入る。
    assert!(
        fired[1].content.starts_with(&fired[0].content),
        "ベースは常に初回のオリジナル依頼文: {}",
        fired[1].content
    );
    assert!(
        fired[1].content.contains("前回の検収が不一致でした"),
        "失敗の事実が注記される: {}",
        fired[1].content
    );
    assert!(
        fired[1].content.contains("NO"),
        "検収コマンドの出力が抜粋で届く: {}",
        fired[1].content
    );
    assert!(
        matches!(fired[1].from, Endpoint::System) && fired[1].hop == 0,
        "再依頼も予定発火と同格の起点"
    );

    let reports = orchestrator.acceptance_reports().await;
    let report = reports.get(&task_id).expect("検収の結末が残ること");
    assert_eq!(report.outcome, "no_match", "最終試行の結末が残る");
}

/// 判定不能（起動失敗）は **fail-closed** — 再依頼を生まない。
#[tokio::test]
async fn an_error_does_not_redeliver() {
    let dir = TempDir::new("error");
    let orchestrator = setup(&dir).await;
    let mut acceptance = echo_acceptance("OK", "OK", 5);
    // 実在しないコマンド。試行が 5 回残っていても再依頼はゼロでなければならない。
    acceptance.probe.command = "fuseforks-missing-acceptance-cmd".to_owned();
    acceptance.probe.args = vec![];
    let (events, task_id) = fire_with_acceptance(
        &orchestrator,
        acceptance,
        false,
        Duration::from_millis(900),
    )
    .await;

    let sent = messages(&events);
    assert_eq!(
        deliveries(&sent).len(),
        1,
        "判定不能は再依頼を生まない: {sent:?}"
    );
    let reports = orchestrator.acceptance_reports().await;
    let report = reports.get(&task_id).expect("判定不能も沈黙にしない");
    assert_eq!(report.outcome, "error");
}

/// 未承認も判定不能 — 再依頼を生まず、結末だけが残る。
#[tokio::test]
async fn an_unapproved_acceptance_does_not_redeliver() {
    let dir = TempDir::new("unapproved");
    let orchestrator = setup(&dir).await;
    orchestrator
        .set_probe_approvals(Arc::new(ApproveNone) as Arc<dyn ProbeApprovals>)
        .await;
    let task = orchestrator
        .create_schedule(
            AgentId::from("agent_01"),
            "見張って".into(),
            THU_17,
            ScheduleOptions {
                probe: None,
                session_mode: SessionMode::Continue,
                summarize_after: false,
                acceptance: Some(echo_acceptance("OK", "OK", 5)),
            },
        )
        .await
        .unwrap();
    let mut rx = orchestrator.subscribe();
    orchestrator
        .run_schedule_tick(jst_at(2026, 7, 30, 17, 0, 29))
        .await;
    let events = drain_until_quiet(&mut rx, Duration::from_millis(900)).await;

    assert_eq!(deliveries(&messages(&events)).len(), 1);
    let reports = orchestrator.acceptance_reports().await;
    assert_eq!(reports.get(&task.id).unwrap().outcome, "unapproved");
}

/// 要約は**確定まで抑止される**（D2 の直列）。不一致 × 2 試行 + summarizeAfter で
/// 要約の System 行はちょうど 1 本 — 試行ごとに発火していたら 2 本になる。
#[tokio::test]
async fn summarising_waits_until_the_acceptance_loop_settles() {
    let dir = TempDir::new("serialised");
    let orchestrator = setup(&dir).await;
    let (events, _task_id) = fire_with_acceptance(
        &orchestrator,
        echo_acceptance("NO", "OK", 2),
        true,
        Duration::from_millis(900),
    )
    .await;

    let sent = messages(&events);
    assert_eq!(deliveries(&sent).len(), 2, "再依頼までは同じ: {sent:?}");
    let summaries: Vec<_> = sent
        .iter()
        .filter(|m| m.content.contains("予定の完了後に"))
        .collect();
    assert_eq!(
        summaries.len(),
        1,
        "要約は確定後に 1 回だけ（試行ごとに走ると 2 本になる）: {sent:?}"
    );
}
