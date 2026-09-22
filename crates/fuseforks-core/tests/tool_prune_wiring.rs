//! `tool prune:` の計器が、見送った結末でも実測を書くことを留める（Spec 59 rev4）。
//!
//! # なぜ走査で留めるか
//!
//! **この計器は門（[`MIN_REDUCTION`]）が妥当かを数える唯一の材料**で、
//! 実機のログを読む以外に効き目を測る手段が無い。ところが計器の書式は
//! どのテストにも掛かっておらず、**欄を落としても全スイートが緑のまま通る**。
//!
//! 実際に踏んだ（2026-09-22 の P4 検収 2）— rev4 で `Applied::BelowFloor` へ
//! `ratio` を持たせ、その doc に「ログの `ratio=`」と書いたのに、計器は
//! `verdict.label()` しか読んでおらず `dropped=` は 0 固定だった。
//! 実機の `below_floor` 4 件から読めたのは「止まった」ことだけで、
//! **0.24 で惜しかったのか 0.02 で全く効かないのかが区別できなかった**。
//!
//! 規則そのものは配線の事実なので、ソースを走査して直接見る
//! （`budget_reserve_wiring.rs` と同じ手 — 呼び出しの取り違えは
//! コンパイラにも lint にも引っかからない）。
//!
//! [`MIN_REDUCTION`]: fuseforks_core::prune::MIN_REDUCTION

use std::path::Path;

fn read(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

/// 書式に `dropped=` と `ratio=` が並ぶ。
#[test]
fn the_note_has_a_drop_count_and_a_ratio() {
    let src = read("src/orchestrator/turn.rs");
    assert!(
        src.contains("dropped={} ratio={}"),
        "tool prune: の書式に dropped= と ratio= を並べること \
         （門が効いたかを実機のログから数える唯一の材料）"
    );
}

/// **見送った結末も同じ 2 つの式から書く。**
///
/// 対の側。書式だけ見ると「`ratio=-` を直書きして全部 `-` にする」変異が
/// 上のテストを緑のまま通る。
#[test]
fn the_skipped_outcomes_report_what_they_measured() {
    let src = read("src/orchestrator/turn.rs");
    assert!(
        src.contains("verdict.dropped()") && src.contains("verdict.ratio_label()"),
        "結末から実測を引くこと — 見送った枝に 0 と - を埋めると、\
         門で止めたのか採点が何も落とさなかったのかがログから読めない"
    );
    // 採点まで至らなかった枝（skip / 失敗）だけが `-` を書く。**2 箇所**で、
    // 増えていたら判定に至った枝のどれかが実測を捨てている。
    assert_eq!(
        src.matches(", \"-\", 0, 0)").count(),
        2,
        "削減率を持たないのは「採点していない」2 枝（4,000 字未満などの skip と、\
         採点そのものの失敗）だけ"
    );
}
