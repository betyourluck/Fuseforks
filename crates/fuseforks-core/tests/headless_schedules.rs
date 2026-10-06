//! `OrchestratorConfig::run_schedules`（Spec 64 D6）— 偽なら予定のティッカーを起こさない。
//!
//! `ask` / `check` の間に期限の来た予定が同じプロセスで走らないための欄。既定は真で、
//! GUI の起動はバイト等価（ティッカーの有無しか変わらない）。手動の tick
//! （`run_schedule_tick`）はテストの足場なので偽でも呼べる — 止めているのは壁時計で
//! 勝手に回る側だけ。
//!
//! **フラグだけでなく振る舞いで見る。** 期限が来ている予定を置いて実ティッカーを 20 ms
//! 間隔で回し、真なら消化される（`lastConsumedDueMs` が埋まる）・偽なら触れられない、を
//! 対で確かめる。正の対照が無いと、ティッカーが何もしない実装でも偽の側は緑になる。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::schedule::{Recurrence, ScheduleOptions};
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-headless-sched-{tag}-{}",
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

async fn boot(dir: &TempDir, run_schedules: bool, schedule_interval: Duration) -> Orchestrator {
    Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::echo("[echo]")),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig {
            schedule_interval,
            run_schedules,
            ..OrchestratorConfig::default()
        },
    )
    .await
    .expect("bootstrap できること")
}

/// 期限が来ている予定を 1 件置いた村を作る（宛先は停止中）。
///
/// 宛先が存在しない予定は起動時に落とされるので、個体を作ってから予定を作り、
/// 作成時刻だけを 10 分前へずらして書き戻す（1 分間隔なら 10 回ぶん期限が来ている）。
/// 宛先は停止中なので、消化されると「停止中なので飛ばしました」で記録だけ進む —
/// LLM は呼ばれない。
async fn village_with_a_due_schedule(dir: &TempDir) {
    let orchestrator = boot(dir, false, Duration::from_secs(3600)).await;
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();
    let agent = AgentId::from("agent_due");
    orchestrator
        .create_agent(AgentSpec::new(agent.clone(), "宛先", "tpl"))
        .await
        .unwrap();
    orchestrator
        .create_schedule(
            agent,
            "期限の来た依頼".into(),
            Recurrence::Interval { every_minutes: 1 },
            ScheduleOptions::default(),
        )
        .await
        .unwrap();
    // 再起動 = 前のプロセスは閉じている（`sessions.redb` の 2 重オープンを避ける）。
    drop(orchestrator);
    tokio::task::yield_now().await;

    let store = ConfigStore::new(&dir.0);
    let mut tasks = store.load_schedules().await.unwrap().tasks;
    assert_eq!(tasks.len(), 1);
    tasks[0].created_at_ms -= 10 * 60 * 1000;
    assert!(tasks[0].last_consumed_due_ms.is_none());
    store.save_schedules(&tasks).await.unwrap();
}

/// 既定は真（GUI はバイト等価）。
#[test]
fn the_default_runs_schedules() {
    assert!(
        OrchestratorConfig::default().run_schedules,
        "既定が偽になると GUI の予定が止まる"
    );
}

/// 真ならティッカーが期限の来た予定を消化する（正の対照）。
#[tokio::test(flavor = "multi_thread")]
async fn a_due_schedule_is_consumed_when_schedules_run() {
    let dir = TempDir::new("on");
    village_with_a_due_schedule(&dir).await;

    let orchestrator = boot(&dir, true, Duration::from_millis(20)).await;
    assert!(orchestrator.runs_schedules());

    let started = std::time::Instant::now();
    loop {
        if orchestrator.schedules().await[0].last_consumed_due_ms.is_some() {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "20 ms 間隔のティッカーが 10 秒で消化しない"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// 偽ならティッカーを起こさず、期限の来た予定にも触れない（消化の記録が残らない）。
///
/// 待つ時間は真の側の間隔の 25 倍。真の側が 10 秒以内に消化することを上の対照が
/// 確かめているので、ここで `None` のままなら「回っていない」と読める。
#[tokio::test(flavor = "multi_thread")]
async fn a_due_schedule_is_left_alone_when_schedules_do_not_run() {
    let dir = TempDir::new("off");
    village_with_a_due_schedule(&dir).await;

    let orchestrator = boot(&dir, false, Duration::from_millis(20)).await;
    assert!(!orchestrator.runs_schedules());

    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        orchestrator.schedules().await[0].last_consumed_due_ms,
        None,
        "ティッカーを起こさない設定で予定が消化された"
    );
    drop(orchestrator);
    tokio::task::yield_now().await;
    let saved = ConfigStore::new(&dir.0).load_schedules().await.unwrap().tasks;
    assert_eq!(
        saved[0].last_consumed_due_ms, None,
        "ファイルの消化の記録にも触れない（次に GUI で開いたとき再開時の 1 回が働く）"
    );
}
