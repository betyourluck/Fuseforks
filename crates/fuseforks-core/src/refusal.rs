//! 断り形の検出（計器。2026-08-24）。
//!
//! `reply:` 行の `refusal=` を埋める純関数だけの層。**配送は 1 ミリも変えない** —
//! Spec 08 の凍結「分類は文言 parse でなく型で運ぶ」が縛るのは配送と分類で、
//! 観測だけの欄はその外に居る。機構ではなく計器から入れる（#47 の規律）。
//!
//! **名前は refusal（断り形）であって failure（未完遂）ではない。**
//! 「できません」が正答である依頼は実在する（不可能なことを不可能と報告するのは
//! 完遂）。ここが数えるのは*形*で、数字の解釈は読む人の側に残す —
//! 計器の名前に判定を埋め込まない。
//!
//! 出自は camel-ai の `validate_task_content`（eigent の土台。「DONE と言われても
//! 信じない」を LLM 不要のヒューリスティックで先に、LLM 採点を後に置く 2 段の
//! 前段）。Spec 27 査読で却下された「結果本文の `Error:` で `ok` を倒す」とは
//! 適用面が違う — あれは**ツール結果**（grep が一致行を返すので偽陽性が壊滅的）、
//! こちらは**依頼主への返信本文**。断り文句は冒頭に来るが `Error:` の引用は
//! 冒頭に来ない、という非対称を窓で使う。
//!
//! **判定は「先頭の窓に語彙が現れるか」であって前方一致ではない。**
//! 「〜いたしかねます」のような断りは文中に現れる形なので、前方一致だと
//! 語彙表の側が原理的に届かない。窓（80 字）が本文中 grep の偽陽性を防ぐ側を担う。

/// 断り文句を探す窓。**返信本文の先頭からの文字数**（バイトではない —
/// バイトで切ると日本語は枠が 1/3 になり、境界で落ちる。#55 の族）。
pub const REFUSAL_WINDOW_CHARS: usize = 80;

/// 断り形の語彙表。**コードの定数で、設定にしない** — 増やすときは
/// コミットがそのまま変更記録になる（閉じた語彙表）。
///
/// 照合は窓を小文字化した上での部分一致なので、**英語はすべて小文字で書く**。
/// 日本語は小文字化の影響を受けない。含意で畳んだ語彙が 2 つある —
/// 「大変申し訳ありませんが」は「申し訳ありませんが」に、「致しかねます」は
/// 「かねます」に含まれるので載せない（載せると決して判定に効かない行になる）。
const REFUSAL_MARKERS: &[&str] = &[
    // en（小文字。U+2019 の引用符はモデルが実際に出すので別項で持つ）
    "i cannot",
    "i can't",
    "i can\u{2019}t",
    "i am unable to",
    "i'm unable to",
    "unable to complete",
    "i don't have the ability",
    "i am not able to",
    // ja（丁寧な断りは冒頭近くに来る）
    "申し訳ありませんが",
    "申し訳ございませんが",
    "申し訳ありません、",
    "対応できません",
    "お手伝いできません",
    "かねます",
];

/// 返信本文が断り形かを判定する。
///
/// 前後の空白を落とし、先頭 [`REFUSAL_WINDOW_CHARS`] 字を小文字化して、
/// [`REFUSAL_MARKERS`] のどれかが**窓の中に現れるか**を見る。
pub fn is_refusal_form(text: &str) -> bool {
    let window: String = text.trim_start().chars().take(REFUSAL_WINDOW_CHARS).collect();
    let window = window.to_lowercase();
    REFUSAL_MARKERS.iter().any(|marker| window.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn japanese_refusals_are_detected() {
        for text in [
            "申し訳ありませんが、その作業には対応できません。",
            "申し訳ございませんが、権限がありません。",
            "申し訳ありません、そのファイルは読めませんでした。",
            "ご依頼の件は対応できません。",
            "この形式のファイルはお手伝いできません。",
            "ご要望にはお応えいたしかねます。",
            "  申し訳ありませんが、できません。", // 先頭空白は落とす
        ] {
            assert!(is_refusal_form(text), "断り形と判定されるべき: {text}");
        }
    }

    #[test]
    fn english_refusals_are_detected_case_insensitively() {
        for text in [
            "I cannot access that file.",
            "I can't complete this request.",
            "I can\u{2019}t help with that.",
            "I am unable to reach the server.",
            "I'm unable to verify the claim.",
            "Sorry, but I was unable to complete the task.",
            "I don't have the ability to run commands.",
            "I am not able to open the attachment.",
        ] {
            assert!(is_refusal_form(text), "断り形と判定されるべき: {text}");
        }
    }

    /// **「できません」が正答の依頼も断り形と数える。** これは仕様 —
    /// 計器の名前が refusal（形）であって failure（未完遂）ではない理由。
    #[test]
    fn a_correct_impossibility_report_still_counts_as_refusal_form() {
        assert!(is_refusal_form(
            "その API は 2026-07-23 に廃止されているため対応できません。代替は /v1/responses です。"
        ));
    }

    #[test]
    fn ordinary_answers_are_not_refusals() {
        for text in [
            "調査が完了しました。結果は blackboard/まとめ.md にあります。",
            "はい、できます。手順は 3 段です。",
            "Error: file not found を検出したので、パスを直して再実行しました。",
            "3 件のうち 2 件が該当しました。",
            "The task finished successfully with exit code 0.",
            "",
            "   ",
        ] {
            assert!(!is_refusal_form(text), "断り形と判定してはいけない: {text}");
        }
    }

    /// 窓の外の断り文句は数えない — 本文中 grep の偽陽性（`Error:` 問題の同族）を
    /// 窓で避けるのがこの計器の設計そのもの。
    #[test]
    fn markers_beyond_the_window_do_not_count() {
        let padding = "あ".repeat(REFUSAL_WINDOW_CHARS);
        let text = format!("{padding}申し訳ありませんが、できません。");
        assert!(!is_refusal_form(&text));
        // 対照: 窓の内側に**丸ごと**収まるなら同じ文言が当たる
        // （窓が効いていることを対で見る。9 = 「申し訳ありませんが」の字数）。
        let inside = "あ".repeat(REFUSAL_WINDOW_CHARS - 9);
        assert!(is_refusal_form(&format!("{inside}申し訳ありませんが")));
    }

    /// 窓はバイトではなく文字で数える（日本語で枠が 1/3 にならないこと）。
    #[test]
    fn the_window_is_measured_in_chars_not_bytes() {
        // 76 字の日本語 + 4 字の断り文句 = ちょうど窓 80 字に収まる。
        // バイトで切る実装だと 80 バイト = 日本語 26 字しか見ず、この文言に届かない。
        let inside = "あ".repeat(76);
        assert!(is_refusal_form(&format!("{inside}かねます")));
    }
}
