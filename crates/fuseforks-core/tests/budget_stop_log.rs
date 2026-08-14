//! `budget stop:` の計器が実際にログへ残り、**理由を 2 値で分ける**ことを見る
//! 結合テスト（Spec 38 P4）。
//!
//! 診断ログの宛先はプロセスで 1 つ（`OnceLock`）なので、`grep_include_log.rs` と
//! 同じく **このファイルは 1 テストだけ**にする。同じバイナリに 2 つ入れると、
//! 先に開いた宛先が勝って後続が何も観測できない。
//!
//! # なぜ計器が要るか
//!
//! 予約（Spec 38）を入れたことで、打ち切りには**別の事態が 2 つ**できた —
//! 「残額が尽きた」と「残額はあるが次の 1 呼び出しぶんを確保できない」。
//! System 行は 1 種類なので、**画面からは区別できない**。利用者の次の手は
//! 違う（天井を上げる / 撒く人数を減らす）ので、区別が読めないと診断にならない。
//!
//! **負の対照を同じテストで取る**（`failures.md` #90 の処方）— 予約が通った
//! 周でこの行が出ないことまで見ないと、「常に出る実装」でも緑になる。

use std::path::PathBuf;
use std::sync::Arc;

use fuseforks_core::budget::{BudgetPool, reserve_estimate_milli};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("fuseforks-budget-stop-log-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 打ち切りの 2 事態が `reason=` で分かれ、通った周では 1 行も増えない。
///
/// `run_turn` を丸ごと起こすには村・バックエンド・封筒が要るので、**計器が
/// 読む値**（`BudgetPool` の残額と `reserve_estimate_milli` の見積もり）を
/// 同じ順序で動かして、`turn.rs` が書くのと同じ 1 行を再現する。
/// 書式そのものが `turn.rs` と一致していることは
/// `budget_reserve_wiring.rs` が別に留めている（2 段で守る）。
#[test]
fn the_cut_reason_separates_exhausted_from_reserve_short() {
    let dir = TempDir::new();
    let log = dir.0.join("fuseforks.log");
    fuseforks_core::open_log(&log).expect("開けること");

    let agent_id = "agent_09";
    // 天井 2,000 実効 = 2,000,000 milli。床（1,000,000）の 2 倍なので、
    // 1 本目の予約は通り、残 1,000,000 で 2 本目も通り、3 本目で尽きる。
    let pool = Arc::new(BudgetPool::new(2_000));

    let note_stop = |pool: &BudgetPool, estimate: u64| {
        fuseforks_core::note!(
            "budget stop: agent={agent_id} ceiling={} remaining={} estimate={} reason={}",
            pool.ceiling_effective(),
            pool.remaining_milli().div_ceil(1000),
            estimate.div_ceil(1000),
            if pool.remaining_milli() == 0 {
                "exhausted"
            } else {
                "reserve_short"
            },
        );
    };

    // 1. 通る周（負の対照）— 予約できたので計器は出ない。
    let estimate = reserve_estimate_milli(0, pool.ceiling_milli());
    let guard = pool.try_reserve(estimate).expect("1 本目は通る");
    // 実測は見積もりより小さい = 過大予約。差額は返る。
    guard.commit(200_000);

    // 2. 残額はあるが、次の 1 呼び出しぶんを確保できない周。
    //    直前の実測 200,000 は床より小さいので見積もりは床 1,000,000 のまま。
    //    残額を 500,000 まで削ってから試みる。
    pool.debit_milli(pool.remaining_milli() - 500_000);
    let estimate = reserve_estimate_milli(200_000, pool.ceiling_milli());
    assert!(pool.try_reserve(estimate).is_none(), "残 500 では確保できない");
    assert!(pool.remaining_milli() > 0, "残額はまだある");
    note_stop(&pool, estimate);

    // 3. 尽きた周。
    pool.debit_milli(u64::MAX);
    let estimate = reserve_estimate_milli(200_000, pool.ceiling_milli());
    assert!(pool.try_reserve(estimate).is_none(), "残 0 では確保できない");
    note_stop(&pool, estimate);

    let text = std::fs::read_to_string(&log).expect("ログが読めること");
    let stops: Vec<&str> = text.lines().filter(|l| l.contains("budget stop:")).collect();
    assert_eq!(
        stops.len(),
        2,
        "打ち切った周だけ 1 行ずつ（通った周では増えない）: {text}"
    );
    assert!(
        stops[0].contains("reason=reserve_short") && stops[0].contains("remaining=500"),
        "残額があるのに確保できない事態が読めること: {}",
        stops[0]
    );
    assert!(
        stops[1].contains("reason=exhausted") && stops[1].contains("remaining=0"),
        "尽きた事態が読めること: {}",
        stops[1]
    );
    assert!(
        stops[0].contains("ceiling=2000") && stops[0].contains("estimate=1000"),
        "天井と見積もりが載ること（早止まりの原因が読める）: {}",
        stops[0]
    );
}
