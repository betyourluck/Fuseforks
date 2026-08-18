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

use crate::attachment::{AttachmentFormat, AttachmentKind};

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

/// 添付のワイヤ上の形式（Spec 23 = 画像 / Spec 36 = 音声・動画・PDF。
/// 契約は `attachment_contract`）。
///
/// **[`crate::attachment::AttachmentFormat`]（ディスク上の形式）とは別の集合。**
/// こちらには [`Self::Jpeg`] が居る — 互換層が WebP を拒んだときの
/// 1 回だけの再送（Spec 23 D3）で作られる形で、**ファイルには決して存在しない**。
/// 「置ける形」と「送れる形」を 1 つの型に畳むと、その非対称が消える。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptMediaType {
    /// WebP（画像の既定。P0 実測で 5 系統すべて通過）。
    Webp,
    /// JPEG（互換層が WebP を 400 で拒んだときのフォールバック。**ワイヤ専用**）。
    Jpeg,
    /// mp3（音声）。
    Mp3,
    /// wav（音声）。
    Wav,
    /// mp4（動画）。
    Mp4,
    /// PDF。
    Pdf,
}

impl PromptMediaType {
    /// ワイヤに載せる MIME 文字列。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Webp => "image/webp",
            Self::Jpeg => "image/jpeg",
            Self::Mp3 => "audio/mpeg",
            Self::Wav => "audio/wav",
            Self::Mp4 => "video/mp4",
            Self::Pdf => "application/pdf",
        }
    }

    /// 種別（`carries` の引数になる粒度）。
    pub fn kind(self) -> AttachmentKind {
        match self {
            Self::Webp | Self::Jpeg => AttachmentKind::Image,
            Self::Mp3 | Self::Wav => AttachmentKind::Audio,
            Self::Mp4 => AttachmentKind::Video,
            Self::Pdf => AttachmentKind::Pdf,
        }
    }

    /// 互換層が使う短い形式名（`input_audio` の `format` 欄）。
    ///
    /// MIME ではなく `"wav"` / `"mp3"` の裸の語を要求するのは OpenAI 互換の
    /// 方言（実測 2026-08-12）。**音声以外では意味を持たない**ので `Option`。
    pub fn audio_format(self) -> Option<&'static str> {
        match self {
            Self::Mp3 => Some("mp3"),
            Self::Wav => Some("wav"),
            _ => None,
        }
    }
}

impl From<AttachmentFormat> for PromptMediaType {
    /// ディスク上の形式からワイヤの形式へ。**逆向きは無い**
    /// （JPEG はワイヤにしか存在しないので、全射だが単射ではない）。
    fn from(format: AttachmentFormat) -> Self {
        match format {
            AttachmentFormat::Webp => Self::Webp,
            AttachmentFormat::Mp3 => Self::Mp3,
            AttachmentFormat::Wav => Self::Wav,
            AttachmentFormat::Mp4 => Self::Mp4,
            AttachmentFormat::Pdf => Self::Pdf,
        }
    }
}

/// この発話と一緒にモデルへ渡す添付（Spec 23 / Spec 36）。
///
/// **`data` は base64 済みの文字列。** adapter は純関数（ネットワークも
/// ファイルも知らない）なので、実体の読み出しと base64 化は
/// プロンプトを組む側（orchestrator）の責務。
///
/// **`file_name` を持つのは PDF のため** — `input_file` / `file` part は
/// ファイル名を要求する社があり、省くと 400 になる。画像・音声では使わない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptAttachment {
    /// ワイヤ上の形式。
    pub media_type: PromptMediaType,
    /// base64 エンコード済みのデータ。
    pub data: String,
    /// 元のファイル名（`input_file` / `file` part 用。既定は空）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file_name: String,
}

impl PromptAttachment {
    /// 形式とデータだけの添付（画像・音声・動画。ファイル名を使わない社向け）。
    pub fn new(media_type: PromptMediaType, data: impl Into<String>) -> Self {
        Self {
            media_type,
            data: data.into(),
            file_name: String::new(),
        }
    }

    /// 種別。
    pub fn kind(&self) -> AttachmentKind {
        self.media_type.kind()
    }

    /// `data:` URL 形式（互換層の `image_url` と Responses の `input_file`）。
    pub fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.media_type.as_str(), self.data)
    }

    /// ファイル名（空なら種別に応じた既定を作る）。
    ///
    /// **空のまま送ると 400 になる社がある**ので、ここで必ず何かを返す。
    pub fn file_name_or_default(&self) -> String {
        if self.file_name.is_empty() {
            format!("attachment.{}", self.media_type.audio_format().unwrap_or("pdf"))
        } else {
            self.file_name.clone()
        }
    }
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
    /// この発話と一緒に渡す添付（Spec 23 / Spec 36。通常は空）。
    ///
    /// **1 ターン限り**（D1）— プロンプトを組む側が現在の発話にだけ付け、
    /// 履歴として積む [`ChatMessage`] には入れない。`skip_serializing_if` により、
    /// 添付が無い発話のシリアライズ結果は欄の追加前と 1 バイトも変わらない。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<PromptAttachment>,
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
            attachments: Vec::new(),
        }
    }

    /// 添付画像つきのユーザーメッセージを作る（Spec 23）。
    pub fn user_with_attachments(
        content: impl Into<String>,
        attachments: Vec<PromptAttachment>,
    ) -> Self {
        Self {
            attachments,
            ..Self::plain(Role::User, content)
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
    /// キャッシュから読まれた入力トークン数。**[`Self::prompt`] の内数。**
    pub cache_read: u64,
    /// キャッシュへ**書き込まれた**入力トークン数。**[`Self::prompt`] の内数**（Spec 40）。
    ///
    /// **[`Self::cache_read`] と重ならない** — 入力は「素の未キャッシュ + 読み取り +
    /// 書き込み」の 3 つに割れ、`prompt` はその合計。素の未キャッシュは
    /// `prompt - cache_read - cache_write` で引く。
    ///
    /// **読み取りと分けて持つのは、課金の倍率が違うから。** Anthropic は読み取り
    /// 0.1× に対し書き込みは TTL 依存（5 分 1.25× / 1 時間 2.0×）で、**この村は
    /// `CACHE_TTL = "1h"` を送っているので 2.0×**。畳むと「1.0× のもの」と
    /// 「2.0× のもの」が同じ袋に入り、外の単価表を当てたときに金額が狂う。
    ///
    /// **[`crate::budget`] の実効トークンはこの欄を 1.0× で数えている**（Spec 40 D3。
    /// 天井は「支払いの上限」ではなく「歯止めの単位」）。
    ///
    /// 取れないワイヤでは 0。**`Option` にしない**（[`Self::reasoning`] と同じ理由）。
    /// **「欄が無い」は観測ではない** — この村の decode は未知の欄を黙って落とすので、
    /// 相手が返していても 0 に見える。0 の意味は実測で決める。
    pub cache_write: u64,
    /// [`Self::cache_write`] のうち、**1 時間 TTL で書かれたぶん**（**部分集合**）。
    ///
    /// **5 分ぶんは持たない** — `cache_write - cache_write_1h` で引ける。3 つの数の
    /// うち独立なのは 2 つなので、3 欄持つと「片方だけ埋まって合計と食い違う」形が
    /// 書けてしまう（Spec 40 D6 rev3。符号化の出自は otari の `BillableUsage`）。
    ///
    /// 不変条件: `cache_write_1h <= cache_write`。内訳を返さないワイヤ・応答では 0
    /// （**合計だけが立つ**）。
    pub cache_write_1h: u64,
    /// うち思考（reasoning / thinking）に使われたトークン数。
    ///
    /// **[`Self::completion`] の内数**であって外数ではない（Spec 32 P0 凍結。
    /// 実測 8/8 で `completion - reasoning` が本文のトークン数と一致した）。
    /// **[`Self::total`] へ足さない** — 足すと二重計上になり、トークン天井
    /// （Spec 11）の実効値が跳ねる。思考ぶんは既に出力として ×4 で乗っている。
    ///
    /// 取れないプロバイダでは 0。**`Option` にしない** —「取れない」と
    /// 「0 だった」を型で分けると表示側が 2 状態を扱うことになり、
    /// 「思考を使わない経路でも 0 の行が出る」という対照が組めなくなる。
    pub reasoning: u64,
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
    /// どの機構が接地したか（Spec 31 D5）。表示のエンジン名はここから辞書で引く。
    #[serde(default)]
    pub engine: GroundingEngine,
}

/// 接地エンジンの閉じた列挙（Spec 31 D5）。
///
/// 自由文字列にしないのは役職の色と同じ理由 — 開いた値は表示層に分岐を漏らす。
/// serde 既定が [`GroundingEngine::Google`] なのは後方互換のため:
/// `engine` 欄の無い既存レコード（`sessions.redb` に保存済みの `AgentMessage`）は
/// すべて Spec 05 の Google 検索由来なので、既定で正しく読める。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroundingEngine {
    /// Gemini の Google 検索グラウンディング（Spec 05）。
    #[default]
    Google,
    /// xAI の Live Search（Agent Tools。Spec 31）。
    Xai,
    /// OpenAI の web 検索（Responses API のサーバー側ツール。Spec 34）。
    ///
    /// **ワイヤ値は `open_ai`**（`rename_all = "snake_case"` が `OpenAi` を割る）。
    /// `Provider` の `open_ai_compat` / `open_ai_responses` と同じ綴りで揃う。
    OpenAi,
    /// Meta の web 検索（Responses API のサーバー側ツール。Spec 37）。
    ///
    /// **出典の `start_index` / `end_index` が実区間で返る唯一のワイヤ**
    /// （xAI は 77/77 がすべて 0 = メッセージ単位）。この村は今のところ
    /// 区間を捨てているが、捨てたことを `meta_responses` の契約に書いてある。
    Meta,
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
        // engine は「最初に接地した側」を採る。1 ターンの provider は固定なので
        // 実際に食い違うことは無いが、空の器が既定値 (Google) のまま
        // xAI の記録を吸うと engine だけが噓になる。
        if self.is_empty() {
            self.engine = other.engine;
        }
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
    /// モデルが返した**思考の要約**（Spec 33）。1 応答に複数入りうるので列。
    ///
    /// **量は [`Usage::reasoning`]、中身はこちら。別の欄・別の口**
    /// （OpenAI 互換は `/v1/chat/completions` で数は取れるが本文は来ない）。
    ///
    /// **[`ChatMessage`] へは積まない**（`reasoning_summary` 契約の凍結 1）—
    /// 履歴へ入れると滑る窓 8 往復で最大 8 回再送され、実測 3,475 字が
    /// 固定費になる。**この型に席があって `ChatMessage` に無いこと自体が
    /// その保証**（Spec 23 D1 と同じ形で、型で閉じている）。
    ///
    /// **空文字は入れない** — 0 字は「要約が無い」であって表示するものが無い。
    /// 要約が付かない回があったことは `dropped content blocks:` の計器が
    /// 別に数えているので、ここを空で埋めても診断は増えない。
    pub reasoning_summary: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 接地エンジンのワイヤ値を固定する。
    ///
    /// **`OpenAi` は `snake_case` で `open_ai` になる**（`openai` ではない）。
    /// この値は `types.ts` の `GroundingEngine` と辞書 `grounding.engine.*` の
    /// 鍵に**そのまま使われる**ので、食い違うと画面に生の鍵が出る —
    /// 型検査にも lint にも掛からない種類の退行（`failures.md` #54 と同じ形で、
    /// あちらは実行時に組み立てる `var(--color-role-${色})` だった）。
    #[test]
    fn grounding_engine_wire_values_are_frozen() {
        for (engine, expected) in [
            (GroundingEngine::Google, r#""google""#),
            (GroundingEngine::Xai, r#""xai""#),
            (GroundingEngine::OpenAi, r#""open_ai""#),
        ] {
            assert_eq!(serde_json::to_string(&engine).unwrap(), expected);
        }
    }

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
            cache_write: 0,
            cache_write_1h: 0,
            reasoning: 0,
        };
        assert_eq!(usage.total(), 125);
    }

    /// 思考ぶんは `completion` の**内数**なので、合計に 1 つも足さない
    /// （Spec 32 D2）。足すと二重計上でトークン天井の実効値が跳ねる。
    ///
    /// **`reasoning` を `completion` と同じ大きさにして測る** — 外数の実装なら
    /// 合計が倍になるので、この 1 本で内数/外数のどちらで実装されたかが割れる。
    #[test]
    fn reasoning_is_inside_completion_and_never_added_to_total() {
        let usage = Usage {
            prompt: 100,
            completion: 25,
            cache_read: 80,
            cache_write: 0,
            cache_write_1h: 0,
            reasoning: 25,
        };
        assert_eq!(usage.total(), 125, "内数なので上のテストと同じ値でなければならない");
    }

    #[test]
    fn absorbing_grounding_merges_rounds_without_duplicates() {
        // 1 ターンは複数周に分かれる。検索した周と関数を呼んだ周が別なら、
        // 畳まない実装は片方を捨てる。
        let mut acc = Grounding::default();
        acc.absorb(Grounding {
            queries: vec!["ARIB B36".into()],
            engine: GroundingEngine::Google,
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
            engine: Default::default(),
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
            engine: Default::default(),
        });

        assert_eq!(acc.queries.len(), 1);
        assert_eq!(acc.sources.len(), 1);
    }
}
