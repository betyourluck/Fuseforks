//! サーヴァントのグループ（Spec 51 P1）。
//!
//! 留めるのは 4 つ — 所属が保存を往復して残る / グループを消しても個体の所属は
//! 変わらない（`group_contract` 凍結 3）/ drop の確定が並びと所属を 1 回で書く
//! （凍結 8）/ **実行経路がグループを読まない**（凍結 5。走査で留める）。

use std::path::PathBuf;
use std::sync::Arc;

use fuseforks_core::model::{AgentGroupId, AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-groups-{tag}-{}",
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
        .set_language(fuseforks_core::world::Language::Ja)
        .await
        .unwrap();
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();
    orchestrator
}

/// 所属は `world.json` を往復して残る（村の共有物）。投影（`AgentSnapshot`）にも写る。
#[tokio::test]
async fn membership_survives_a_restart_and_shows_in_the_snapshot() {
    let dir = TempDir::new("persist");
    let group_id = {
        let orchestrator = setup(&dir).await;
        let group = orchestrator.create_group("調査").await.unwrap();
        let mut spec = AgentSpec::new("agent_01", "ザリ", "tpl");
        spec.group_id = Some(group.id.clone());
        let snapshot = orchestrator.create_agent(spec).await.unwrap();
        assert_eq!(snapshot.group_id.as_ref(), Some(&group.id), "投影に所属が写る");
        group.id
    };

    let reopened = setup(&dir).await;
    let groups = reopened.list_groups().await;
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].name, "調査");
    assert!(groups[0].batch_start, "既定は全体 ▶ の対象");
    let snapshot = reopened.snapshot(&AgentId::from("agent_01")).await.unwrap();
    assert_eq!(snapshot.group_id, Some(group_id));
}

/// グループを消しても個体の `group_id` は**変わらない**（凍結 3。役職の削除と同じ）。
#[tokio::test]
async fn deleting_a_group_leaves_the_agents_group_id_untouched() {
    let dir = TempDir::new("delete");
    let orchestrator = setup(&dir).await;
    let group = orchestrator.create_group("リリース").await.unwrap();
    let mut spec = AgentSpec::new("agent_01", "ルナ", "tpl");
    spec.group_id = Some(group.id.clone());
    orchestrator.create_agent(spec).await.unwrap();

    orchestrator.remove_group(&group.id).await.unwrap();

    assert!(orchestrator.list_groups().await.is_empty());
    let snapshot = orchestrator.snapshot(&AgentId::from("agent_01")).await.unwrap();
    assert_eq!(
        snapshot.group_id,
        Some(group.id.clone()),
        "引けない id が残る（無所属として描かれ、次に所属を書く操作で None へ）"
    );
    // 消したものをもう一度消すと名指しで断る（黙って成功しない）。
    let err = orchestrator.remove_group(&group.id).await.unwrap_err();
    assert_eq!(err.code(), "GROUP_NOT_FOUND");
}

/// drop の確定は並びと所属を**一緒に**書き、再起動をまたいで両方が残る（凍結 8）。
#[tokio::test]
async fn a_drop_commits_order_and_membership_together() {
    let dir = TempDir::new("drop");
    let (a, b, c) = (
        AgentId::from("agent_a"),
        AgentId::from("agent_b"),
        AgentId::from("agent_c"),
    );
    let group_id = {
        let orchestrator = setup(&dir).await;
        let group = orchestrator.create_group("調査").await.unwrap();
        for (id, name) in [(&a, "A"), (&b, "B"), (&c, "C")] {
            orchestrator
                .create_agent(AgentSpec::new(id.as_str(), name, "tpl"))
                .await
                .unwrap();
        }
        // C を先頭へ動かし、同時に「調査」へ入れる（別の箱へ落ちた形）。
        orchestrator
            .commit_agent_drop(&[c.clone(), a.clone(), b.clone()], Some((&c, Some(group.id.clone()))))
            .await
            .unwrap();
        group.id
    };

    let reopened = setup(&dir).await;
    let mut snapshots = reopened.snapshots().await;
    snapshots.sort_by_key(|s| s.order);
    let ids: Vec<&str> = snapshots.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["agent_c", "agent_a", "agent_b"], "並びが残る");
    assert_eq!(snapshots[0].group_id, Some(group_id), "所属が残る");
    assert_eq!(snapshots[1].group_id, None, "動かしていない個体の所属は触らない");

    // 無所属の箱へ落ちた形 = None を書く（引けない id の正規化もこの経路）。
    reopened
        .commit_agent_drop(&[a.clone(), b.clone(), c.clone()], Some((&c, None)))
        .await
        .unwrap();
    let snapshot = reopened.snapshot(&c).await.unwrap();
    assert_eq!(snapshot.group_id, None);
    assert_eq!(snapshot.order, 2);
}

/// 空の名前は拒む。改名も同じ門を通る。
#[tokio::test]
async fn a_blank_group_name_is_refused() {
    let dir = TempDir::new("blank");
    let orchestrator = setup(&dir).await;
    let err = orchestrator.create_group("   ").await.unwrap_err();
    assert_eq!(err.code(), "INVALID_GROUP_NAME");

    let mut group = orchestrator.create_group("調査").await.unwrap();
    group.name = "".into();
    let err = orchestrator.upsert_group(group).await.unwrap_err();
    assert_eq!(err.code(), "INVALID_GROUP_NAME");

    let unknown = fuseforks_core::model::AgentGroup {
        id: AgentGroupId::new("missing"),
        name: "x".into(),
        batch_start: true,
    };
    let err = orchestrator.upsert_group(unknown).await.unwrap_err();
    assert_eq!(err.code(), "GROUP_NOT_FOUND");
}

/// **実行経路はグループを読まない**（凍結 5）。配送・委譲・ターン・予定・文脈の
/// 本体に `group` の語が 1 つも無いことを走査で留める — `pricing` が `budget` を
/// 読まない網と同じ形。CRUD の包み（`lifecycle.rs`）だけが例外。
#[test]
fn the_execution_path_never_reads_the_group() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/orchestrator");
    for file in [
        "turn.rs",
        "delegation.rs",
        "context.rs",
        "runtime.rs",
        "schedules.rs",
        "sessions.rs",
        "bootstrap.rs",
        "settings.rs",
        "mod.rs",
    ] {
        let path = root.join(file);
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
        // コメントと doc を落としてから見る（説明文に「グループ」と書くのは自由）。
        let code: String = src
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !code.contains("group_id") && !code.contains("AgentGroup"),
            "{file} の本体がグループを読んでいる（group_contract 凍結 5 が破れた）"
        );
    }
}
