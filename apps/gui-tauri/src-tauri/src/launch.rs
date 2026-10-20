//! 起動時の一括起動（Spec 69 D3。`launch_exit_contract` 5）。
//!
//! **プロセスに 1 回だけ、組み立てが成功して `AppState` を manage した後に、別のタスクで回す。**
//! 画面の `initialize()` には置かない（再読み込み（F5）のたびに走り、人が止めた個体まで起き直す）。
//! `manage` の前に待たない（`start_agent` は MCP の接続を待つので、全個体ぶん覆いが外れなくなる）。
//!
//! 対象は [`fuseforks_core::Orchestrator::batch_start_ids`]（全体 ▶ と同じ規則）の順で、**直列**。
//! **1 体の失敗で残りを止めない**（CLI の `start_agents` は失敗で全部止めて終わるが、あちらは
//! 「指定どおり起動できなければ実行しない」コマンド。GUI は人が後から ▶ を押せる）。
//!
//! 画面は取りこぼさない — `initialize()` は購読を張ってから全件を取り直すので、購読より前に
//! 起きた個体は取り直しで、後に起きた個体はイベントで映る。新しい IPC は要らない。

use std::fmt::Display;
use std::future::Future;
use std::sync::Arc;

use fuseforks_core::model::AgentId;
use fuseforks_core::Orchestrator;

/// 一括起動の結果。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LaunchBatchReport {
    /// 対象の数。
    pub targets: usize,
    /// 起きた数。
    pub started: usize,
    /// 起こせなかった個体（並びは試した順）。
    pub failed: Vec<AgentId>,
}

impl LaunchBatchReport {
    /// 計器の 1 行（`launch batch: targets=N started=M failed=K …`）。**理由の文は載せない** —
    /// 理由は個体ごとの失敗の行が持つ。
    pub fn line(&self) -> String {
        let mut line = format!(
            "launch batch: targets={} started={} failed={}",
            self.targets,
            self.started,
            self.failed.len()
        );
        if !self.failed.is_empty() {
            let ids: Vec<&str> = self.failed.iter().map(AgentId::as_str).collect();
            line.push(' ');
            line.push_str(&ids.join(","));
        }
        line
    }
}

/// `ids` の順に 1 体ずつ起こす。前の 1 体の起動を待ってから次へ進む。
pub async fn start_in_order<F, Fut, E>(ids: &[AgentId], mut start: F) -> LaunchBatchReport
where
    F: FnMut(AgentId) -> Fut,
    Fut: Future<Output = Result<(), E>>,
    E: Display,
{
    let mut report = LaunchBatchReport {
        targets: ids.len(),
        ..LaunchBatchReport::default()
    };
    for id in ids {
        match start(id.clone()).await {
            Ok(()) => report.started += 1,
            Err(err) => {
                fuseforks_core::note!("launch batch: {} を起こせませんでした（{err}）", id.as_str());
                report.failed.push(id.clone());
            }
        }
    }
    report
}

/// 起動時の一括起動を回す（別のタスクで呼ぶ）。
pub async fn run(orchestrator: Arc<Orchestrator>) {
    let ids = orchestrator.batch_start_ids().await;
    let report = start_in_order(&ids, |id| {
        let orchestrator = Arc::clone(&orchestrator);
        async move { orchestrator.start_agent(&id).await }
    })
    .await;
    fuseforks_core::note!("{}", report.line());
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use super::*;

    fn ids(names: &[&str]) -> Vec<AgentId> {
        names.iter().map(|name| AgentId::new(*name)).collect()
    }

    #[tokio::test]
    async fn agents_start_one_at_a_time_in_order_and_a_failure_does_not_stop_the_rest() {
        let order = Mutex::new(Vec::new());
        let in_flight = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);

        let report = start_in_order(&ids(&["agent_1", "agent_2", "agent_3"]), |id| {
            let order = &order;
            let in_flight = &in_flight;
            let peak = &peak;
            async move {
                let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::task::yield_now().await;
                order.lock().unwrap().push(id.as_str().to_owned());
                in_flight.fetch_sub(1, Ordering::SeqCst);
                if id.as_str() == "agent_2" {
                    Err("MCP の接続に失敗")
                } else {
                    Ok(())
                }
            }
        })
        .await;

        assert_eq!(*order.lock().unwrap(), ["agent_1", "agent_2", "agent_3"]);
        assert_eq!(peak.load(Ordering::SeqCst), 1, "直列 — 同時に 2 体は起こさない");
        assert_eq!(report.targets, 3);
        assert_eq!(report.started, 2);
        assert_eq!(report.failed, ids(&["agent_2"]));
    }

    #[test]
    fn the_line_lists_failed_ids_only_when_some_failed() {
        let ok = LaunchBatchReport {
            targets: 2,
            started: 2,
            failed: Vec::new(),
        };
        assert_eq!(ok.line(), "launch batch: targets=2 started=2 failed=0");
        let failed = LaunchBatchReport {
            targets: 3,
            started: 1,
            failed: ids(&["agent_2", "agent_3"]),
        };
        assert_eq!(
            failed.line(),
            "launch batch: targets=3 started=1 failed=2 agent_2,agent_3"
        );
    }

    #[tokio::test]
    async fn no_targets_starts_nothing() {
        let calls = AtomicUsize::new(0);
        let report = start_in_order(&[], |_id| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Ok::<(), &str>(()) }
        })
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(report.line(), "launch batch: targets=0 started=0 failed=0");
    }
}
