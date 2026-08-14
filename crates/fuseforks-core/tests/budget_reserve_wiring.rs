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

/// 打ち切りの計器は**打ち切った周にだけ**出て、理由を 2 値で分ける（Spec 38 P4）。
///
/// `budget stop:` が無いと、System 行からは「尽きた」と「次の 1 呼び出しぶんを
/// 確保できない（残額はある）」の区別がつかない — 利用者から見て次の手が
/// 違う（天井を上げる / 撒く人数を減らす）ので、同じ文言に畳めない。
///
/// **負の対照つき**（#90）: 通常運転の経路には 1 行も増えないこと。
#[test]
fn the_cut_instrument_separates_exhausted_from_reserve_short() {
    let src = read("src/orchestrator/turn.rs");
    // 先頭の `"` まで含めて数える — 引用符が無いものは計器の説明コメントで、
    // 数えると「コメントを 1 行足したら緑になる」テストになる。
    assert_eq!(
        src.matches("\"budget stop:").count(),
        2,
        "打ち切りの 2 箇所（周回境界・まとめ呼び出し）に計器があること"
    );
    // 書式が壊れていないこと（行継続が消えて空白が埋まった実例がある）。
    assert!(
        !src.contains("budget stop:") || !src.contains("remaining={}  "),
        "計器の書式に連続空白を残さない（ログが読めなくなる）"
    );
    assert!(
        src.contains("reserve_short") && src.contains("exhausted"),
        "理由を 2 値で分けること"
    );
    // 負の対照。予約が通った側に計器を置くと毎周 1 行増える。
    let commit_line = src
        .lines()
        .find(|l| l.contains("guard.commit("))
        .expect("清算の行があること");
    assert!(
        !commit_line.contains("note!"),
        "清算の側には計器を置かない（通常運転でログを増やさない）"
    );
}

/// System 行は「使い切った」と言わない（Spec 38 D3）。
///
/// 予約は次の 1 呼び出しぶんを先に確保するので、**実費を使い切る前に止まる**
/// ことがある。起きた事実は「次のぶんを確保できなかった」で、尽きた場合も
/// 早止まりの場合も正しい。
#[test]
fn the_system_line_does_not_claim_the_budget_was_used_up() {
    let src = read("src/orchestrator/turn.rs");
    assert!(
        !src.contains("を使い切ったため") && !src.contains("is used up"),
        "過大予約による早止まりでは嘘になる文言を使わないこと"
    );
    assert!(
        src.contains("確保できなかったため") && src.contains("Could not reserve"),
        "起きた事実を日英とも書くこと"
    );
}
