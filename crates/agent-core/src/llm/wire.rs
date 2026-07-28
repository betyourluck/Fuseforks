//! ワイヤ型（プロバイダの生の JSON 形）。
//!
//! **ここが LLM との境界の唯一の真実。** 壊れるのは常にこの ser/de なので、
//! 単体テストで形を固定しておく。canonical 型との相互変換は adapter が担う。
//!
//! 応答側のフィールドには一律 `#[serde(default)]` と `Option` を敷いてある。
//! OpenAI 互換を名乗るサーバは実際には形がまちまちで、必須扱いにすると
//! 「動くはずのサーバで丸ごとパースに失敗する」壊れ方をするため。
//!
//! 由来: Kataribe `crates/llm_client/src/wire.rs`。

use serde::{Deserialize, Serialize};

// ============================================================================
// OpenAI 互換 chat/completions
// ============================================================================

/// OpenAI 互換のメッセージ役割。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OaiRole {
    /// システム。
    System,
    /// ユーザー。
    User,
    /// アシスタント。
    Assistant,
    /// ツール結果。
    Tool,
}

/// OpenAI 互換の送信メッセージ。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OaiMessage {
    /// 役割。
    pub role: OaiRole,
    /// 本文。
    pub content: String,
}

/// OpenAI 互換のリクエスト本体。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OaiRequest {
    /// モデル名。
    pub model: String,
    /// 会話履歴。
    pub messages: Vec<OaiMessage>,
    /// サンプリング温度。**明示設定時のみ送る。**
    /// 新しめのモデルは非対応で、送ると 400 を返す。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// 最大出力トークン数。
    pub max_tokens: u32,
    /// ツール定義。空なら送らない。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<OaiTool>,
    /// ツール選択の強制。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<OaiToolChoice>,
    /// 推論制御。対象モデル以外には送らない（キーごと省略）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<&'static str>,
}

/// 関数ツール定義。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OaiTool {
    /// 常に `"function"`。
    #[serde(rename = "type")]
    pub kind: OaiToolKind,
    /// 関数の定義。
    pub function: OaiFunctionDef,
}

/// ツール種別。現状は関数のみ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OaiToolKind {
    /// 関数呼び出し。
    Function,
}

/// 関数の定義本体。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OaiFunctionDef {
    /// 関数名。
    pub name: String,
    /// 説明。
    pub description: String,
    /// 引数の JSON Schema。
    pub parameters: serde_json::Value,
}

/// 特定関数の呼び出しを強制する指定。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum OaiToolChoice {
    /// `"auto"` / `"required"` / `"none"` の素の文字列。
    Mode(&'static str),
    /// `{"type":"function","function":{"name":...}}`。
    Function {
        /// 常に `"function"`。
        #[serde(rename = "type")]
        kind: OaiToolKind,
        /// 対象関数。
        function: OaiToolChoiceFunction,
    },
}

/// 強制対象の関数名。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OaiToolChoiceFunction {
    /// 関数名。
    pub name: String,
}

/// OpenAI 互換のレスポンス本体。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OaiResponse {
    /// 応答候補。
    #[serde(default)]
    pub choices: Vec<OaiChoice>,
    /// 使用量。返さないサーバでも壊れないよう `Option`。
    #[serde(default)]
    pub usage: Option<OaiUsage>,
}

/// OpenAI 互換の使用量。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OaiUsage {
    /// 入力トークン数。
    #[serde(default)]
    pub prompt_tokens: u64,
    /// 出力トークン数。
    #[serde(default)]
    pub completion_tokens: u64,
    /// 入力トークンの内訳。
    #[serde(default)]
    pub prompt_tokens_details: Option<OaiPromptTokensDetails>,
}

/// 入力トークンの内訳。`cached_tokens > 0` はプロンプト先頭がキャッシュから読まれたことを示す。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OaiPromptTokensDetails {
    /// キャッシュから読まれたトークン数。
    #[serde(default)]
    pub cached_tokens: u64,
}

/// 応答候補。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OaiChoice {
    /// 応答メッセージ。
    #[serde(default)]
    pub message: OaiResponseMessage,
    /// 終了理由（`stop` / `tool_calls` / `length` ...）。返さないサーバでも壊れない。
    #[serde(default)]
    pub finish_reason: Option<String>,
}

/// 応答メッセージ。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OaiResponseMessage {
    /// 本文。
    #[serde(default)]
    pub content: Option<String>,
    /// ツール呼び出し。
    #[serde(default)]
    pub tool_calls: Vec<OaiToolCall>,
}

/// ツール呼び出し。
#[derive(Debug, Clone, Deserialize)]
pub struct OaiToolCall {
    /// 呼び出し ID。返さないサーバは空扱い。
    #[serde(default)]
    pub id: Option<String>,
    /// 関数呼び出しの中身。
    pub function: OaiFunctionCall,
}

/// 関数呼び出しの中身。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OaiFunctionCall {
    /// 関数名。
    #[serde(default)]
    pub name: Option<String>,
    /// 引数。**JSON オブジェクトではなく JSON 文字列**である点に注意。
    /// decode 境界で 1 回だけ parse する。
    #[serde(default)]
    pub arguments: String,
}

// ============================================================================
// Anthropic Messages API
// ============================================================================

/// Anthropic Messages API のリクエスト本体。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AnthropicRequest {
    /// モデル名。
    pub model: String,
    /// 最大出力トークン数。Anthropic では必須。
    pub max_tokens: u32,
    /// システムプロンプト。ブロック配列で送ることで `cache_control` を打てる。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub system: Vec<AnthropicTextBlock>,
    /// 会話履歴（system を除く）。
    pub messages: Vec<AnthropicMessage>,
    /// サンプリング温度。明示設定時のみ送る。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// ツール定義。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<AnthropicTool>,
    /// ツール選択の強制。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<AnthropicToolChoice>,
}

/// システムプロンプトのテキストブロック。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AnthropicTextBlock {
    /// 常に `"text"`。
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// 本文。
    pub text: String,
    /// キャッシュ指示。ここより前を再利用対象にする。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<AnthropicCacheControl>,
}

/// プロンプトキャッシュの指示。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnthropicCacheControl {
    /// 常に `"ephemeral"`。
    #[serde(rename = "type")]
    pub kind: &'static str,
}

/// Anthropic の会話メッセージ。`system` ロールは持てない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnthropicMessage {
    /// `"user"` または `"assistant"`。
    pub role: &'static str,
    /// 本文。
    pub content: String,
}

/// Anthropic のツール定義。引数スキーマのキー名が `input_schema` である点が OpenAI と異なる。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AnthropicTool {
    /// 関数名。
    pub name: String,
    /// 説明。
    pub description: String,
    /// 引数の JSON Schema。
    pub input_schema: serde_json::Value,
}

/// Anthropic のツール選択指定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnthropicToolChoice {
    /// `"auto"` / `"any"` / `"tool"`。
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// `kind == "tool"` のときの対象名。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Anthropic Messages API のレスポンス本体。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AnthropicResponse {
    /// 応答コンテンツブロック列。
    #[serde(default)]
    pub content: Vec<AnthropicContentBlock>,
    /// 終了理由（`end_turn` / `tool_use` / `max_tokens` ...）。
    #[serde(default)]
    pub stop_reason: Option<String>,
    /// 使用量。
    #[serde(default)]
    pub usage: Option<AnthropicUsage>,
}

/// 応答のコンテンツブロック。
///
/// `type` による判別共用体。未知の種別（thinking など）が来ても
/// 丸ごと失敗しないよう `Other` で受け止める。
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicContentBlock {
    /// テキスト。
    Text {
        /// 本文。
        #[serde(default)]
        text: String,
    },
    /// ツール呼び出し。`input` は **JSON オブジェクト**（文字列ではない）。
    ToolUse {
        /// 呼び出し ID。
        #[serde(default)]
        id: String,
        /// 関数名。
        #[serde(default)]
        name: String,
        /// 引数オブジェクト。
        #[serde(default)]
        input: serde_json::Value,
    },
    /// 未知の種別。将来のブロック種別で丸ごと壊れないための受け皿。
    #[serde(other)]
    Other,
}

/// Anthropic の使用量。キー名が OpenAI と全く異なるため adapter で正規化する。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AnthropicUsage {
    /// 入力トークン数（キャッシュ分を含まない）。
    #[serde(default)]
    pub input_tokens: u64,
    /// 出力トークン数。
    #[serde(default)]
    pub output_tokens: u64,
    /// キャッシュから読まれたトークン数。
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    /// キャッシュ書き込みに要したトークン数。
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temperature_is_omitted_when_unset() {
        let req = OaiRequest {
            model: "gpt-4o".into(),
            messages: vec![OaiMessage {
                role: OaiRole::User,
                content: "hi".into(),
            }],
            temperature: None,
            max_tokens: 256,
            tools: Vec::new(),
            tool_choice: None,
            reasoning_effort: None,
        };
        let json = serde_json::to_value(&req).unwrap();

        assert!(json.get("temperature").is_none(), "temperature キーごと省略される");
        assert!(json.get("tools").is_none(), "空の tools は送らない");
        assert!(json.get("reasoning_effort").is_none());
    }

    #[test]
    fn response_parses_when_server_omits_optional_fields() {
        // usage も finish_reason も返さない互換サーバを想定する。
        let raw = r#"{"choices":[{"message":{"content":"ok"}}]}"#;
        let resp: OaiResponse = serde_json::from_str(raw).expect("欠落フィールドで壊れないこと");

        assert_eq!(resp.choices.len(), 1);
        assert_eq!(resp.choices[0].message.content.as_deref(), Some("ok"));
        assert!(resp.usage.is_none());
    }

    #[test]
    fn anthropic_unknown_content_block_does_not_break_parsing() {
        let raw = r#"{"content":[{"type":"thinking","thinking":"..."},{"type":"text","text":"ok"}]}"#;
        let resp: AnthropicResponse = serde_json::from_str(raw).expect("未知ブロックを許容すること");

        assert_eq!(resp.content.len(), 2);
        assert!(matches!(resp.content[0], AnthropicContentBlock::Other));
        assert!(matches!(&resp.content[1], AnthropicContentBlock::Text { text } if text == "ok"));
    }
}
