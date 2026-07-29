//! プロバイダ中立の canonical モデル。
//!
//! オーケストレーター（[`crate::orchestrator`]）はこの型だけを組み立て、
//! 各 adapter（[`super::openai_compat`] / [`super::anthropic`]）が
//! **encode / decode の純関数**で wire 形と相互変換する。
//!
//! 契約: **オーケストレーターは wire 形を一切見ない。** 方言の差分は adapter が全部持つ。
//! これにより「新しいプロバイダを足す」作業が adapter 1 ファイルの追加に閉じる。
//!
//! 由来: Kataribe `crates/llm_client/src/canonical.rs`（spec 12 Unified Tool Layer）。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 会話の役割。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// システム指示。
    System,
    /// 人間または上流エージェントからの入力。
    User,
    /// モデルの応答。
    Assistant,
    /// ツール実行の結果。直前の [`Role::Assistant`] のツール呼び出しに対応する。
    Tool,
}

/// 会話の 1 ターン。
///
/// ツール往復のために `tool_calls` と `tool_call_id` を持つ。ほとんどの発話では
/// 空のままだが、**別の型に分けるとプロンプトを組む側が場合分けを抱える**。
/// プロバイダの API がこの形（1 つのメッセージ列に混在させる）を取っているので、
/// canonical でも同じ形にして翻訳の段差を減らす。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    /// 役割。
    pub role: Role,
    /// 本文。ツール呼び出しだけの応答では空になりうる。
    pub content: String,
    /// [`Role::Assistant`] がツールを呼んだとき、その呼び出し。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// [`Role::Tool`] のとき、どの呼び出しへの結果か。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// [`Role::Tool`] のとき、実行したツール名。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

impl ChatMessage {
    /// 役割と本文だけの発話を作る。
    fn plain(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            tool_name: None,
        }
    }

    /// システムメッセージを作る。
    pub fn system(content: impl Into<String>) -> Self {
        Self::plain(Role::System, content)
    }

    /// ユーザーメッセージを作る。
    pub fn user(content: impl Into<String>) -> Self {
        Self::plain(Role::User, content)
    }

    /// アシスタントメッセージを作る。
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::plain(Role::Assistant, content)
    }

    /// ツールを呼んだアシスタントの発話を作る。
    ///
    /// **呼び出しを履歴へ残さずに結果だけ積むと、プロバイダ側が
    /// 「対応する呼び出しが無い結果」を拒否する。** 対で積むこと。
    pub fn assistant_tool_calls(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            tool_calls,
            ..Self::plain(Role::Assistant, content)
        }
    }

    /// ツール実行の結果を作る。
    pub fn tool_result(
        call_id: impl Into<String>,
        tool_name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            tool_call_id: Some(call_id.into()),
            tool_name: Some(tool_name.into()),
            ..Self::plain(Role::Tool, content)
        }
    }
}

/// 推論の深さ。canonical の語彙はこの 5 段階ひとつだけで、
/// 各プロバイダの方言（`reasoning_effort` / `output_config.effort` 等）への
/// 写像と丸めは adapter の責務。
///
/// **未設定（`None`）なら何も送らない。** 黙って既定値を送ると
/// 「効いているつもり」の静かな挙動差になる。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    /// 最小限の推論。
    Low,
    /// 標準。
    Medium,
    /// 深い推論。
    High,
    /// 非常に深い推論。
    ///
    /// serde の `snake_case` 変換では `x_high` になってしまうが、
    /// プロバイダが受け取るワイヤ値は `xhigh` なので明示的に固定する。
    /// [`Effort::as_str`] の戻り値と一致していることがこの型の不変条件。
    #[serde(rename = "xhigh")]
    XHigh,
    /// 上限。
    Max,
}

impl Effort {
    /// wire に載せる文字列。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }

    /// 設定文字列をパースする。
    ///
    /// 不正値は `None` ではなく `Err` にする。黙って無視すると
    /// 「設定したのに効いていない」という静かな漏出になる。
    pub fn parse(raw: &str) -> Result<Self, super::LlmError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" | "x-high" | "x_high" => Ok(Self::XHigh),
            "max" => Ok(Self::Max),
            other => Err(super::LlmError::Config(format!(
                "推論深度 '{other}' を解釈できません (low / medium / high / xhigh / max)"
            ))),
        }
    }
}

/// ツール定義。`parameters` は JSON Schema。
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    /// 関数名。
    pub name: String,
    /// モデルへ渡す説明。
    pub description: String,
    /// 引数の JSON Schema。
    pub parameters: Value,
}

/// ツール選択の方針。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ToolChoice {
    /// ツールを使わせない（既定）。
    #[default]
    None,
    /// モデルの判断に委ねる。
    Auto,
    /// いずれかのツール呼び出しを必須にする。
    Required,
    /// 特定のツール呼び出しを強制する（構造化出力の主経路）。
    Specific(String),
}

/// プロバイダ中立のリクエスト。
#[derive(Debug, Clone, PartialEq)]
pub struct ChatRequest {
    /// プロバイダに渡すモデル名。
    pub model: String,
    /// 会話履歴。
    pub messages: Vec<ChatMessage>,
    /// 利用可能なツール。
    pub tools: Vec<ToolSpec>,
    /// ツール選択方針。
    pub tool_choice: ToolChoice,
    /// サンプリング温度。
    ///
    /// **`None` なら送らない。** 新しめのモデルは `temperature` を非対応にしており、
    /// 送ると 400 を返す。既定値を勝手に補うと、そのモデルで恒久的に失敗する。
    pub temperature: Option<f32>,
    /// 1 応答あたりの最大出力トークン数。
    pub max_tokens: u32,
    /// 推論の深さ。`None` なら送らない。
    pub effort: Option<Effort>,
    /// プロンプト先頭の安定部分の文字数。
    ///
    /// プロンプトキャッシュを効かせるための境界位置で、adapter が
    /// `cache_control` の打ち場所として使う。0 ならキャッシュ指示を出さない。
    pub cacheable_prefix_len: usize,
}

impl ChatRequest {
    /// ツールを使わない素の生成リクエストを作る。
    pub fn plain(model: impl Into<String>, messages: Vec<ChatMessage>, max_tokens: u32) -> Self {
        Self {
            model: model.into(),
            messages,
            tools: Vec::new(),
            tool_choice: ToolChoice::None,
            temperature: None,
            max_tokens,
            effort: None,
            cacheable_prefix_len: 0,
        }
    }
}

/// 応答の終了理由。
///
/// `Length` は空応答防御の判定材料。推論モデルが出力予算を思考に使い切ったときの
/// 一次シグナルであり、通常の空応答と区別するために必要。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Finish {
    /// 正常終了。
    Stop,
    /// ツール呼び出しで終了。
    ToolUse,
    /// 出力上限に到達して打ち切られた。
    Length,
    /// 判別不能・プロバイダ固有。
    ///
    /// 既定値でもある。**分からないものを「正常終了」に倒さない**ため。
    #[default]
    Other,
}

/// プロバイダ中立の使用量。
///
/// `cache_read` はキャッシュ健全性の一次ソース。0 が続く場合は
/// プロンプト先頭が毎回揺れている（＝キャッシュが効いていない）という診断になる。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    /// 入力トークン数。
    pub prompt: u64,
    /// 出力トークン数。
    pub completion: u64,
    /// キャッシュから読まれた入力トークン数。
    pub cache_read: u64,
}

impl Usage {
    /// 課金・統計に用いる合計トークン数。
    pub fn total(&self) -> u64 {
        self.prompt + self.completion
    }
}

/// ツール呼び出し。
///
/// `args` は **必ず JSON オブジェクト**。OpenAI 系の「arguments は JSON 文字列」という
/// 方言は adapter の decode 境界で 1 回だけ解いてから canonical に載せる。
/// 二重エンコードと未パースの取り違えを、境界で殺しておくための不変条件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// 呼び出し ID。プロバイダが返さなければ空文字。
    pub id: String,
    /// 関数名。
    pub name: String,
    /// 解析済みの引数オブジェクト。
    pub args: Value,
    /// プロバイダ固有の不透明な随伴データ。**canonical は中身を解釈しない。**
    ///
    /// Gemini (OpenAI 互換) はツール呼び出しごとに
    /// `extra_content.google.thought_signature`（思考署名）を返し、履歴として
    /// assistant の tool_calls を再送するとき同じ値を返すことを要求する。
    /// 欠くと 400 INVALID_ARGUMENT で、ツールを呼んだ会話の 2 周目が必ず落ちる。
    /// decode した adapter だけが encode で読み戻す（ラウンドトリップ専用）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<Value>,
}

/// プロバイダが**代行して実行した**接地の記録。
///
/// こちらが呼ぶツール（[`ToolCall`]）とは別物。モデルの内部で検索が走り、
/// 結果が応答に織り込まれて返ってくる経路の痕跡で、実行するものは何も無い。
///
/// **これを捨てると、モデルは出典を訊かれたときに作る。**
/// 実際に、参照元を運ぶ経路を持たないまま「根拠 URL を出せ」と求めた結果、
/// ドメインのルート URL に記事の見出しを添えた偽の引用が返った（2026-07-29）。
/// 副作用が起きた事実は、こちらが起こしていなくても記録に残す。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Grounding {
    /// モデルが実際に投げた検索語。
    #[serde(default)]
    pub queries: Vec<String>,
    /// 参照元。**空なら出典は存在しない**（モデルの言う出典を信じない根拠）。
    #[serde(default)]
    pub sources: Vec<GroundingSource>,
}

impl Grounding {
    /// 接地が一切起きていないか。
    pub fn is_empty(&self) -> bool {
        self.queries.is_empty() && self.sources.is_empty()
    }

    /// 別の記録を畳み込む。検索語は文字列、参照元は URL で重複を潰す。
    ///
    /// 1 ターンは複数回の LLM 呼び出しに分かれる（ツールを呼ぶたびに 1 周）。
    /// 接地はそのうちのどの周でも起こりうるので、**周ごとの記録を捨てずに
    /// 足し合わせる**。同じ検索語・同じ URL が複数の周で返ることは普通にあり、
    /// 畳まないと表示に同じ出典が並ぶ。
    pub fn absorb(&mut self, other: Grounding) {
        for query in other.queries {
            if !self.queries.contains(&query) {
                self.queries.push(query);
            }
        }
        for source in other.sources {
            if !self.sources.iter().any(|s| s.uri == source.uri) {
                self.sources.push(source);
            }
        }
    }
}

/// 参照した web ページ 1 件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroundingSource {
    /// URL。
    pub uri: String,
    /// ページ表題。取れなければ空。
    pub title: String,
}

/// プロバイダ中立の応答。
///
/// **履歴には積まれない**（積まれるのは [`ChatMessage`]）。この型に足したフィールドは
/// ラウンドトリップの契約に影響しないので、観測のための席を安全に増やせる。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChatResponse {
    /// 本文（ある場合）。
    pub text: Option<String>,
    /// ツール呼び出し。
    pub tool_calls: Vec<ToolCall>,
    /// 終了理由。
    pub finish: Finish,
    /// 使用量。
    pub usage: Usage,
    /// プロバイダが代行した接地の記録。持たないプロバイダでは空。
    pub grounding: Grounding,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effort_wire_value_matches_as_str() {
        // serde の出す値と as_str() が食い違うと、設定を保存できても読み戻せない。
        for effort in [
            Effort::Low,
            Effort::Medium,
            Effort::High,
            Effort::XHigh,
            Effort::Max,
        ] {
            let json = serde_json::to_string(&effort).unwrap();
            assert_eq!(json, format!("\"{}\"", effort.as_str()));
        }
    }

    #[test]
    fn effort_rejects_unknown_values_loudly() {
        assert_eq!(Effort::parse("high").unwrap(), Effort::High);
        assert_eq!(Effort::parse(" XHigh ").unwrap(), Effort::XHigh);
        assert!(Effort::parse("turbo").is_err());
    }

    #[test]
    fn plain_request_sends_no_temperature() {
        let req = ChatRequest::plain("gpt-4o", vec![ChatMessage::user("hi")], 256);
        assert!(req.temperature.is_none());
        assert_eq!(req.tool_choice, ToolChoice::None);
    }

    #[test]
    fn usage_total_sums_both_directions() {
        let usage = Usage {
            prompt: 100,
            completion: 25,
            cache_read: 80,
        };
        assert_eq!(usage.total(), 125);
    }

    #[test]
    fn absorbing_grounding_merges_rounds_without_duplicates() {
        // 1 ターンは複数周に分かれる。検索した周と関数を呼んだ周が別なら、
        // 畳まない実装は片方を捨てる。
        let mut acc = Grounding::default();
        acc.absorb(Grounding {
            queries: vec!["ARIB B36".into()],
            sources: vec![GroundingSource {
                uri: "https://example.com/a".into(),
                title: "A".into(),
            }],
        });
        acc.absorb(Grounding {
            queries: vec!["ARIB B36".into(), "字幕 変換".into()],
            sources: vec![
                GroundingSource {
                    uri: "https://example.com/a".into(),
                    title: "A（別表題）".into(),
                },
                GroundingSource {
                    uri: "https://example.com/b".into(),
                    title: "B".into(),
                },
            ],
        });

        assert_eq!(acc.queries, vec!["ARIB B36", "字幕 変換"]);
        assert_eq!(
            acc.sources.iter().map(|s| s.uri.as_str()).collect::<Vec<_>>(),
            vec!["https://example.com/a", "https://example.com/b"],
        );
        // 先に入ったほうの表題を残す（後の周で表題だけ変わっても出典は同じ）。
        assert_eq!(acc.sources[0].title, "A");
    }

    #[test]
    fn absorbing_into_an_empty_record_also_dedupes_within_one_round() {
        // 1 応答の groundingChunks にも同じ URL が複数回並ぶ。周をまたがなくても畳む。
        let mut acc = Grounding::default();
        acc.absorb(Grounding {
            queries: vec!["同じ語".into(), "同じ語".into()],
            sources: vec![
                GroundingSource {
                    uri: "https://example.com/x".into(),
                    title: "X".into(),
                },
                GroundingSource {
                    uri: "https://example.com/x".into(),
                    title: "X".into(),
                },
            ],
        });

        assert_eq!(acc.queries.len(), 1);
        assert_eq!(acc.sources.len(), 1);
    }
}
