//! `resume after approval:` の計器が 1 押し 1 行で残ることを見る結合テスト
//! （Spec 56 D7）。
//!
//! 診断ログの宛先はプロセスで 1 つ（`OnceLock`）なので、**このファイルは
//! 1 テストだけ**にする（`budget_stop_log.rs` と同じ規律）。
//!
//! # なぜ計器が要るか
//!
//! 「承認して続けさせる」は**人が引いた線**で、機構が起こした配送ではない。
//! どれだけ押されているかが読めないと、この経路が実際に効いているのか、
//! それとも人が結局は入力欄へ打ち直しているのかが後から数えられない。
//!
//! **負の対照を同じテストで取る**（`failures.md` #90 の処方）— 承認だけを
//! 押した回でこの行が出ないことまで見ないと、「常に出る実装」でも緑になる。

use std::path::PathBuf;
use std::sync::Arc;

use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-resume-log-{}",
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

/// 配送した回だけ 1 行増え、残った判断待ちの件数と新しい天井が読める。
#[tokio::test]
async fn one_line_per_press_carries_the_pending_count_and_the_fresh_ceiling() {
    let dir = TempDir::new();
    let log = dir.0.join("fuseforks.log");
    fuseforks_core::open_log(&log).expect("開けること");

    let store = ConfigStore::new(&dir.0);
    let orchestrator = Orchestrator::bootstrap(
        store.clone(),
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
    // 天井は新しい根に与えられる値そのもの。`new_root_budget` を呼ばずに
    // `None` を渡す実装だと `ceiling=-` になって下の期待が落ちる。
    orchestrator.set_token_budget(Some(12_345)).await.unwrap();

    let agent = AgentId::from("agent_01");
    orchestrator
        .create_agent(AgentSpec::new(agent.clone(), "ロボットくん", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&agent).await.unwrap();

    // 判断待ちを 3 件積む（`run` の実行を経ずに `run.json` を直に作る —
    // 見たいのは件数の読み取りであって、積まれ方ではない）。
    store
        .update_command_policy(&agent, |policy| {
            // 戻り値は押し出された要求。3 件では上限に当たらないので `None` が正
            // （ここで捨てると、上限が変わったときに件数の期待だけが黙ってずれる）。
            for (command, args, now) in [
                ("git", vec!["status".to_owned()], 1_000),
                ("ls", Vec::new(), 2_000),
                ("cat", Vec::new(), 3_000),
            ] {
                assert!(
                    policy.note_pending(command, &args, now).is_none(),
                    "3 件では押し出しが起きないこと"
                );
            }
        })
        .await
        .unwrap();

    // 1. 承認だけを押した回（負の対照）— `run.json` は動くが配送は起きない。
    orchestrator
        .approve_command(&agent, "git", &["status".to_owned()], false)
        .await
        .unwrap();
    let text = std::fs::read_to_string(&log).expect("ログが読めること");
    assert!(
        !text.contains("resume after approval:"),
        "承認だけでは 1 行も増えない（退行の網）: {text}"
    );

    // 2. 承認して続けさせた回。
    orchestrator.resume_after_approval(&agent).await.unwrap();

    let text = std::fs::read_to_string(&log).expect("ログが読めること");
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("resume after approval:"))
        .collect();
    assert_eq!(lines.len(), 1, "押した回数 = 行数: {text}");
    assert!(
        lines[0].contains("agent=agent_01"),
        "誰の線かが読めること: {}",
        lines[0]
    );
    assert!(
        lines[0].contains("pending_left=2"),
        "配送した時点で残っている判断待ちの件数が載ること\
         （3 件積んで 1 件承認したので 2）: {}",
        lines[0]
    );
    assert!(
        lines[0].contains("ceiling=12345"),
        "新しい根に与えた天井が載ること（財布を継がず作り直している証拠）: {}",
        lines[0]
    );
}
