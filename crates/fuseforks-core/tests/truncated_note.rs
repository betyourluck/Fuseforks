//! **払ったトークンが計器に出ることを機械で留める**（2026-08-13）。
//!
//! 出力上限で本文が空になったターンは、当時は `?` で即座に伝播して
//! `turn.rs` の加算に 1 つも到達せず、**課金は起きているのにカードにも予算にも
//! `turn:` 行にも出なかった**。唯一の痕跡が `reject_empty_reasoning` の中の
//! `truncated:` 行で、**その行が本当に出ることをここで確かめる**。
//!
//! 2026-08-16 に台帳側は塞いだ（`Err` に usage を載せ、飛行中台帳が清算する —
//! `tests/failed_turn_settlement.rs`）。この行は**1 呼び出しの計器**として残る:
//! `turn:` はターンの合計なので、どの呼び出しが `limit=` いくつで切れたかは
//! ここにしか出ない。
//!
//! **負の対照を同じテストで取る**（`failures.md` #90 の処方）— 本文がある応答で
//! この行が出ないことまで見ないと、「常に出る実装」でも緑になる。
//! **計器を置いた側と読む側の両方が「無い」と言った**前例（#99）が動機。

use fuseforks_core::llm::openai_compat::reject_empty_reasoning;
use fuseforks_core::llm::{ChatResponse, Finish, Grounding, Usage};

/// 上限に達し、本文もツール呼び出しも無い応答（= 失敗する形）。
fn truncated() -> ChatResponse {
    ChatResponse {
        text: None,
        tool_calls: Vec::new(),
        finish: Finish::Length,
        usage: Usage {
            prompt: 23_440,
            completion: 64,
            cache_read: 12_000,
            reasoning: 64,
        },
        grounding: Grounding::default(),
        reasoning_summary: Vec::new(),
    }
}

#[test]
fn the_paid_tokens_are_logged_when_the_turn_is_rejected() {
    let dir = std::env::temp_dir().join(format!("fuseforks-truncated-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("一時フォルダを作れること");
    let path = dir.join("fuseforks.log");
    // **この統合テストは 1 バイナリ 1 プロセス**なので、`OnceLock` の宛先を
    // ここで確定できる（他のテストと同居させると先に開いたほうが勝つ）。
    fuseforks_core::diag::open_log(&path).expect("ログを開けること");

    // 負の対照を先に取る。**本文がある応答では出ない。**
    let mut ok = truncated();
    ok.text = Some("答えです".into());
    assert!(reject_empty_reasoning(ok, 64).is_ok(), "本文があれば通る");

    let before = std::fs::read_to_string(&path).expect("読めること");
    assert!(
        !before.contains("truncated:"),
        "本文のある応答で計器が鳴っている（常に出る実装と区別できない）"
    );

    // 本番の形。
    let err = reject_empty_reasoning(truncated(), 64);
    assert!(err.is_err(), "本文が空で上限に達したら失敗させる");

    let after = std::fs::read_to_string(&path).expect("読めること");
    let line = after
        .lines()
        .find(|l| l.contains("truncated:"))
        .expect("`truncated:` の行が出ること");

    // **数字まで見る。** 行が出ることだけを見ると、0 を並べる実装でも緑になり、
    // 「払った量が読める」という目的を果たさない。
    assert!(line.contains("limit=64"), "上限が読めること: {line}");
    assert!(line.contains("prompt=23440"), "入力が読めること: {line}");
    assert!(line.contains("cached=12000"), "キャッシュが読めること: {line}");
    assert!(line.contains("completion=64"), "出力が読めること: {line}");
    assert!(line.contains("reasoning=64"), "思考が読めること: {line}");

    let _ = std::fs::remove_dir_all(&dir);
}
