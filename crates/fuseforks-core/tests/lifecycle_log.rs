//! 起動・停止が**ログに残る**ことを見る結合テスト（2026-08-15）。
//!
//! 診断ログの宛先はプロセスで 1 つ（`OnceLock`）なので、`grep_include_log.rs` と
//! 同じく **このファイルは 1 テストだけ**にする。
//!
//! # なぜ計器が要るか
//!
//! 2026-08-15 の実機で「停止したら再稼働できなくなった」が起きたとき、
//! **ログに手掛かりが 1 つも無かった** — `start_agent` / `stop_agent` は
//! 成功しても失敗しても 1 行も出さないので、
//!
//!   - 起動要求が届いていない
//!   - 届いたが返ってこない
//!   - 完了したが画面が古い
//!
//! の 3 つが**同じ無音**に見えた。原因を特定できないまま、アプリを閉じたので
//! 生の状態も失われた。#72（受け皿が底なしだと落ちたものは無かったことになる）
//! と #99（計器は在ったのに、書いた側も読む側も「無い」と言った）の同族。
//!
//! **`joined=` が最も読みたい値。** 停止は join を最大 30 秒しか待たず、
//! 切れても古いタスクは自走を続ける（割り込みの効かないツールを掴んでいる
//! 場合に起きる — MCP ツールは `cancel` を見ない）。`joined=false` は
//! 「停止は返ったが、まだ走っている個体が居る」という状態そのもの。

use std::path::PathBuf;
use std::sync::Arc;

use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("fuseforks-lifecycle-log-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 起動・停止・二重起動の拒否が、それぞれ 1 行ずつ残る。
///
/// **負の対照つき**（#90）— 起動していない個体を停止しても
/// `agent stopped:` は出ない（`NotRunning` で早期に返るため）。
/// これを見ないと「常に出る実装」でも緑になる。
///
/// # 覆っていない範囲（ミューテーションで確かめた事実）
///
/// **`joined` を定数 `true` にする変異では、このテストは緑のまま通る。**
/// ここが踏むのは join が即座に完了する経路だけで、`joined=false`
/// （= 停止が返っても古いタスクがまだ走っている）は 1 度も作られない。
/// 作るには 30 秒の待ちを跨ぐ必要があり、スイートに置ける長さではない。
///
/// **つまりこのテストが留めているのは「行が出ること」であって
/// 「`joined` が実態を映すこと」ではない。** 覆うなら停止の待ち時間を
/// 設定可能にする（`OrchestratorConfig` へ 1 つ足す）判断が要る —
/// 実機で `joined=false` を 1 度も見ていないうちに knob を増やすのは
/// 早いと見て、いまは覆わないことを明示して残す。
#[tokio::test]
async fn start_and_stop_each_leave_one_line() {
    let dir = TempDir::new();
    let log = dir.0.join("fuseforks.log");
    fuseforks_core::open_log(&log).expect("開けること");

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

    let id = AgentId::from("agent_01");
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "ザリ", "tpl"))
        .await
        .unwrap();

    // 1. 稼働していない個体の停止 — 早期に返るので行は出ない（負の対照）。
    assert!(orchestrator.stop_agent(&id).await.is_err(), "NotRunning で断る");

    // 2. 起動 → 1 行。
    orchestrator.start_agent(&id).await.unwrap();

    // 3. 二重起動 → 拒否の 1 行。
    assert!(
        orchestrator.start_agent(&id).await.is_err(),
        "稼働中の起動は AlreadyRunning で断る"
    );

    // 4. 停止 → 1 行。
    orchestrator.stop_agent(&id).await.unwrap();

    let text = std::fs::read_to_string(&log).expect("ログが読めること");
    let line = |needle: &str| -> Vec<String> {
        text.lines()
            .filter(|l| l.contains(needle))
            .map(str::to_owned)
            .collect()
    };

    let started = line("agent started:");
    assert_eq!(started.len(), 1, "起動は 1 行だけ: {text}");
    assert!(
        started[0].contains("agent=agent_01") && started[0].contains("elapsed_ms="),
        "個体と所要時間が載ること: {}",
        started[0]
    );
    assert!(
        started[0].contains("reaped=false"),
        "失敗からの復帰でないことが読めること: {}",
        started[0]
    );

    let refused = line("agent start refused:");
    assert_eq!(refused.len(), 1, "二重起動の拒否も 1 行: {text}");
    assert!(
        refused[0].contains("reason=already_running"),
        "断った理由が読めること: {}",
        refused[0]
    );

    let stopped = line("agent stopped:");
    assert_eq!(
        stopped.len(),
        1,
        "停止は 1 行だけ（稼働していない個体の停止では出ない = 負の対照）: {text}"
    );
    assert!(
        stopped[0].contains("joined=true"),
        "join が完了したかが読めること — false は「停止は返ったがまだ走っている」: {}",
        stopped[0]
    );
}
