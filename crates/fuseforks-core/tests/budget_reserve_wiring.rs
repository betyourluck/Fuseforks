//! 予約をどこで取り、どこで取らないかの配線を留める（Spec 38 P2）。
//!
//! **配送前検査は予約しない。** `deliver_and_wait` 冒頭の門は LLM を呼ばない
//! ので、残額の観測（`has_remaining`）で足りる。ここを `try_reserve` へ
//! 変えると、**配送を拒否するたびに返金が要る**形が黙って生える。
//!
//! 初版はこれを「波が予算切れで止まること」を見る結合テストで書いたが、
//! **時間依存（`drain_until_quiet`）で凍結したい規則を確かめる形**になって
//! いた。規則そのものは配線の事実なので、ソースを走査して直接見る
//! （`probe_approval_pruning.rs` / `defaultEnabledTools.test.ts` と同じ手 —
//! 呼び出しの取り違えはコンパイラにも lint にも引っかからない）。

use std::path::Path;

fn read(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

/// 配送前検査は観測のまま（予約しない）。
#[test]
fn the_pre_delivery_gate_observes_and_never_reserves() {
    let src = read("src/orchestrator/delegation.rs");
    assert!(
        src.contains("has_remaining()"),
        "配送前の門が残額の観測で書かれていること"
    );
    assert!(
        !src.contains("try_reserve("),
        "配送前の門で予約しないこと — 予約すると拒否のたびに返金が要る \
         （Spec 38 Notes 3 / 契約 token_budget.reservation）"
    );
}

/// LLM を呼ぶ 2 箇所は予約する（観測だけで通さない）。
///
/// 対の側。片方だけ見ると「全部 has_remaining に戻す」ミューテーションが
/// 上のテストを緑のまま通ってしまう。
#[test]
fn both_llm_call_sites_reserve_before_calling() {
    let src = read("src/orchestrator/turn.rs");
    let reserves = src.matches("try_reserve(").count();
    assert_eq!(
        reserves, 2,
        "run_turn の周回境界とまとめ呼び出しの 2 箇所で予約すること"
    );
    assert!(
        !src.contains("has_remaining()"),
        "LLM を呼ぶ側に観測だけの門を残さないこと（#105 の再演を塞ぐ）"
    );
    assert_eq!(
        src.matches(".commit(").count(),
        2,
        "予約した 2 箇所とも実測で清算すること（返しっぱなしにしない）"
    );
}
