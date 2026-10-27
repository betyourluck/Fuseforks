//! 地図の座標をまとめて保存する（「重なりをほどく」・2026-10-27）。
//!
//! 留めるのは 2 つ — **通知は 1 回**（1 体ずつ書くと体数ぶん地図が取り直され、
//! ほどいている途中の配置が出る）/ 書いた座標は `world.json` を往復して残る。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use fuseforks_core::event::CoreEvent;
use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::world::TopologyPosition;
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-positions-{tag}-{}",
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

async fn setup(dir: &TempDir) -> Orchestrator {
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::echo("[echo]")),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig::default(),
    )
    .await
    .expect("bootstrap できること");
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();
    orchestrator
}

#[tokio::test]
async fn bulk_positions_notify_once_and_survive_a_restart() {
    let dir = TempDir::new("bulk");
    let ids = [AgentId::from("agent_01"), AgentId::from("agent_02"), AgentId::from("agent_03")];
    {
        let orchestrator = setup(&dir).await;
        for (i, id) in ids.iter().enumerate() {
            orchestrator
                .create_agent(AgentSpec::new(id.clone(), format!("個体{i}"), "tpl"))
                .await
                .unwrap();
        }

        let mut rx = orchestrator.subscribe();
        let positions: BTreeMap<AgentId, TopologyPosition> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), TopologyPosition { x: i as f64 * 96.0, y: 0.0 }))
            .collect();
        orchestrator.set_topology_positions(&positions).await.unwrap();

        let mut changed = 0;
        while let Ok(event) = rx.try_recv() {
            if matches!(event, CoreEvent::TopologyChanged) {
                changed += 1;
            }
        }
        assert_eq!(changed, 1, "3 体ぶんの座標でも通知は 1 回");
    }

    let reopened = setup(&dir).await;
    let saved = reopened.topology_positions().await;
    for (i, id) in ids.iter().enumerate() {
        assert_eq!(saved.get(id), Some(&TopologyPosition { x: i as f64 * 96.0, y: 0.0 }));
    }
}
