//! Spec 28（スケジュールの前判定）の配線層の結合テスト。
//!
//! 判定そのもの（1 行目の照合・付記の切り出し・承認鍵）は
//! `src/schedule_probe.rs` の単体テストが持つ。ここで確かめるのは
//! **副作用の配線**: 承認の門・プロセスを起こすかどうか・配送するかどうか・
//! 付記が本文へ届くか・保存がバイト等価か。
//!
//! tick はすべて手動（`Orchestrator::run_schedule_tick`）で固定時刻を渡す。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fuseforks_core::event::CoreEvent;
use fuseforks_core::model::{AgentId, AgentSpec, Endpoint, ModelTemplate};
use fuseforks_core::orchestrator::ProbeApprovals;
use fuseforks_core::schedule::{Recurrence, ScheduleOptions, Weekday};
use fuseforks_core::schedule_probe::{PROBE_TIMEOUT_DEFAULT, ScheduleProbe, SessionMode};
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};
use tokio::sync::broadcast::Receiver;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-probe-{tag}-{}",
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

/// 承認を記録する差し込み。**問われた鍵を覚える**ので、
/// 「そもそも承認を確かめたか」まで検査できる。
#[derive(Default)]
struct Approvals {
    allowed: std::sync::Mutex<Vec<String>>,
    asked: std::sync::Mutex<Vec<String>>,
}

impl ProbeApprovals for Approvals {
    fn is_approved(&self, key: &str) -> bool {
        self.asked.lock().unwrap().push(key.to_owned());
        self.allowed.lock().unwrap().iter().any(|k| k == key)
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
    // ホストの OS ロケールに依存させない（CI は en・開発機は ja — Spec 35 で
    // 言語がコアの挙動の入力になった。時計を引数で受けるのと同じ規律）。
    orchestrator.set_language(fuseforks_core::world::Language::Ja).await.unwrap();
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
/// **`quiet` は `stats_interval`（1 秒）より短くする。** 統計イベントが毎秒
/// 流れているので、1 秒以上の窓は原理的に閉じず、テストが永久に返らない。
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

/// **合図しか出さない**前判定（1 行だけ印字して終わる）。
fn echo_probe(signal: &str, expect: &str) -> ScheduleProbe {
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
    ScheduleProbe {
        command,
        args,
        expect: expect.to_owned(),
        timeout_secs: PROBE_TIMEOUT_DEFAULT,
        cwd: None,
    }
}

/// 合図 + 本文の 2 行を出す前判定。
fn echo_probe_with_body(signal: &str, body: &str, expect: &str) -> ScheduleProbe {
    let mut probe = echo_probe(signal, expect);
    probe.args = if cfg!(windows) {
        vec![
            "/C".to_owned(),
            format!("echo {signal}& echo {body}"),
        ]
    } else {
        vec![
            "-c".to_owned(),
            format!("printf '{signal}\\n{body}\\n'"),
        ]
    };
    probe
}

/// 前判定つきの予定を 1 件登録し、承認の器を返す。
async fn with_probe(
    orchestrator: &Orchestrator,
    probe: ScheduleProbe,
    session_mode: SessionMode,
) -> Arc<Approvals> {
    let approvals = Arc::new(Approvals::default());
    orchestrator
        .set_probe_approvals(Arc::clone(&approvals) as Arc<dyn ProbeApprovals>)
        .await;
    orchestrator
        .create_schedule(
            AgentId::from("agent_01"),
            "見張って".into(),
            THU_17,
            ScheduleOptions {
                probe: Some(probe),
                session_mode,
                summarize_after: false,
            },
        )
        .await
        .unwrap();
    approvals
}

/// 承認済み + 合図が一致 → 配送される。付記は無い（1 行しか出していない）。
#[tokio::test]
async fn an_approved_probe_that_matches_delivers() {
    let dir = TempDir::new("match");
    let orchestrator = setup(&dir).await;
    let probe = echo_probe("CHANGED", "CHANGED");
    let key = probe.approval_key(&orchestrator.village_id().await);
    let approvals = with_probe(&orchestrator, probe, SessionMode::Continue).await;
    approvals.allowed.lock().unwrap().push(key);

    let mut rx = orchestrator.subscribe();
    orchestrator
        .run_schedule_tick(jst_at(2026, 7, 30, 17, 0, 29))
        .await;
    let events = drain_until_quiet(&mut rx, Duration::from_millis(900)).await;
    let sent = messages(&events);

    let fired: Vec<_> = sent
        .iter()
        .filter(|m| m.content.starts_with("【定期実行: 毎週 木曜 17:00】"))
        .collect();
    assert_eq!(fired.len(), 1, "一致したら配送されること: {sent:?}");
    assert!(fired[0].content.contains("見張って"));
    assert!(
        !fired[0].content.contains("【前判定の出力】"),
        "2 行目が無い前判定では付記を足さない: {}",
        fired[0].content
    );
    assert!(
        matches!(fired[0].from, Endpoint::System) && fired[0].hop == 0,
        "前判定を挟んでも起点の性質は変わらない"
    );
}

/// 合図が一致しない → **配送はゼロ。ただし消化はされる**（1 つの due に判定は 1 回）。
#[tokio::test]
async fn a_probe_that_does_not_match_delivers_nothing_but_still_consumes() {
    let dir = TempDir::new("nomatch");
    let orchestrator = setup(&dir).await;
    let probe = echo_probe("UNCHANGED", "CHANGED");
    let key = probe.approval_key(&orchestrator.village_id().await);
    let approvals = with_probe(&orchestrator, probe, SessionMode::Continue).await;
    approvals.allowed.lock().unwrap().push(key);

    let mut rx = orchestrator.subscribe();
    orchestrator
        .run_schedule_tick(jst_at(2026, 7, 30, 17, 0, 29))
        .await;
    let events = drain_until_quiet(&mut rx, Duration::from_millis(900)).await;
    let sent = messages(&events);

    assert!(
        !sent
            .iter()
            .any(|m| m.content.starts_with("【定期実行:")),
        "一致しなければ 1 通も配送しない: {sent:?}"
    );
    // **消化は済んでいる。** 消化しないと同じ due を毎 tick 判定し直し、
    // daily の予定が一致するまで連続ポーリングに化ける。
    let due_ms = jst_at(2026, 7, 30, 17, 0, 0).timestamp_millis() as u64;
    let tasks = orchestrator.schedules().await;
    assert_eq!(
        tasks[0].last_consumed_due_ms,
        Some(due_ms),
        "不一致でも消化する"
    );
}

/// **承認が無ければプロセスを 1 つも起こさない。**
///
/// 「配送が無いこと」だけでは足りない — 実行してから結果を捨てる実装でも
/// 緑になる。前判定に**副作用（ファイルを作る）**を持たせ、
/// **その痕跡が無いこと**で「起こしていない」を読む。
#[tokio::test]
async fn an_unapproved_probe_never_starts_a_process() {
    let dir = TempDir::new("unapproved");
    let orchestrator = setup(&dir).await;
    let witness = dir.0.join("probe_ran.txt");

    // 走れば必ずファイルが残る前判定。
    let (command, args) = if cfg!(windows) {
        (
            "cmd".to_owned(),
            vec![
                "/C".to_owned(),
                format!("echo CHANGED> \"{}\"& echo CHANGED", witness.display()),
            ],
        )
    } else {
        (
            "sh".to_owned(),
            vec![
                "-c".to_owned(),
                format!("echo CHANGED > '{}'; echo CHANGED", witness.display()),
            ],
        )
    };
    let probe = ScheduleProbe {
        command,
        args,
        expect: "CHANGED".to_owned(),
        timeout_secs: PROBE_TIMEOUT_DEFAULT,
        cwd: None,
    };
    // **承認を 1 件も入れない。**
    let approvals = with_probe(&orchestrator, probe, SessionMode::Continue).await;

    let mut rx = orchestrator.subscribe();
    orchestrator
        .run_schedule_tick(jst_at(2026, 7, 30, 17, 0, 29))
        .await;
    let events = drain_until_quiet(&mut rx, Duration::from_millis(900)).await;

    assert!(
        !witness.exists(),
        "未承認の前判定はプロセスごと起こさない（痕跡が残ってはいけない）"
    );
    assert!(
        !messages(&events)
            .iter()
            .any(|m| m.content.starts_with("【定期実行:")),
        "未承認なら配送もしない"
    );
    assert_eq!(
        approvals.asked.lock().unwrap().len(),
        1,
        "承認は 1 回だけ問われること（問わずに通す実装も、二重に問う実装も落とす）"
    );
}

/// **村の識別子が違えば承認は一致しない**（差し替え攻撃の再現形）。
///
/// 同じコマンド行を、別の村の識別子で作った鍵で承認しておく。
/// 実行時に組まれる鍵はこの村の識別子を含むので、一致しない。
#[tokio::test]
async fn an_approval_from_another_village_does_not_apply() {
    let dir = TempDir::new("village");
    let orchestrator = setup(&dir).await;
    let probe = echo_probe("CHANGED", "CHANGED");
    // **別の村**で承認したときの鍵。
    let foreign = probe.approval_key("ffffffff-ffff-4fff-8fff-ffffffffffff");
    let approvals = with_probe(&orchestrator, probe, SessionMode::Continue).await;
    approvals.allowed.lock().unwrap().push(foreign);

    let mut rx = orchestrator.subscribe();
    orchestrator
        .run_schedule_tick(jst_at(2026, 7, 30, 17, 0, 29))
        .await;
    let events = drain_until_quiet(&mut rx, Duration::from_millis(900)).await;

    assert!(
        !messages(&events)
            .iter()
            .any(|m| m.content.starts_with("【定期実行:")),
        "別の村で得た承認では走らない"
    );
    let asked = approvals.asked.lock().unwrap();
    assert_eq!(asked.len(), 1);
    assert_ne!(
        asked[0], "ffffffff-ffff-4fff-8fff-ffffffffffff",
        "問い合わせているのは鍵であって識別子そのものではない"
    );
}

/// 2 行目以降が依頼文へ**付記される**。
#[tokio::test]
async fn the_second_line_onwards_reaches_the_agent() {
    let dir = TempDir::new("appendix");
    let orchestrator = setup(&dir).await;
    let probe = echo_probe_with_body("CHANGED", "3 posts", "CHANGED");
    let key = probe.approval_key(&orchestrator.village_id().await);
    let approvals = with_probe(&orchestrator, probe, SessionMode::Continue).await;
    approvals.allowed.lock().unwrap().push(key);

    let mut rx = orchestrator.subscribe();
    orchestrator
        .run_schedule_tick(jst_at(2026, 7, 30, 17, 0, 29))
        .await;
    let events = drain_until_quiet(&mut rx, Duration::from_millis(900)).await;
    let sent = messages(&events);

    let fired = sent
        .iter()
        .find(|m| m.content.starts_with("【定期実行:"))
        .expect("配送されること");
    assert!(
        fired.content.contains("【前判定の出力】"),
        "付記の見出しが出ること: {}",
        fired.content
    );
    assert!(
        fired.content.contains("3 posts"),
        "2 行目の本文が届くこと: {}",
        fired.content
    );
    assert!(
        !fired.content.contains("CHANGED"),
        "1 行目（合図）は付記に含めない — 判定に使った印を本文へ流さない: {}",
        fired.content
    );
}

/// 前判定を持たない予定は、**保存もバイト等価**で通る（Spec 28 S5）。
///
/// `#[serde(default)]` だけだと読みは通るが、保存の瞬間に 3 欄が書き足される。
/// 読みだけを見て「加算的だから安全」と言えないのがここ。
#[tokio::test]
async fn a_probeless_schedule_round_trips_byte_for_byte() {
    let dir = TempDir::new("roundtrip");
    let orchestrator = setup(&dir).await;
    orchestrator
        .create_schedule(
            AgentId::from("agent_01"),
            "点検".into(),
            THU_17,
            ScheduleOptions::default(),
        )
        .await
        .unwrap();

    let path = dir.0.join("schedules.json");
    let saved = std::fs::read_to_string(&path).unwrap();

    assert!(
        !saved.contains("probe"),
        "既定のままの欄は書き出さない: {saved}"
    );
    assert!(
        !saved.contains("sessionMode"),
        "既定のままの欄は書き出さない: {saved}"
    );
    assert!(
        !saved.contains("summarizeAfter"),
        "既定のままの欄は書き出さない: {saved}"
    );

    // 読み直して保存し直しても 1 バイトも変わらない。
    orchestrator
        .set_schedule_enabled(&orchestrator.schedules().await[0].id.clone(), true)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        saved,
        "同じ内容の保存でファイルが変わらないこと"
    );
}

/// 前判定つきの予定は、保存すると**その 3 欄だけ**が現れる。
#[tokio::test]
async fn a_probe_is_persisted_and_reloads() {
    let dir = TempDir::new("persist");
    let orchestrator = setup(&dir).await;
    let probe = echo_probe("CHANGED", "CHANGED");
    with_probe(&orchestrator, probe, SessionMode::Fresh).await;

    let saved = std::fs::read_to_string(dir.0.join("schedules.json")).unwrap();
    assert!(saved.contains("\"probe\""), "{saved}");
    assert!(saved.contains("\"expect\": \"CHANGED\""), "{saved}");
    assert!(saved.contains("\"sessionMode\": \"fresh\""), "{saved}");
    assert!(
        !saved.contains("summarizeAfter"),
        "偽のままの欄は書き出さない: {saved}"
    );

    // 読み直しても同じ形で戻る。
    let reopened = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::echo("[echo]")),
        Arc::new(InMemorySecretStore::new()),
        config(),
    )
    .await
    .unwrap();
    // ホストの OS ロケールに依存させない（CI は en・開発機は ja — Spec 35 で
    // 言語がコアの挙動の入力になった。時計を引数で受けるのと同じ規律）。
    reopened.set_language(fuseforks_core::world::Language::Ja).await.unwrap();
    let tasks = reopened.schedules().await;
    assert_eq!(tasks.len(), 1);
    let restored = tasks[0].probe.as_ref().expect("前判定が戻ること");
    assert_eq!(restored.expect, "CHANGED");
    assert_eq!(tasks[0].session_mode, SessionMode::Fresh);
}

/// `summarizeAfter` は**因果に参加した個体だけ**を畳む。
///
/// 負の対照が本体 — 同じ村で稼働しているのに**この発火に関わっていない**個体の
/// 履歴は残る。これが崩れると、予定が「関与していない個体のぶんまで払う」形になり、
/// Spec 12 P4 の規律と衝突する。
#[tokio::test]
async fn summarize_after_folds_only_the_agents_in_the_causality() {
    let dir = TempDir::new("summarize");
    let orchestrator = setup(&dir).await;

    // 発火に関わらない 2 体目。**先に会話させて履歴を作っておく** —
    // 履歴が空の個体は元から対象外なので、それでは負の対照にならない。
    let bystander = AgentId::from("agent_02");
    orchestrator
        .create_agent(AgentSpec::new(bystander.clone(), "そばの人", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&bystander).await.unwrap();
    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&bystander, "こんにちは")
        .await
        .unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(600)).await;

    orchestrator
        .create_schedule(
            AgentId::from("agent_01"),
            "見張って".into(),
            THU_17,
            ScheduleOptions {
                probe: None,
                session_mode: SessionMode::Continue,
                summarize_after: true,
            },
        )
        .await
        .unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator
        .run_schedule_tick(jst_at(2026, 7, 30, 17, 0, 29))
        .await;
    let events = drain_until_quiet(&mut rx, Duration::from_millis(900)).await;

    let sent = messages(&events);
    let summary_line = sent
        .iter()
        .find(|m| m.content.contains("予定の完了後に"))
        .unwrap_or_else(|| {
            panic!("自動要約の System 行が出ること（「押しました」の文面と分けてある）: {sent:?}")
        });

    // **体数がそのまま負の対照**。因果の根 1 体だけが畳まれ、同じ村で稼働して
    // いる（履歴もある）そばの人は数に入らない。2 体になったら、予定が
    // 関わっていない個体のぶんまで払っている。
    assert!(
        summary_line.content.contains("予定の完了後に 1 体の記憶を要約しました"),
        "畳むのは因果の根だけであること: {}",
        summary_line.content
    );
}

/// `summarizeAfter` が偽なら、**要約は 1 度も走らない**。
#[tokio::test]
async fn without_the_flag_nothing_is_summarised() {
    let dir = TempDir::new("nosummarize");
    let orchestrator = setup(&dir).await;
    orchestrator
        .create_schedule(
            AgentId::from("agent_01"),
            "見張って".into(),
            THU_17,
            ScheduleOptions::default(),
        )
        .await
        .unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator
        .run_schedule_tick(jst_at(2026, 7, 30, 17, 0, 29))
        .await;
    let events = drain_until_quiet(&mut rx, Duration::from_millis(900)).await;

    assert!(
        !messages(&events)
            .iter()
            .any(|m| m.content.contains("要約しました")),
        "既定では要約しない: {:?}",
        messages(&events)
    );
}

/// 村の識別子は**起動をまたいで変わらない**（変わると全承認が外れる）。
#[tokio::test]
async fn the_village_id_survives_a_restart() {
    let dir = TempDir::new("villageid");
    let orchestrator = setup(&dir).await;
    let first = orchestrator.village_id().await;
    assert_eq!(first.len(), 36, "UUID の書式: {first}");
    drop(orchestrator);

    let reopened = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::echo("[echo]")),
        Arc::new(InMemorySecretStore::new()),
        config(),
    )
    .await
    .unwrap();
    // ホストの OS ロケールに依存させない（CI は en・開発機は ja — Spec 35 で
    // 言語がコアの挙動の入力になった。時計を引数で受けるのと同じ規律）。
    reopened.set_language(fuseforks_core::world::Language::Ja).await.unwrap();
    assert_eq!(
        reopened.village_id().await,
        first,
        "識別子が起動ごとに変わると、承認が毎回外れる"
    );
}
