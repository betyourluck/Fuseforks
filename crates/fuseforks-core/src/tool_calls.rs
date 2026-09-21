//! ツール呼び出しの中身（Spec 57）— 引数と、モデルへ返した本文のリングバッファ。
//!
//! 会話ペインのツール行を開いたときだけ読まれる。**置き場はプロセスのメモリだけ**で、
//! `fuseforks.log` にも `sessions.redb` にも書き出しにも出さない — 引数と出力には
//! 利用者の秘密が入りうる（`failures.md` #71）。線は「運ぶのは可、残すのは不可」
//! （`data_contract.yaml` の `tool_call_detail_contract` が正）。
//!
//! 純データで、イベントの発行は呼び出し側が持つ（`plan::PlanWaveStore` と同じ分業）。

use std::collections::VecDeque;

use serde::Serialize;
use serde_json::Value;

/// 保持する呼び出しの上限。**フロントの `toolRuns` の上限（500）と同じ数** —
/// 数えている対象も同じ（`ToolInvoked` 1 本 = 1 件）なので、通常は
/// 「行がある = 詳細がある」が成り立つ。保証ではない（フロントがイベントを
/// 取りこぼすと 2 つの窓がずれる）。
pub const TOOL_CALL_CAPACITY: usize = 500;

/// 1 件あたり、引数と出力それぞれの上限（文字数）。
///
/// **同梱ツールの出力が 1 件も切れない値**（実測の最大は `grep` の 13,606 字）。
/// 切れるのは MCP の長い出力だけ。
pub const MAX_DETAIL_CHARS: usize = 16_000;

/// ツール呼び出し 1 件の中身。
///
/// **`args` は `args_truncated` だけで形が決まる** — 偽ならモデルが送った JSON 値
/// そのまま、真なら**詰めた形の文字列の先頭 [`MAX_DETAIL_CHARS`] 字**
/// （途中で切った JSON は parse できないので値では返せない）。
/// 受け手は `typeof` で推測せず、旗で分岐する。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallDetail {
    /// `CoreEvent::ToolInvoked` の `call_id` と同じ値。
    pub call_id: u64,
    /// モデルが送った引数。コアは欄を足しも除きもしない（`reason` 欄も除かない）。
    pub args: Value,
    /// 引数の**元の**字数。`serde_json` の詰めた形で数える =
    /// `tool:` 行の `args_chars` と同じ数え方（画面の字数とログの字数が一致する）。
    pub args_chars: u64,
    /// 引数を切ったか。
    pub args_truncated: bool,
    /// モデルへ `tool_result` として返した文字列（切った後）。
    pub body: String,
    /// 本文の**元の**字数。
    pub body_chars: u64,
    /// 本文を切ったか。
    pub body_truncated: bool,
}

/// 呼び出しのリングバッファ。
pub struct ToolCallStore {
    calls: VecDeque<ToolCallDetail>,
    /// 次に払い出す `call_id`。1 始まり。
    ///
    /// **[`ToolCallStore::clear`] でも戻さない** — 画面に残った古い行の id が、
    /// 会話を切り替えた後の新しい呼び出しを指してしまう。
    next_id: u64,
}

impl Default for ToolCallStore {
    fn default() -> Self {
        Self {
            calls: VecDeque::new(),
            next_id: 1,
        }
    }
}

impl ToolCallStore {
    /// 呼び出し 1 件を記録し、`call_id` を払い出す。
    ///
    /// **採番をここに置くのは、並行するターンが同じロックの中で振るため**
    /// （呼び出し側で先に振ると、記録の順と id の順が食い違いうる）。
    pub fn record(&mut self, args: &Value, body: &str) -> u64 {
        let call_id = self.next_id;
        self.next_id += 1;

        let (args, args_chars, args_truncated) = clip_args(args);
        let (body, body_chars, body_truncated) = clip(body);

        if self.calls.len() >= TOOL_CALL_CAPACITY {
            self.calls.pop_front();
        }
        self.calls.push_back(ToolCallDetail {
            call_id,
            args,
            args_chars,
            args_truncated,
            body,
            body_chars,
            body_truncated,
        });
        call_id
    }

    /// 1 件を引く。**`None` は「押し出された / 会話を切り替えた」でエラーではない。**
    #[must_use]
    pub fn get(&self, call_id: u64) -> Option<ToolCallDetail> {
        self.calls.iter().find(|c| c.call_id == call_id).cloned()
    }

    /// 全件を捨てる（会話の切り替え）。採番は戻さない。
    pub fn clear(&mut self) {
        self.calls.clear();
    }

    /// 保持している件数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.calls.len()
    }

    /// 1 件も保持していないか。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }
}

/// 先頭 [`MAX_DETAIL_CHARS`] 字へ切る。返すのは（切った後・元の字数・切ったか）。
///
/// **数えるのも切るのも文字単位。** `len()` で数えると日本語は 1/3 の位置で切れ、
/// バイト位置のスライスは文字の途中で panic する。
fn clip(text: &str) -> (String, u64, bool) {
    let chars = text.chars().count();
    if chars <= MAX_DETAIL_CHARS {
        return (text.to_owned(), chars as u64, false);
    }
    let head: String = text.chars().take(MAX_DETAIL_CHARS).collect();
    (head, chars as u64, true)
}

/// 引数を切る。収まるなら JSON 値のまま、超えるなら詰めた形の文字列の先頭。
fn clip_args(args: &Value) -> (Value, u64, bool) {
    // `Value` の `Display` は詰めた形（`tool:` 行の `args_chars` と同じ式）。
    let compact = args.to_string();
    let (head, chars, truncated) = clip(&compact);
    if truncated {
        (Value::String(head), chars, true)
    } else {
        (args.clone(), chars, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ids_start_at_one_and_the_record_comes_back_verbatim() {
        let mut store = ToolCallStore::default();
        let args = json!({"pattern": "fn main", "path": "src", "reason": "入口を探す"});
        let id = store.record(&args, "src/main.rs:1: fn main() {");
        assert_eq!(id, 1);

        let detail = store.get(id).expect("記録した直後は引ける");
        assert_eq!(detail.args, args, "コアは欄を足しも除きもしない");
        assert!(!detail.args_truncated);
        assert_eq!(detail.body, "src/main.rs:1: fn main() {");
        assert!(!detail.body_truncated);
        assert_eq!(detail.body_chars, 26);
    }

    #[test]
    fn args_chars_counts_the_compact_form_like_the_tool_log_line() {
        let mut store = ToolCallStore::default();
        let args = json!({"path": "設計/メモ.md"});
        let id = store.record(&args, "");
        let detail = store.get(id).expect("引ける");
        // `turn.rs` の `tool:` 行と同じ式。字下げした表示の長さではない。
        assert_eq!(
            detail.args_chars,
            args.to_string().chars().count() as u64
        );
    }

    #[test]
    fn the_oldest_record_is_pushed_out_at_capacity() {
        let mut store = ToolCallStore::default();
        for _ in 0..TOOL_CALL_CAPACITY {
            store.record(&json!({}), "x");
        }
        assert!(store.get(1).is_some());

        let newest = store.record(&json!({}), "y");
        assert_eq!(store.len(), TOOL_CALL_CAPACITY);
        assert!(store.get(1).is_none(), "501 件目で最古が消える");
        assert!(store.get(2).is_some());
        assert_eq!(store.get(newest).map(|d| d.body), Some("y".to_owned()));
    }

    #[test]
    fn a_body_of_exactly_the_limit_is_kept_whole() {
        let mut store = ToolCallStore::default();
        let body = "あ".repeat(MAX_DETAIL_CHARS);
        let id = store.record(&json!({}), &body);
        let detail = store.get(id).expect("引ける");
        assert!(!detail.body_truncated);
        assert_eq!(detail.body.chars().count(), MAX_DETAIL_CHARS);
    }

    #[test]
    fn a_japanese_body_is_cut_by_characters_and_keeps_the_original_count() {
        let mut store = ToolCallStore::default();
        // バイト長なら 48,003。`len()` で数える実装は 16,000 字に届く前に切る。
        let body = "あ".repeat(MAX_DETAIL_CHARS + 1);
        let id = store.record(&json!({}), &body);
        let detail = store.get(id).expect("引ける");
        assert!(detail.body_truncated);
        assert_eq!(detail.body.chars().count(), MAX_DETAIL_CHARS);
        assert_eq!(detail.body_chars, (MAX_DETAIL_CHARS + 1) as u64);
    }

    #[test]
    fn oversized_args_come_back_as_a_cut_string_not_a_json_value() {
        let mut store = ToolCallStore::default();
        let args = json!({"op": "write", "content": "字".repeat(MAX_DETAIL_CHARS)});
        let id = store.record(&args, "ok");
        let detail = store.get(id).expect("引ける");

        assert!(detail.args_truncated);
        let Value::String(head) = &detail.args else {
            panic!("切った引数は文字列で返る（途中で切った JSON は parse できない）");
        };
        assert_eq!(head.chars().count(), MAX_DETAIL_CHARS);
        assert!(head.starts_with("{\""), "詰めた形の先頭");
        assert_eq!(
            detail.args_chars,
            args.to_string().chars().count() as u64,
            "持つのは元の字数"
        );
    }

    #[test]
    fn clear_drops_every_record_but_never_reuses_an_id() {
        let mut store = ToolCallStore::default();
        let first = store.record(&json!({}), "前の会話");
        store.clear();
        assert!(store.is_empty());
        assert!(store.get(first).is_none());

        // 画面に残った古い行の id が、新しい会話の呼び出しを指さない。
        let next = store.record(&json!({}), "新しい会話");
        assert!(next > first);
    }

    #[test]
    fn the_wire_shape_is_camel_case() {
        let mut store = ToolCallStore::default();
        let id = store.record(&json!({"a": 1}), "b");
        let wire = serde_json::to_value(store.get(id).expect("引ける")).expect("直列化");
        let mut keys: Vec<&str> = wire
            .as_object()
            .expect("オブジェクト")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "args",
                "argsChars",
                "argsTruncated",
                "body",
                "bodyChars",
                "bodyTruncated",
                "callId"
            ]
        );
    }
}
