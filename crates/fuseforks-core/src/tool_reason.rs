//! ツール呼び出しの理由（Spec 27）。
//!
//! 会話ペインに出ていたのは**ツール名と成否だけ**で、`grep` が 5 回走ったことは
//! 分かっても何を探したのかも、なぜ探したのかも読めなかった。
//! ここはモデルが書く 1 行の意図を運ぶ層。
//!
//! **理由はモデルの自己申告であって監査証跡ではない。** 理由が正しいことは
//! 誰も保証しない（`failures.md` #84 — 引用できることは従うことの証拠にならない）。
//! **`ok` も同じ** — あれは返り値が `Ok` かどうかであって副作用の成否ではない。
//!
//! **説明文で読み手を明かさない**（Spec 27 D9）。「利用者が読みます」と書くと
//! **安心させるための文章を誘発しうる**ので、求めるのは*行為の記述*にしてある。
//! これは保証ではなく傾向。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// 引数に置く欄の名前。
pub const REASON_KEY: &str = "reason";

/// 表示に載る最終的な文字数の上限。**`…` を含む長さ。**
///
/// **典型の長さを決めるのはこの天井ではなく [`REASON_DESCRIPTION`]。**
/// ここは外れ値だけを止める。`reason_chars` の中央値がこの値に貼り付いたら、
/// 天井ではなく説明文を直す（Spec 27 D3）。
pub const MAX_REASON_CHARS: usize = 60;

/// 切り詰めの目印。**1 文字**（最終長を数えるときに効く）。
const ELLIPSIS: char = '…';

/// モデルへ渡す欄の説明。**この村で 1 箇所だけ。**
///
/// 注入は [`crate::tool::ToolRegistry::specs_for`] の 1 箇所で行うので、
/// 9 本のツールへ複製されない = **ドリフトの余地が構造的に無い**。
pub const REASON_DESCRIPTION: &str =
    "この呼び出しで次に何を確かめるのかを 20〜40 字で 1 行。長い説明や手順は書かない。";

/// 理由の状態。
///
/// **`Option<String>` にしない。** `None` が「モデルが書かなかった」と
/// 「そもそも尋ねていない」の両方を指してしまい、**フロントには
/// 「このツールは理由を持てるはずか」を知る手段が原理的に無い**
/// （`wants_reason` はコアにしかなく、MCP 接続は動的）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ReasonState {
    /// モデルが書いた。トリム済みで、超過していれば切り詰め済み。
    Written {
        /// 表示する本文。
        text: String,
    },
    /// 理由を尋ねたが、モデルが書かなかった。画面には「理由なし」。
    Omitted,
    /// **外部（MCP）のスキーマなので尋ねていない。** 画面には「外部ツール」。
    ///
    /// 他人が宣言したスキーマへこちらの欄を生やして転送すると、
    /// `additionalProperties: false` のサーバーが拒否する。
    Unsupported,
    /// **この村の判断で対象外にしているツール。** 画面には理由の行を出さない。
    ///
    /// `ask` / `handoff` / `plan` / `room_log` の合成側 —
    /// **引数が発話として会話ペインに出る**ので理由は重複になる（Spec 27 D2）。
    ///
    /// **`Unsupported` と分けるのは、画面のラベルが嘘になるため** —
    /// `ask_agent_3` に「外部ツール」と出すのは誤り。
    /// **P1 の実装で contract の 3 値では足りないと分かった**（Spec 27 の P1 実装記録）。
    Excluded,
}

/// 計器へ出す状態の名前。
///
/// **`reason_chars` だけでは 3 つの状態が 0 に畳まれる**（書かなかった /
/// 外部なので尋ねていない / 対象外）。**畳んだ時点で後から区別できない** —
/// 実機の初日に、尋ねていない 2 件を「短い理由」として平均に混ぜる誤りを踏んだ。
///
/// **本文は出さない**（`failures.md` #71）。ここが出すのは**閉じた列挙の名前**で、
/// モデルが書いた文字列ではない。
pub fn kind_label(state: &ReasonState) -> &'static str {
    match state {
        ReasonState::Written { .. } => "written",
        ReasonState::Omitted => "omitted",
        ReasonState::Unsupported => "unsupported",
        ReasonState::Excluded => "excluded",
    }
}

/// 引数から理由を読む。
///
/// 返すのは（状態, **トリム後・切り詰め前**の文字数）。
/// **切り詰め後を返すと「モデルが上限を超えて書くか」が全部 [`MAX_REASON_CHARS`] に
/// 貼り付いて測れなくなる**（Spec 27 の検収 5 が死ぬ）。
///
/// 手順は**トリム → 数える → 切り詰め**で固定する。トリムを先に置くのは、
/// **前後の空白付きの秘密が字数として漏れない**ようにするため。
pub fn read(args: &Value) -> (ReasonState, usize) {
    let Some(raw) = args.get(REASON_KEY).and_then(Value::as_str) else {
        return (ReasonState::Omitted, 0);
    };
    let trimmed = raw.trim();
    // **文字数で数える。** `len()` はバイト長なので、日本語では枠の 1/3 で発火する
    // （Spec 16 / Spec 17 の査読で 2 回踏んだ形。テストが ASCII だけだと通る）。
    let chars = trimmed.chars().count();
    if chars == 0 {
        // 空白だけの理由を「書いた」と数えない。
        return (ReasonState::Omitted, 0);
    }
    (ReasonState::Written { text: truncate(trimmed, chars) }, chars)
}

/// スキーマへ理由欄を足す。
///
/// **`required` には入れない**（Spec 27 D4）。埋めない個体の呼び出しが
/// バリデーションで落ちると、**理由を出すための機能がツールを使えなくする**。
///
/// オブジェクトでないスキーマは**触らない**（壊すより出さないほうが安全）。
pub fn inject(parameters: &mut Value) {
    let Some(root) = parameters.as_object_mut() else {
        return;
    };
    let props = root
        .entry("properties")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(props) = props.as_object_mut() else {
        return;
    };
    props.insert(
        REASON_KEY.to_owned(),
        serde_json::json!({
            "type": "string",
            "description": REASON_DESCRIPTION,
        }),
    );
}

/// 超過していれば `MAX_REASON_CHARS - 1` 字 + `…` へ寄せる。
///
/// **最終長はちょうど [`MAX_REASON_CHARS`]。** `…` を含めた長さで数える
/// （含めないと 61 字になり「1 行に収まる」が破れる）。
fn truncate(trimmed: &str, chars: usize) -> String {
    if chars <= MAX_REASON_CHARS {
        return trimmed.to_owned();
    }
    let head: String = trimmed.chars().take(MAX_REASON_CHARS - 1).collect();
    format!("{head}{ELLIPSIS}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_reason_is_written_as_is() {
        let (state, chars) = read(&serde_json::json!({ "reason": "修正前の確認のため" }));
        assert_eq!(state, ReasonState::Written { text: "修正前の確認のため".into() });
        assert_eq!(chars, 9);
    }

    #[test]
    fn a_missing_field_is_omitted() {
        let (state, chars) = read(&serde_json::json!({ "pattern": "foo" }));
        assert_eq!(state, ReasonState::Omitted);
        assert_eq!(chars, 0, "書かなかったものは 0 字");
    }

    #[test]
    fn whitespace_only_is_omitted_not_written() {
        // 空白だけの理由を Written("") にすると、画面に空の行が出る。
        let (state, chars) = read(&serde_json::json!({ "reason": "　 \t\n " }));
        assert_eq!(state, ReasonState::Omitted);
        assert_eq!(chars, 0);
    }

    #[test]
    fn surrounding_whitespace_is_trimmed_before_counting() {
        // トリムを先に置くのは、前後の空白付きの秘密が字数として漏れないため。
        let (state, chars) = read(&serde_json::json!({ "reason": "  確認  " }));
        assert_eq!(state, ReasonState::Written { text: "確認".into() });
        assert_eq!(chars, 2, "空白を含めて数えない");
    }

    #[test]
    fn a_non_string_field_is_omitted() {
        let (state, _) = read(&serde_json::json!({ "reason": 42 }));
        assert_eq!(state, ReasonState::Omitted);
    }

    #[test]
    fn exactly_the_limit_is_not_truncated() {
        // 境界は 0 と 2 だけでは足りない（failures.md #69）。ちょうどを 1 本置く。
        let at_limit: String = "あ".repeat(MAX_REASON_CHARS);
        let (state, chars) = read(&serde_json::json!({ "reason": at_limit }));
        assert_eq!(chars, MAX_REASON_CHARS);
        let ReasonState::Written { text } = state else {
            panic!("Written のはず");
        };
        assert!(!text.ends_with(ELLIPSIS), "ちょうどは切らない");
        assert_eq!(text.chars().count(), MAX_REASON_CHARS);
    }

    #[test]
    fn japanese_over_the_limit_is_truncated_to_exactly_the_limit() {
        // **`len()` 実装ならここが赤になる** — 日本語 61 字は 183 バイトなので、
        // バイト長で数えると 60 バイト = 20 字で切れる。
        let long: String = "あ".repeat(MAX_REASON_CHARS + 1);
        let (state, chars) = read(&serde_json::json!({ "reason": long }));
        assert_eq!(chars, MAX_REASON_CHARS + 1, "数えるのは切り詰め前");
        let ReasonState::Written { text } = state else {
            panic!("Written のはず");
        };
        assert_eq!(
            text.chars().count(),
            MAX_REASON_CHARS,
            "最終長は `…` を含めてちょうど上限"
        );
        assert!(text.ends_with(ELLIPSIS));
    }

    #[test]
    fn japanese_under_the_limit_is_not_truncated() {
        // `len()` 実装で赤になるもう 1 本（20 字を超えた時点で切られる）。
        let text: String = "あ".repeat(30);
        let (state, chars) = read(&serde_json::json!({ "reason": text.clone() }));
        assert_eq!(chars, 30);
        assert_eq!(state, ReasonState::Written { text });
    }

    #[test]
    fn inject_adds_the_field_without_making_it_required() {
        let mut schema = serde_json::json!({
            "type": "object",
            "properties": { "pattern": { "type": "string" } },
            "required": ["pattern"],
        });
        inject(&mut schema);

        assert_eq!(
            schema["properties"][REASON_KEY]["type"], "string",
            "理由欄が生えている"
        );
        assert_eq!(
            schema["required"],
            serde_json::json!(["pattern"]),
            "required は 1 要素も増えない"
        );
    }

    #[test]
    fn inject_creates_properties_when_missing() {
        let mut schema = serde_json::json!({ "type": "object" });
        inject(&mut schema);
        assert_eq!(schema["properties"][REASON_KEY]["type"], "string");
    }

    #[test]
    fn inject_leaves_non_objects_alone() {
        // 壊すより出さないほうが安全。
        let mut schema = serde_json::json!("これはスキーマではない");
        inject(&mut schema);
        assert_eq!(schema, serde_json::json!("これはスキーマではない"));
    }

    #[test]
    fn the_wire_shape_carries_a_kind_tag() {
        // フロントは kind で分岐する（推測しない）。ワイヤ形を凍結する。
        assert_eq!(
            serde_json::to_value(ReasonState::Written { text: "確認".into() }).unwrap(),
            serde_json::json!({ "kind": "written", "text": "確認" })
        );
        assert_eq!(
            serde_json::to_value(ReasonState::Omitted).unwrap(),
            serde_json::json!({ "kind": "omitted" })
        );
        assert_eq!(
            serde_json::to_value(ReasonState::Unsupported).unwrap(),
            serde_json::json!({ "kind": "unsupported" })
        );
        assert_eq!(
            serde_json::to_value(ReasonState::Excluded).unwrap(),
            serde_json::json!({ "kind": "excluded" })
        );
    }
}
