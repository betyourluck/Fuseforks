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

/// OpenAI 互換の送信メッセージの本文。
///
/// **添付が無ければ必ず [`Text`](Self::Text) を構築する**（Spec 23 P2）。
/// `#[serde(untagged)]` はシリアライズでは variant がそのまま形を決めるので、
/// `Text` は今日までの素の文字列と 1 バイトも変わらない。**危ないのは構築側** —
/// 「常に `Blocks` を作る」実装にすると添付ゼロの発話でも配列が出て、
/// 素の文字列しか受けない互換サーバで全ターンが落ちる。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OaiContent {
    /// 素の文字列（既定。添付が無い発話はすべてこちら）。
    Text(String),
    /// ブロック列（添付画像つきの発話だけ）。
    Blocks(Vec<OaiContentPart>),
}

impl OaiContent {
    /// 素の文字列なら参照を返す。ブロック列なら `None`。
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            Self::Blocks(_) => None,
        }
    }
}

/// ブロック列の 1 要素（`content` が配列のときの形）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OaiContentPart {
    /// テキスト。
    Text {
        /// 本文。
        text: String,
    },
    /// 画像（data URL）。
    ImageUrl {
        /// `image_url` オブジェクト。
        image_url: OaiImageUrl,
    },
}

/// 画像参照。`url` に `data:image/webp;base64,...` の data URL を入れる。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OaiImageUrl {
    /// data URL（または http URL。この村は data URL しか作らない）。
    pub url: String,
}

/// OpenAI 互換の送信メッセージ。
///
/// ツール往復では 2 通りの形を取る:
/// - assistant がツールを呼んだ発話: `content` は空になりうる。`tool_calls` を持つ
/// - ツール結果: `role: "tool"` + `tool_call_id` + 結果本文
///
/// `Eq` を持たないのは、[`OaiRequestToolCall::extra_content`] が
/// `serde_json::Value`（浮動小数を含みうるため `PartialEq` 止まり）だから。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OaiMessage {
    /// 役割。
    pub role: OaiRole,
    /// 本文。ツール呼び出しだけの発話では省く。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<OaiContent>,
    /// assistant がこの発話で呼んだツール。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<OaiRequestToolCall>,
    /// `role: "tool"` のとき、対応する呼び出しの ID。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl OaiMessage {
    /// 本文だけの発話。
    pub fn text(role: OaiRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: Some(OaiContent::Text(content.into())),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }
}

/// リクエスト側のツール呼び出し（履歴として送り返す形）。
///
/// `arguments` は **JSON 文字列**。応答で受け取ったときと同じ形へ戻す必要がある。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OaiRequestToolCall {
    /// 呼び出し ID。
    pub id: String,
    /// 常に `"function"`。
    #[serde(rename = "type")]
    pub kind: OaiToolKind,
    /// 関数呼び出しの中身。
    pub function: OaiRequestFunctionCall,
    /// プロバイダ固有の随伴データ。応答で受けた値を**そのまま**返す。
    ///
    /// Gemini の思考署名（`{"google":{"thought_signature":...}}`）が入る。
    /// 無いときはキーごと省く——署名を返さないサーバへ null や空オブジェクトを
    /// 送ると、未知キーに厳格なサーバで壊れる。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_content: Option<serde_json::Value>,
}

/// リクエスト側の関数呼び出し。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OaiRequestFunctionCall {
    /// 関数名。
    pub name: String,
    /// 引数（JSON 文字列）。
    pub arguments: String,
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
    /// 最大出力トークン数。**モデルにより欄名が割れる** — gpt-5 系と o 系は
    /// この欄を 400 `unsupported_parameter` で拒否し `max_completion_tokens` を
    /// 要求する。逆に互換サーバ（llama.cpp / vLLM 等）には新欄を知らないものが
    /// あるため、常にどちらか**片方だけ**を送る（選ぶのは
    /// [`super::openai_compat::uses_max_completion_tokens`]）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// 推論系モデル用の出力上限（思考トークン込み）。`max_tokens` と排他。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_completion_tokens: Option<u32>,
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
///
/// `Deserialize` も持つのは、履歴として送り返す [`OaiRequestToolCall`] が
/// リクエスト型でありながら往復可能である必要があるため。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    /// 出力トークンの内訳（Spec 32）。
    ///
    /// **思考の「本文」は `/chat/completions` に来ないが、「数」は来ることがある。**
    /// 推論モデルがこの欄で `reasoning_tokens` を返す。欄が無ければ 0 に落ちる。
    #[serde(default)]
    pub completion_tokens_details: Option<OaiCompletionTokensDetails>,
}

/// 入力トークンの内訳。`cached_tokens > 0` はプロンプト先頭がキャッシュから読まれたことを示す。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OaiPromptTokensDetails {
    /// キャッシュから読まれたトークン数。
    #[serde(default)]
    pub cached_tokens: u64,
}

/// 出力トークンの内訳。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OaiCompletionTokensDetails {
    /// 思考に使われたトークン数。**`completion_tokens` の内数**。
    #[serde(default)]
    pub reasoning_tokens: u64,
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
    /// プロバイダ固有の随伴データ（Gemini の思考署名など）。
    /// 中身は解釈せず、履歴再送時にそのまま返すためだけに保持する。
    #[serde(default)]
    pub extra_content: Option<serde_json::Value>,
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
    /// 拡張思考の要求（Spec 33）。**5 世代のモデルにだけ送る。**
    ///
    /// `None` のときはキーごと省く — 旧世代へ送ると 400 になり、
    /// **省いた形は Spec 33 より前と 1 バイトも変わらない**。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<AnthropicThinking>,
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
    /// 生存期間。省略時はプロバイダ既定の 5 分。
    ///
    /// **対話的な使い方では 5 分は短すぎる。** 人が読んで考えて次を打つまでの
    /// 間隔がそれを超えると、次のターンの 1 回目はまた書き込みになる。
    /// 書き込みは読み取りの 10 倍以上のコストなので、**1 回でも余計な書き込みが
    /// 減れば長い TTL のほうが得**（1h 書き込み 2.0× < 5m 書き込み 1.25× × 2 回）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<&'static str>,
}

/// Anthropic の会話メッセージ。`system` ロールは持てない。
///
/// 本文は常にブロック配列で送る。文字列と配列の両方を受ける API だが、
/// ツール往復では配列しか使えないので、**形を 1 つに揃えて場合分けを消す**。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AnthropicMessage {
    /// `"user"` または `"assistant"`。
    pub role: &'static str,
    /// 本文ブロック。
    pub content: Vec<AnthropicRequestBlock>,
}

/// リクエスト側のコンテンツブロック。
///
/// **ツール結果は `user` ロールのメッセージに載せる**のが Anthropic の形。
/// OpenAI 互換の `role: "tool"` とは構造が違うので、canonical から翻訳する。
///
/// どの種別も `cache_control` を持てる。**キャッシュの境界を増える側（履歴）へ
/// 置くために必要**で、system だけに打っていた頃はツールループが毎周
/// `messages` 全体を素の値段で送り直していた（failures.md #42）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicRequestBlock {
    /// テキスト。
    Text {
        /// 本文。
        text: String,
        /// キャッシュ指示。ここまでを再利用対象にする。
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<AnthropicCacheControl>,
    },
    /// アシスタントが呼んだツール。履歴として送り返す。
    ToolUse {
        /// 呼び出し ID。
        id: String,
        /// 関数名。
        name: String,
        /// 引数（**オブジェクト**。文字列ではない）。
        input: serde_json::Value,
        /// キャッシュ指示。ここまでを再利用対象にする。
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<AnthropicCacheControl>,
    },
    /// ツール実行の結果。
    ToolResult {
        /// 対応する `tool_use` の ID。
        tool_use_id: String,
        /// 結果本文。
        content: String,
        /// キャッシュ指示。ここまでを再利用対象にする。
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<AnthropicCacheControl>,
    },
    /// 添付画像（Spec 23）。**テキストより前に置く**（公式の推奨）。
    Image {
        /// 画像の実体（base64）。
        source: AnthropicImageSource,
        /// キャッシュ指示。ここまでを再利用対象にする。
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<AnthropicCacheControl>,
    },
}

/// Anthropic の画像ソース。この村は base64 だけを使う
/// （URL は外部から到達できる場所へ画像を置くことになる — Spec 23 Notes 8）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnthropicImageSource {
    /// 常に `"base64"`。
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// `image/webp` など。
    pub media_type: String,
    /// base64 エンコード済みデータ。
    pub data: String,
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
    /// 拡張思考。**答えの本文ではない**ので `text` へは混ぜないが、
    /// **要約として受け取る**（Spec 33）。落としたことも従来どおり数える —
    /// 実機で「出力 399 トークンあり・本文なし・ツール呼び出しなし」のターンが
    /// 出たとき、どの計器にも載らなかった。
    ///
    /// **`thinking` は 0 字のことがある** — `display` を送っていない要求
    /// （5 世代の既定は `"omitted"`）と、思考が短い回。`signature` は
    /// どちらの場合も 368〜16,328 字ある（実測）。
    Thinking {
        /// 思考の要約。**英語で返る**（問いが日本語でも。実測）。
        #[serde(default)]
        thinking: String,
    },
    /// 伏せられた拡張思考。扱いは [`Self::Thinking`] と同じ。
    RedactedThinking,
    /// 未知の種別。将来のブロック種別で丸ごと壊れないための受け皿。
    #[serde(other)]
    Other,
}

/// 拡張思考の要求（Spec 33）。
///
/// **`type: "adaptive"` は 5 世代のモデルにしか無い** — 旧世代へ送ると
/// `400 adaptive thinking is not supported on this model`（実測）。
/// 旧形式の `{"type":"enabled","budget_tokens":N}` は**送らない**
/// （旧世代は既定で思考しないので、送ることは*見せる*ことではなく
/// **思考を有効化してコストを増やす**ことになる）。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AnthropicThinking {
    /// 常に `"adaptive"`。
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// 思考の返し方。`"summarized"` を送らないと 5 世代の既定
    /// `"omitted"` が効き、**`signature` だけで本文が 0 字**になる（実測）。
    pub display: &'static str,
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
    /// 出力トークンの内訳（Spec 32 P4）。
    ///
    /// **`thinking` を送っていない要求の応答にも付く** — claude-sonnet-5 は
    /// 既定で思考するため（実測 2026-08-10。5/5 の応答に出た）。
    #[serde(default)]
    pub output_tokens_details: Option<AnthropicOutputTokensDetails>,
}

/// Anthropic の出力トークンの内訳。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AnthropicOutputTokensDetails {
    /// 思考に使われたトークン数。**`output_tokens` の内数**。
    ///
    /// 実測で `output_tokens == thinking_tokens == max_tokens` の応答
    /// （本文が 1 ブロックも無いターン）を観測している — `failures.md` #72 の
    /// 「出力トークンだけがあって本文が無い」の正体。
    #[serde(default)]
    pub thinking_tokens: u64,
}

// ============================================================================
// Gemini ネイティブ generateContent
// ============================================================================
//
// OpenAI 互換層（`/chat/completions`）とは**別の口**。互換層は
// `tools: [{"type":"google_search"}]` を `400 Invalid tool type` で拒否するため、
// Google 検索による接地を使うにはネイティブ経路が要る（実測 2026-07-29）。
//
// 形の要点:
// - system は `contents` に混ぜず `systemInstruction` へ分ける
// - role は `user` / `model` の 2 値。ツール結果も `user` ロールの part として積む
// - part は `type` タグを持たず、**どのキーがあるか**で種別が決まる判別共用体
// - `functionCall`（こちらが実行する）と `toolCall`（Google が実行済み）は別物

/// Gemini のリクエスト。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiRequest {
    /// 会話履歴。system は含めない。
    pub contents: Vec<GeminiContent>,
    /// システム指示。`contents` とは別枠。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<GeminiContent>,
    /// 提示するツール。空なら送らない。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<GeminiTool>,
    /// ツール選択方針。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_config: Option<GeminiToolConfig>,
    /// 生成パラメータ。
    pub generation_config: GeminiGenerationConfig,
}

/// 1 ロール分の発話。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeminiContent {
    /// `"user"` / `"model"`。`systemInstruction` では省く。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// 中身。
    #[serde(default)]
    pub parts: Vec<GeminiPart>,
}

/// 発話の構成要素。
///
/// **`type` タグが無い判別共用体**なので、enum ではなく全フィールド `Option` の
/// 構造体で受ける。未知のキーが増えても既知の部分は読めるという性質が、
/// 応答側フィールドを緩く取る本モジュールの方針と一致する。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiPart {
    /// 本文。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// 思考ブロックであることの印。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought: Option<bool>,
    /// 思考署名。**受けた値をそのまま返す**（ラウンドトリップ専用、中身は解釈しない）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
    /// **こちらが実行する**関数呼び出し。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function_call: Option<GeminiFunctionCall>,
    /// 関数実行の結果（履歴として送る側）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function_response: Option<GeminiFunctionResponse>,
    /// **Google 側が実行済み**の組み込みツール呼び出し（検索など）。
    ///
    /// `functionCall` と取り違えると、実行済みのものを二重に実行しにいく。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<GeminiServerToolCall>,
    /// 組み込みツールの結果。中身は解釈しない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_response: Option<serde_json::Value>,
}

/// 関数呼び出し。`args` は **JSON オブジェクト**（OpenAI 系の文字列方言ではない）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeminiFunctionCall {
    /// 呼び出し ID。返さないモデルもあるので `Option`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// 関数名。
    #[serde(default)]
    pub name: String,
    /// 引数オブジェクト。
    #[serde(default)]
    pub args: serde_json::Value,
}

/// 関数実行の結果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeminiFunctionResponse {
    /// 対応する呼び出しの ID。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// 関数名。
    pub name: String,
    /// 結果オブジェクト。文字列を直に置けないので `{"result": ...}` で包む。
    pub response: serde_json::Value,
}

/// Google 側が実行した組み込みツールの記録。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiServerToolCall {
    /// 種別（例: `GOOGLE_SEARCH_WEB`）。
    #[serde(default)]
    pub tool_type: String,
    /// 引数。検索なら `{"queries": [...]}`。
    #[serde(default)]
    pub args: serde_json::Value,
    /// 呼び出し ID。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// 提示するツール 1 件。
///
/// 組み込みツールと関数宣言は**同じ配列に別要素として**並べる。
/// キー名は実測で通った綴り（`google_search` / `functionDeclarations`）に合わせる。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct GeminiTool {
    /// Google 検索による接地。有効化は空オブジェクトを置くことで表す。
    #[serde(rename = "google_search", skip_serializing_if = "Option::is_none")]
    pub google_search: Option<serde_json::Value>,
    /// 関数宣言。
    #[serde(
        rename = "functionDeclarations",
        skip_serializing_if = "Option::is_none"
    )]
    pub function_declarations: Option<Vec<GeminiFunctionDeclaration>>,
}

/// 関数の宣言。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GeminiFunctionDeclaration {
    /// 関数名。
    pub name: String,
    /// 説明。
    pub description: String,
    /// 引数のスキーマ。
    ///
    /// **JSON Schema そのものではなく OpenAPI 3.0 の部分集合**で、未知キー
    /// （`$schema` / `additionalProperties` 等）は 400 で拒否される。
    /// adapter 側で削ってから載せること。引数を取らない関数では `None`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

/// ツール選択方針。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiToolConfig {
    /// 関数呼び出しの制御。関数を 1 つも提示していないなら送らない。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_calling_config: Option<GeminiFunctionCallingConfig>,
    /// サーバー側ツールの実行記録を応答に含めるか。
    ///
    /// **組み込みツール（`google_search`）と関数呼び出しを併用するなら必須。**
    /// 欠くと 400 で
    /// `Please enable tool_config.include_server_side_tool_invocations to use
    /// Built-in tools with Function calling.` と名指しで断られる（実測 2026-07-29）。
    ///
    /// 真にすると応答の parts に `toolCall` / `toolResponse` が現れる。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_server_side_tool_invocations: Option<bool>,
}

/// 関数呼び出しの制御。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiFunctionCallingConfig {
    /// `AUTO` / `ANY` / `NONE`。
    pub mode: &'static str,
    /// `ANY` のとき呼び出しを許す関数名。特定ツールの強制に使う。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_function_names: Option<Vec<String>>,
}

/// 生成パラメータ。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiGenerationConfig {
    /// サンプリング温度。**未設定ならキーごと省く**（OpenAI 系と同じ理由）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// 最大出力トークン数。
    pub max_output_tokens: u32,
    /// 思考の返し方（Spec 33）。
    pub thinking_config: GeminiThinkingConfig,
}

/// 思考の設定（Spec 33）。
///
/// **`include_thoughts` を送らないと `thought: true` の part がそもそも返らない**
/// （実測。既定では答えの part 1 つだけ）。**思考自体は送信の有無に関わらず
/// 起きており**（実測 `thoughtsTokenCount` 3,773）、この欄は返し方だけを変える。
///
/// **モデルによるガードは要らない** — `gemini-3.5-flash-lite` と
/// `gemini-3.6-flash` の双方が 200 で受けた（Anthropic の `adaptive` は
/// 旧世代で 400 になるので、そちらだけガードがある）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiThinkingConfig {
    /// 思考の part を応答へ含めるか。
    pub include_thoughts: bool,
}

/// Gemini の応答。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiResponse {
    /// 候補。通常 1 件。
    #[serde(default)]
    pub candidates: Vec<GeminiCandidate>,
    /// 使用量。
    #[serde(default)]
    pub usage_metadata: Option<GeminiUsageMetadata>,
}

/// 応答の候補。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiCandidate {
    /// 中身。
    #[serde(default)]
    pub content: Option<GeminiContent>,
    /// 終了理由（`STOP` / `MAX_TOKENS` など）。
    ///
    /// **`STOP` は終了を意味しない。** 関数呼び出しを返したときも `STOP` が来る
    /// （実測 2026-07-29: `finishMessage: "Model generated function call(s)."`）。
    /// 終了判定は parts の中身で行うこと。
    #[serde(default)]
    pub finish_reason: Option<String>,
    /// 接地の来歴（参照した web ページ）。
    ///
    /// **モデルが「出典」として語る文字列を信じてはいけない。**
    /// 実在の URL はここにしか無く、ここが空なら出典は存在しない。
    /// 空のまま出典を求めると、モデルは引用らしく見える文字列を作る
    /// （実測 2026-07-29: ドメインのルート URL に記事の見出しが添えられた）。
    #[serde(default)]
    pub grounding_metadata: Option<GeminiGroundingMetadata>,
}

/// 接地の来歴。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiGroundingMetadata {
    /// 参照元。
    #[serde(default)]
    pub grounding_chunks: Vec<GeminiGroundingChunk>,
    /// モデルが実際に投げた検索語。
    #[serde(default)]
    pub web_search_queries: Vec<String>,
}

/// 参照元 1 件。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GeminiGroundingChunk {
    /// web ページ。他の種別（検索以外の接地）では欠ける。
    #[serde(default)]
    pub web: Option<GeminiGroundingWeb>,
}

/// 参照した web ページ。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GeminiGroundingWeb {
    /// URL。
    #[serde(default)]
    pub uri: Option<String>,
    /// ページ表題。
    #[serde(default)]
    pub title: Option<String>,
}

/// Gemini の使用量。キー名が OpenAI とも Anthropic とも違う。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiUsageMetadata {
    /// 入力トークン数。
    #[serde(default)]
    pub prompt_token_count: u64,
    /// 出力トークン数。
    #[serde(default)]
    pub candidates_token_count: u64,
    /// 思考に使われたトークン数。課金対象。
    #[serde(default)]
    pub thoughts_token_count: u64,
    /// キャッシュから読まれたトークン数。
    #[serde(default)]
    pub cached_content_token_count: u64,
}

// ============================================================================
// xAI Responses（`/v1/responses`。Spec 31）
// ============================================================================
//
// OpenAI 互換層（`/chat/completions`）とは**別の口**。Grok の Live Search
// （Agent Tools の `web_search` / `x_search`）は互換層に存在せず、legacy の
// `search_parameters` は HTTP 410 Gone でサーバー自身が後継を名指しして断る
// （実測 2026-08-09）。検索を使うときだけこの経路が要る。
//
// 形の要点（すべて実 probe で確認。data_contract の `xai_responses` が正）:
// - リクエストは `messages` ではなく `input`（メッセージと関数往復の混在列）
// - `include: ["no_inline_citations"]` を常送 — 本文の `[[N]](url)` 汚染を防ぎ、
//   annotations が「参照した分」から「触れた全 URL」へ増える（2 → 18 件を実測）
// - 応答は `output` 配列。`message` / `reasoning` / 検索呼び出し / `function_call`
//   が interleave する。検索呼び出しの型名は 3 つ観測されうる
//   （`web_search_call` / `x_search_call` / `custom_tool_call`）
// - トップレベル `citations` 欄は生の REST に存在しない（SDK の集約属性）

/// xAI Responses へのリクエスト。
#[derive(Debug, Serialize)]
pub struct XaiRequest {
    /// モデル名。
    pub model: String,
    /// 会話とツール往復の混在列（`messages` に相当）。
    pub input: Vec<ResponsesInputItem>,
    /// 空なら欄ごと省く（`tools: []` を送る意味は無い）。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ResponsesTool>,
    /// 契約により `["no_inline_citations"]` 固定。
    pub include: Vec<&'static str>,
    /// 応答を接続先に保持させるか。**契約により常に `false`**（Spec 34 D3）。
    ///
    /// **`Option` にせず常送する。** `skip_serializing_if` を付けると
    /// 「送っている」ことがワイヤ形の凍結（encode golden）から読めなくなる。
    /// この欄は「村の会話を接続先に保持させない」という主張を支える側なので、
    /// **出力に現れるほうが正しい**。
    ///
    /// Spec 31 の時点ではこの欄を 1 度も送っておらず、xAI 側の既定に委ねたまま
    /// **それが何かを測っていなかった**（Spec 34 の起票時に発見）。
    /// 200 で受けることは実測済み。
    pub store: bool,
    /// 最大出力トークン数。
    pub max_output_tokens: u32,
    /// サンプリング温度。`None` なら送らない（canonical の規律）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// ツール選択方針。`required` / `{type: function, name}` のときだけ送る。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<serde_json::Value>,
}

/// OpenAI Responses（`/v1/responses`）へのリクエスト（Spec 34）。
///
/// **`XaiRequest` と共有しない。** 同じエンドポイント名でも送る側の欄が違う
/// （契約 `openai_responses` / Spec 34 D2 rev6）:
/// - `include` は**送らない** — stateless では `encrypted_content` が既定で
///   返るので要求の必要が無い。xAI は `["no_inline_citations"]` を常送する
/// - `reasoning` を**常送する** — xAI は要約が既定で来るので送らない
/// - **`temperature` の欄が存在しない** — gpt-5.6 系は 400
///   `Unsupported parameter: 'temperature'` を返す（実測。Spec 34 D11）。
///   **モデル名で送り分けるより、欄を持たないほうが強い**（送れる形が無い）
#[derive(Debug, Serialize)]
pub struct OpenAiResponsesRequest {
    /// モデル名。
    pub model: String,
    /// 会話とツール往復の混在列。**要素の型は xAI と共有する**（実測で同形）。
    pub input: Vec<ResponsesInputItem>,
    /// 空なら欄ごと省く。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ResponsesTool>,
    /// 思考の制御。契約により**常に送る**。
    pub reasoning: OpenAiReasoning,
    /// 応答を接続先に保持させるか。**契約により常に `false`**（Spec 34 D3）。
    pub store: bool,
    /// 最大出力トークン数。**この欄名の 1 名だけ** —
    /// `max_completion_tokens` を送ると `failures.md` #76 の逆向きを踏む。
    pub max_output_tokens: u32,
    /// ツール選択方針。`required` / `{type: function, name}` のときだけ送る。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<serde_json::Value>,
}

/// `reasoning` オブジェクト（Spec 34 D4）。
#[derive(Debug, Serialize)]
pub struct OpenAiReasoning {
    /// 推論の深さ。canonical が未指定なら送らない。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<&'static str>,
    /// 要約の粒度。**契約により常に `"detailed"`**。
    ///
    /// **`"auto"` は要約を 1 字も返さない**（実測 0 字 / 522 字）。
    /// auto を選ぶと「送っているのに何も出ない」という最も診断しにくい形になる
    /// （`reasoning_tokens` は出ているので払ってはいる）。
    pub summary: &'static str,
    /// 思考の参照範囲。**契約により常に `"current_turn"`**。
    ///
    /// gpt-5.6 系の既定は `all_turns` だが、この村は `previous_response_id` を
    /// 持たず思考を履歴へも積まない（`reasoning_summary` 凍結 1）ので
    /// **参照する先が存在しない**。**省略は「無効」ではなく「サーバ既定を採る」**
    /// （`failures.md` #77 の教訓の 2 例目）。
    pub context: &'static str,
    /// 実行モード。`Some("pro")` のときだけ送る（トグル）。
    ///
    /// **送らないのと `"standard"` は `input_tokens` が完全に一致する**
    /// （実測 20 / 20）ので、省略 ≡ standard。だから 2 状態で足りる。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<&'static str>,
}

/// `input` の 1 要素。メッセージと関数往復が同じ列に混在する。
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum ResponsesInputItem {
    /// 通常の発話。`type` 欄は省略可能（role で判別される）。
    Message {
        /// "system" / "user" / "assistant"。
        role: &'static str,
        /// 本文。
        content: String,
    },
    /// 履歴として再送する assistant の関数呼び出し。
    FunctionCall {
        /// 常に `"function_call"`。
        #[serde(rename = "type")]
        kind: &'static str,
        /// 呼び出し ID（応答の `call_id` をそのまま返す）。
        call_id: String,
        /// 関数名。
        name: String,
        /// JSON 文字列（canonical の `args` オブジェクトを encode 時に文字列化）。
        arguments: String,
    },
    /// 関数実行の結果。
    FunctionCallOutput {
        /// 常に `"function_call_output"`。
        #[serde(rename = "type")]
        kind: &'static str,
        /// 対応する呼び出しの ID。
        call_id: String,
        /// 実行結果の本文。
        output: String,
    },
}

/// `tools` の 1 要素。サーバー側ツールと関数ツールが同居できる（実測 2026-08-10）。
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum ResponsesTool {
    /// サーバー側ツール（`web_search` / `x_search`）。
    Server {
        /// `"web_search"` / `"x_search"`。
        #[serde(rename = "type")]
        kind: &'static str,
    },
    /// 関数ツール。OpenAI 互換層と違い、`function` の入れ子ではなく flat。
    Function {
        /// 常に `"function"`。
        #[serde(rename = "type")]
        kind: &'static str,
        /// 関数名。
        name: String,
        /// モデルへ渡す説明。
        description: String,
        /// 引数の JSON Schema。
        parameters: serde_json::Value,
    },
}

/// xAI Responses の応答。
#[derive(Debug, Deserialize)]
pub struct ResponsesResponse {
    /// 応答の本体列。message / reasoning / 検索呼び出し / function_call が interleave する。
    #[serde(default)]
    pub output: Vec<ResponsesOutputItem>,
    /// "completed" / "incomplete" など。
    #[serde(default)]
    pub status: Option<String>,
    /// `status: "incomplete"` のときの理由。
    #[serde(default)]
    pub incomplete_details: Option<ResponsesIncompleteDetails>,
    /// 使用量。返さない応答でも壊れないよう `Option`。
    #[serde(default)]
    pub usage: Option<ResponsesUsage>,
}

#[derive(Debug, Deserialize)]
/// 打ち切りの理由（`max_output_tokens` なら Length へ写す）。
pub struct ResponsesIncompleteDetails {
    /// 理由の文字列。
    #[serde(default)]
    pub reason: Option<String>,
}

/// `output` の 1 要素。
///
/// `type` タグはあるが、腕を enum で閉じると未知種別で応答全体の parse が落ちる。
/// Gemini の part と同じく**全フィールド Option の構造体**で受け、`kind` 文字列で
/// 分岐する（未知種別は数えてから捨てる — #72）。
#[derive(Debug, Deserialize)]
pub struct ResponsesOutputItem {
    /// 種別。message / reasoning / web_search_call / x_search_call / custom_tool_call / function_call ほか。
    #[serde(rename = "type")]
    pub kind: String,
    /// `message` のとき。テキスト片と出典の列。
    #[serde(default)]
    pub content: Option<Vec<ResponsesContentPart>>,
    /// `function_call` のとき。呼び出し ID。
    #[serde(default)]
    pub call_id: Option<String>,
    /// `function_call` のとき。関数名。
    #[serde(default)]
    pub name: Option<String>,
    /// `function_call` のとき。JSON 文字列（decode 境界で 1 回だけ解く）。
    #[serde(default)]
    pub arguments: Option<String>,
    /// 検索呼び出しのとき。`query` が検索語を運ぶ（`web_search_call` の形）。
    #[serde(default)]
    pub action: Option<ResponsesSearchAction>,
    /// `custom_tool_call`（x_search の実体）のとき、引数の **JSON 文字列**。
    /// `{"query": "...", "limit": "5"}` の形で検索語が入る。`action` は無い。
    #[serde(default)]
    pub input: Option<String>,
    /// `reasoning` のとき、思考の要約（Spec 33）。
    ///
    /// **既定で入る** — `reasoning: {"summary":"auto"}` を送っても分布は変わらない
    /// （実測 4 対 4）。中身は 2 形あり、短形（129 字 = 問いの再掲だけ）と
    /// 長形（919〜1,367 字）。**どちらも「空」ではない**ので長さで切らない
    /// （`reasoning_summary` 契約の凍結 2）。
    #[serde(default)]
    pub summary: Option<Vec<ResponsesSummaryPart>>,
}

/// `reasoning` item の要約 1 片。
#[derive(Debug, Deserialize)]
pub struct ResponsesSummaryPart {
    /// 片の種別。読むのは `summary_text` だけ。
    #[serde(rename = "type")]
    pub kind: String,
    /// 要約の本文。**英語で返る**（問いが日本語でも。実測）。
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Deserialize)]
/// `message` の中身 1 片（`output_text` など）。
pub struct ResponsesContentPart {
    /// 片の種別。読むのは `output_text` だけ。
    #[serde(rename = "type")]
    pub kind: String,
    /// 本文。
    #[serde(default)]
    pub text: Option<String>,
    /// 出典（`url_citation`）。
    #[serde(default)]
    pub annotations: Vec<ResponsesAnnotation>,
}

/// 出典 1 件。`no_inline_citations` では位置（start/end_index）が全部 0 なので
/// 位置は受けない — 印の座標であって主張の座標ではない（契約）。
#[derive(Debug, Deserialize)]
pub struct ResponsesAnnotation {
    /// 種別。読むのは `url_citation` だけ。
    #[serde(rename = "type")]
    pub kind: String,
    /// 出典 URL。
    #[serde(default)]
    pub url: Option<String>,
    /// 表題。URL の複製で返ることがある（decode 側が空へ落とす）。
    #[serde(default)]
    pub title: Option<String>,
}

/// 検索呼び出しの中身。`query` が実際の検索語（`Grounding.queries` の原料）。
#[derive(Debug, Deserialize)]
pub struct ResponsesSearchAction {
    /// 行為の種別。**OpenAI では `search` / `open_page` / `find_in_page` の 3 種**で、
    /// 後 2 者は検索語を持たない（Spec 34 D9）。**`queries < calls` は正常**で、
    /// Spec 31 が xAI で使った `queries == calls` の等式をこちらへ写すと、
    /// ページを開いただけの周がある応答が常に不合格に見える。
    ///
    /// xAI の `web_search_call` では読まない（あちらは `search` しか出ない）。
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    /// 検索語。
    #[serde(default)]
    pub query: Option<String>,
}

/// 使用量。回数の欄は観測名（`server_side_tool_usage_details`）と公式文書名
/// （`server_side_tool_usage`）の**どちらで来ても読める**ようにする（契約）。
#[derive(Debug, Deserialize)]
pub struct ResponsesUsage {
    /// 入力トークン数（検索結果の注入込み）。
    #[serde(default)]
    pub input_tokens: u64,
    /// 出力トークン数（reasoning 込み）。
    #[serde(default)]
    pub output_tokens: u64,
    /// 入力トークンの内訳。
    #[serde(default)]
    pub input_tokens_details: Option<ResponsesInputTokensDetails>,
    /// 出力トークンの内訳。思考ぶんはここにしか出ない（Spec 32）。
    #[serde(default)]
    pub output_tokens_details: Option<ResponsesOutputTokensDetails>,
    /// 補助の生値。単位が未検証なので換算しない（契約）。
    #[serde(default)]
    pub cost_in_usd_ticks: Option<u64>,
    /// 回数の欄（観測名）。
    #[serde(default)]
    pub server_side_tool_usage_details: Option<serde_json::Value>,
    /// 回数の欄（公式文書名）。
    #[serde(default)]
    pub server_side_tool_usage: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
/// 入力トークンの内訳。
pub struct ResponsesInputTokensDetails {
    /// キャッシュから読まれたトークン数。
    #[serde(default)]
    pub cached_tokens: u64,
}

#[derive(Debug, Deserialize)]
/// 出力トークンの内訳。
pub struct ResponsesOutputTokensDetails {
    /// 思考に使われたトークン数。**`output_tokens` の内数**
    /// （実測 8/8 で差は本文のトークン数だった。Spec 32 P0）。
    #[serde(default)]
    pub reasoning_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temperature_is_omitted_when_unset() {
        let req = OaiRequest {
            model: "gpt-4o".into(),
            messages: vec![OaiMessage::text(OaiRole::User, "hi")],
            temperature: None,
            max_tokens: Some(256),
            max_completion_tokens: None,
            tools: Vec::new(),
            tool_choice: None,
            reasoning_effort: None,
        };
        let json = serde_json::to_value(&req).unwrap();

        assert!(json.get("temperature").is_none(), "temperature キーごと省略される");
        assert!(json.get("tools").is_none(), "空の tools は送らない");
        assert!(json.get("reasoning_effort").is_none());
        assert!(json.get("max_completion_tokens").is_none(), "未使用側の上限欄は送らない");
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

    /// 本文以外のブロックが来ても壊れず、**種別を取り違えない**。
    ///
    /// `thinking` は 2026-08-06 に `Other` から専用の枝へ移した — 落とすのは
    /// 同じだが、**落とした種別をログへ出す**ために名前が要る（実機で
    /// 「出力トークンだけがあって本文が無い」ターンの原因が読めなかった）。
    #[test]
    fn anthropic_non_text_blocks_are_typed_not_lumped_together() {
        let raw = r#"{"content":[
            {"type":"thinking","thinking":"..."},
            {"type":"redacted_thinking","data":"..."},
            {"type":"some_future_block","x":1},
            {"type":"text","text":"ok"}
        ]}"#;
        let resp: AnthropicResponse = serde_json::from_str(raw).expect("未知ブロックを許容すること");

        assert_eq!(resp.content.len(), 4);
        // Spec 33: `thinking` の本文まで受ける（要約の受け取り）。
        assert!(matches!(
            &resp.content[0],
            AnthropicContentBlock::Thinking { thinking } if thinking == "..."
        ));
        assert!(matches!(resp.content[1], AnthropicContentBlock::RedactedThinking));
        // **本当に未知のものだけが `Other`。** thinking を一緒くたにすると、
        // ログの `kinds=` が「unknown」しか言えなくなる。
        assert!(matches!(resp.content[2], AnthropicContentBlock::Other));
        assert!(matches!(&resp.content[3], AnthropicContentBlock::Text { text } if text == "ok"));
    }
}
