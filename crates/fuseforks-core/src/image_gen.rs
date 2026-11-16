//! 画像生成のワイヤ（Spec 76 / `image_gen_contract`）。
//!
//! **LLM のワイヤ（`llm/`）とは別の口。** 会話の canonical を通らず、1 回の要求で 1 枚を返す。
//! 要求と応答の形・縦横比と解像度の写像・失敗の分類は Lorekeel（Kataribe の `image_gen.rs`）から
//! 写した。写さなかったもの（ComfyUI・参照のストック・履歴）は Spec 76 D11。
//!
//! ここにあるのは 3 つ —
//!
//! 1. **純関数**: 写像（[`openai_size`] ほか）・URL・要求の本文（`encode_*`）・応答の解釈（`decode_*`）・
//!    状態コードの分類（[`classify_status`]）
//! 2. **HTTP**: [`generate`]（打ち切りは要求ごと切る — 契約の不変条件 8）と [`probe`]（画像を作らない GET）
//! 3. **失敗の閉じた 8 区分**（[`ImageGenFailure`]）
//!
//! **村の言語の文はここで書かない**（`failures.md` #149 — 言語を知らない層が文を書かない）。
//! 失敗の `detail` は接続先の原文か、機械の短い説明（英語）だけで、利用者とモデルが読む文は
//! ツール（`tools::image`）が村の言語で組む。

use std::time::{Duration, Instant};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::llm::{PromptAttachment, PromptMediaType};

/// 失敗の `detail` に残す接続先の本文の上限（文字）。
const DETAIL_MAX_CHARS: usize = 300;

/// 接続確認のタイムアウト（Lorekeel と同じ 20 秒）。
pub const PROBE_TIMEOUT_SECS: u64 = 20;

/// タイムアウトの下限（秒）。手で 0 や 1 を書いても、TLS の往復だけで切れる値にしない。
pub const MIN_TIMEOUT_SECS: u64 = 5;

/// 画像生成の接続先（アプリ全体で 1 つを選ぶ。Spec 76 D1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageProvider {
    /// OpenAI Images（generations / edits）。
    Openai,
    /// Gemini（generateContent の画像出力）。
    Gemini,
    /// Meta（Responses の `image_generation` ツール）。
    Meta,
    /// xAI（images の generations / edits）。
    Xai,
}

impl ImageProvider {
    /// 全部（設定画面の並び順・枠の網羅検査）。
    pub const ALL: [Self; 4] = [Self::Openai, Self::Gemini, Self::Meta, Self::Xai];

    /// ログと設定ファイルの語。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Gemini => "gemini",
            Self::Meta => "meta",
            Self::Xai => "xai",
        }
    }

    /// 既定のモデル（Lorekeel の既定 = 安い側）。
    pub fn default_model(self) -> &'static str {
        match self {
            Self::Openai => "gpt-image-1-mini",
            Self::Gemini => "gemini-3.1-flash-lite-image",
            Self::Meta => "muse-image-1.0",
            Self::Xai => "grok-imagine-image-2.0",
        }
    }

    /// 既定の接続先。
    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::Openai => "https://api.openai.com/v1",
            Self::Gemini => "https://generativelanguage.googleapis.com/v1beta",
            Self::Meta => "https://api.meta.ai/v1",
            Self::Xai => "https://api.x.ai/v1",
        }
    }

    /// 既定のタイムアウト（秒。Lorekeel の値 — Meta の実測 10〜19 秒、xAI 9.8〜16.9 秒）。
    pub fn default_timeout_secs(self) -> u64 {
        match self {
            Self::Gemini => 90,
            Self::Openai | Self::Meta | Self::Xai => 120,
        }
    }

    /// 解像度の段が効くか。**Meta は効かない**（`size` が縦横比しか決めず、`quality` は黙って
    /// 無視される — Lorekeel の 2026-10-05 の実測。Spec 76 D4）。
    pub fn honors_resolution(self) -> bool {
        !matches!(self, Self::Meta)
    }
}

/// 縦横比（3 値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageAspect {
    /// 正方形。
    #[default]
    Square,
    /// 横長。
    Landscape,
    /// 縦長。
    Portrait,
}

impl ImageAspect {
    /// ログの語（ツールの引数と同じ）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Square => "square",
            Self::Landscape => "landscape",
            Self::Portrait => "portrait",
        }
    }

    /// ツールの引数の語から引く。知らない語は `None`。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "square" => Some(Self::Square),
            "landscape" => Some(Self::Landscape),
            "portrait" => Some(Self::Portrait),
            _ => None,
        }
    }
}

/// 解像度の段（3 段。**モデルに選ばせない** — 費用を決めるのは利用者。Spec 76 D2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageResolution {
    /// 標準（最も安い段）。
    #[default]
    Standard,
    /// 高い。
    High,
    /// 最も高い。
    Highest,
}

/// OpenAI の `size`（Meta も同じ語彙を縦横比として使う）。
pub fn openai_size(aspect: ImageAspect) -> &'static str {
    match aspect {
        ImageAspect::Square => "1024x1024",
        ImageAspect::Landscape => "1536x1024",
        ImageAspect::Portrait => "1024x1536",
    }
}

/// OpenAI の `quality`（消費が 16 倍動く — Lorekeel の実測 high 4160 tok / low 272 tok）。
pub fn openai_quality(resolution: ImageResolution) -> &'static str {
    match resolution {
        ImageResolution::Standard => "low",
        ImageResolution::High => "medium",
        ImageResolution::Highest => "high",
    }
}

/// Gemini と xAI の縦横比の語。
pub fn ratio(aspect: ImageAspect) -> &'static str {
    match aspect {
        ImageAspect::Square => "1:1",
        ImageAspect::Landscape => "16:9",
        ImageAspect::Portrait => "9:16",
    }
}

/// Gemini の `imageSize`。
pub fn gemini_size(resolution: ImageResolution) -> &'static str {
    match resolution {
        ImageResolution::Standard => "1K",
        ImageResolution::High | ImageResolution::Highest => "2K",
    }
}

/// xAI の (`resolution`, `quality`)。`quality` の既定 `auto` は edits で medium になるので毎回明示する。
pub fn xai_resolution_quality(resolution: ImageResolution) -> (&'static str, &'static str) {
    match resolution {
        ImageResolution::Standard => ("1k", "low"),
        ImageResolution::High => ("2k", "low"),
        ImageResolution::Highest => ("2k", "medium"),
    }
}

/// 1 回の生成に使う設定（アプリ全体の設定から、選んでいる接続先の枠を取り出したもの）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageGenConfig {
    /// 接続先。
    pub provider: ImageProvider,
    /// モデル。空なら既定。
    pub model: String,
    /// 接続先の URL。空なら既定。
    pub base_url: String,
    /// タイムアウト（秒）。`None` なら既定。
    pub timeout_secs: Option<u64>,
    /// 解像度の段（設定の既定）。
    pub resolution: ImageResolution,
}

impl ImageGenConfig {
    /// 既定の枠（モデル・接続先・タイムアウトを空にしたもの）。
    pub fn new(provider: ImageProvider) -> Self {
        Self {
            provider,
            model: String::new(),
            base_url: String::new(),
            timeout_secs: None,
            resolution: ImageResolution::default(),
        }
    }

    /// 使うモデル。
    pub fn model_or_default(&self) -> &str {
        let model = self.model.trim();
        if model.is_empty() {
            self.provider.default_model()
        } else {
            model
        }
    }

    /// 使う接続先（末尾の `/` を落とす）。
    pub fn base(&self) -> &str {
        let base = self.base_url.trim().trim_end_matches('/');
        if base.is_empty() {
            self.provider.default_base_url()
        } else {
            base
        }
    }

    /// タイムアウト（下限 [`MIN_TIMEOUT_SECS`]）。
    pub fn timeout(&self) -> Duration {
        let secs = self
            .timeout_secs
            .unwrap_or_else(|| self.provider.default_timeout_secs());
        Duration::from_secs(secs.max(MIN_TIMEOUT_SECS))
    }
}

/// 接続先の URL のうち、版の段（`/v1` / `/v1beta`）を補った根。
fn versioned_base(config: &ImageGenConfig) -> String {
    let base = config.base();
    let version = match config.provider {
        ImageProvider::Gemini => "/v1beta",
        ImageProvider::Openai | ImageProvider::Meta | ImageProvider::Xai => "/v1",
    };
    if base.ends_with(version) {
        base.to_owned()
    } else {
        format!("{base}{version}")
    }
}

/// 生成の URL（参照の有無で口が分かれる社がある）。
pub fn generate_url(config: &ImageGenConfig, with_refs: bool) -> String {
    let root = versioned_base(config);
    match config.provider {
        ImageProvider::Openai | ImageProvider::Xai => {
            let path = if with_refs { "images/edits" } else { "images/generations" };
            format!("{root}/{path}")
        }
        ImageProvider::Gemini => {
            format!("{root}/models/{}:generateContent", config.model_or_default())
        }
        ImageProvider::Meta => format!("{root}/responses"),
    }
}

/// 接続確認の URL（画像を作らない GET。契約の不変条件 14）。
pub fn probe_url(config: &ImageGenConfig) -> String {
    let root = versioned_base(config);
    let model = config.model_or_default();
    match config.provider {
        ImageProvider::Openai => format!("{root}/models"),
        ImageProvider::Gemini | ImageProvider::Meta => format!("{root}/models/{model}"),
        ImageProvider::Xai => format!("{root}/image-generation-models/{model}"),
    }
}

/// 1 回の生成の依頼。
#[derive(Debug, Clone, Copy)]
pub struct ImageRequest<'a> {
    /// 描く内容。
    pub prompt: &'a str,
    /// 縦横比。
    pub aspect: ImageAspect,
    /// 参照画像（3 枚まで。並びは指定の順）。
    pub refs: &'a [PromptAttachment],
}

/// OpenAI の generations の本文（参照なし）。
pub fn encode_openai(config: &ImageGenConfig, request: &ImageRequest<'_>) -> Value {
    json!({
        "model": config.model_or_default(),
        "prompt": request.prompt,
        "size": openai_size(request.aspect),
        "quality": openai_quality(config.resolution),
        "n": 1,
        "output_format": "png",
    })
}

/// OpenAI の edits の本文（参照あり。multipart/form-data）。戻り値は (`Content-Type`, 本文)。
///
/// **依存を足さずに組む**（reqwest の `multipart` 機能は使わない）。区切りは呼び出し側が渡す
/// （テストで固定するため）。`input_fidelity` は送らない（Lorekeel の罠 — gpt-image-1-mini が 400）。
pub fn encode_openai_edit(
    config: &ImageGenConfig,
    request: &ImageRequest<'_>,
    boundary: &str,
) -> (String, Vec<u8>) {
    let engine = base64::engine::general_purpose::STANDARD;
    let mut body = Vec::new();
    for (index, reference) in request.refs.iter().enumerate() {
        let bytes = engine.decode(&reference.data).unwrap_or_default();
        let ext = match reference.media_type {
            PromptMediaType::Jpeg => "jpg",
            PromptMediaType::Webp => "webp",
            _ => "bin",
        };
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"image[]\"; filename=\"ref{}.{ext}\"\r\nContent-Type: {}\r\n\r\n",
                index + 1,
                reference.media_type.as_str()
            )
            .as_bytes(),
        );
        body.extend_from_slice(&bytes);
        body.extend_from_slice(b"\r\n");
    }
    for (key, value) in [
        ("prompt", request.prompt.to_owned()),
        ("model", config.model_or_default().to_owned()),
        ("size", openai_size(request.aspect).to_owned()),
        ("quality", openai_quality(config.resolution).to_owned()),
        ("n", "1".to_owned()),
    ] {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{key}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

/// Gemini の本文。参照は**テキストの前**に `inlineData` で並べる。
pub fn encode_gemini(config: &ImageGenConfig, request: &ImageRequest<'_>) -> Value {
    let mut parts: Vec<Value> = request
        .refs
        .iter()
        .map(|r| json!({ "inlineData": { "mimeType": r.media_type.as_str(), "data": r.data } }))
        .collect();
    parts.push(json!({ "text": request.prompt }));
    json!({
        "contents": [{ "role": "user", "parts": parts }],
        "generationConfig": {
            "responseModalities": ["IMAGE", "TEXT"],
            "imageConfig": {
                "aspectRatio": ratio(request.aspect),
                "imageSize": gemini_size(config.resolution),
            }
        }
    })
}

/// Meta の本文。**解像度は写さない**（`size` が縦横比だけを決める）。参照は `input_image` を
/// テキストの前に。`store: false`（送った内容を接続先に残さない側）。
pub fn encode_meta(config: &ImageGenConfig, request: &ImageRequest<'_>) -> Value {
    let mut content: Vec<Value> = request
        .refs
        .iter()
        .map(|r| json!({ "type": "input_image", "image_url": r.data_url() }))
        .collect();
    content.push(json!({ "type": "input_text", "text": request.prompt }));
    json!({
        "model": config.model_or_default(),
        "store": false,
        "stream": false,
        "input": [{ "type": "message", "role": "user", "content": content }],
        "tools": [{
            "type": "image_generation",
            "output_format": "webp",
            "size": openai_size(request.aspect),
            "reasoning_strength": "low",
        }],
    })
}

/// xAI の本文。参照ありは同じ形に `images` を足して edits の口へ。
pub fn encode_xai(config: &ImageGenConfig, request: &ImageRequest<'_>) -> Value {
    let (resolution, quality) = xai_resolution_quality(config.resolution);
    let mut body = json!({
        "model": config.model_or_default(),
        "prompt": request.prompt,
        "n": 1,
        "aspect_ratio": ratio(request.aspect),
        "resolution": resolution,
        "quality": quality,
        "response_format": "b64_json",
    });
    if !request.refs.is_empty() {
        body["images"] = Value::Array(
            request
                .refs
                .iter()
                .map(|r| json!({ "type": "image_url", "url": r.data_url() }))
                .collect(),
        );
    }
    body
}

/// 接続先が返した使用量・費用（欄があれば）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImageUsage {
    /// 入力トークン（OpenAI / Meta / Gemini）。
    pub input_tokens: Option<u64>,
    /// 出力トークン（同上）。
    pub output_tokens: Option<u64>,
    /// xAI の `cost_in_usd_ticks`（1e-10 USD）。
    pub cost_ticks: Option<u64>,
}

/// 生成できた 1 枚（**元のバイト列**。形式は中身から決める）。
#[derive(Debug, Clone)]
pub struct GeneratedImage {
    /// 元のバイト列。
    pub bytes: Vec<u8>,
    /// 使用量。
    pub usage: ImageUsage,
}

impl GeneratedImage {
    /// 中身から決めた拡張子（`png` / `jpg` / `webp`）。どれでもなければ `bin`。
    pub fn ext(&self) -> &'static str {
        sniff_ext(&self.bytes)
    }
}

/// 先頭のバイトから拡張子を決める（接続先の申告を信じない）。
pub fn sniff_ext(bytes: &[u8]) -> &'static str {
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "webp"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "jpg"
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else {
        "bin"
    }
}

/// 失敗の閉じた 8 区分（契約の不変条件 9。Lorekeel の `ImageGenError` と 1 対 1 — ComfyUI の 1 つを除く）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageGenFailure {
    /// 401 / 403（鍵）。
    Auth,
    /// 429。`detail` は本文の要点。
    RateLimited {
        /// 本文の要点。
        detail: String,
    },
    /// 接続先が生成を断った（Gemini の空の 200 / xAI の `respect_moderation: false` ほか）。
    Blocked {
        /// 接続先の理由の語。
        reason: String,
    },
    /// 既定のタイムアウトを超えた。
    Timeout,
    /// 繋がらない（DNS / 接続拒否 / TLS）。
    Network {
        /// 原因の連鎖。
        detail: String,
    },
    /// 応答の形が違う・画像が入っていない。
    Shape {
        /// 何が無かったか（機械の短い説明）と、あれば接続先の返事。
        detail: String,
    },
    /// 上のどれでもない 4xx / 5xx（200 で `error` が返ったときも）。
    Api {
        /// 状態コード。
        status: u16,
        /// 本文の先頭。
        detail: String,
    },
    /// 設定の誤り（生成の時の 404 = モデルが無い、ほか）。
    Config {
        /// 何が誤っているか。
        detail: String,
    },
}

impl ImageGenFailure {
    /// ログの `outcome=` の語（閉じた集合）。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::RateLimited { .. } => "rate_limited",
            Self::Blocked { .. } => "blocked",
            Self::Timeout => "timeout",
            Self::Network { .. } => "network",
            Self::Shape { .. } => "shape",
            Self::Api { .. } => "api",
            Self::Config { .. } => "config",
        }
    }
}

/// [`generate`] の失敗。打ち切りは 8 区分の外。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenerateError {
    /// ターンが打ち切られた（要求の future を落とした）。
    Interrupted,
    /// 8 区分のどれか。
    Failed(ImageGenFailure),
}

/// 文字境界を守って先頭を切る。
fn head(text: &str) -> String {
    text.chars().take(DETAIL_MAX_CHARS).collect()
}

/// 状態コードから分類する（2xx 以外のとき）。
pub fn classify_status(status: u16, body: &str) -> ImageGenFailure {
    match status {
        401 | 403 => ImageGenFailure::Auth,
        429 => ImageGenFailure::RateLimited {
            detail: rate_limit_detail(body),
        },
        404 => ImageGenFailure::Config {
            detail: format!("not found (404): {}", error_message(body)),
        },
        _ => ImageGenFailure::Api {
            status,
            detail: error_message(body),
        },
    }
}

/// 本文から `error.message` を取り出す。無ければ本文の先頭。
fn error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .map_or_else(|| head(body.trim()), |message| head(&message))
}

/// 429 の本文から要点を 1 行に絞る。Google は `Quota exceeded for metric: …` の行で
/// どのクォータに当たったかを名指しするので、あればその行。
fn rate_limit_detail(body: &str) -> String {
    if let Some(message) = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_owned))
    {
        if let Some(line) = message.lines().find(|l| l.contains("Quota exceeded")) {
            return head(line.trim_start_matches('*').trim());
        }
        return head(&message);
    }
    head(body.trim())
}

/// JSON として読む。読めなければ `Shape`。
fn parse(body: &str) -> Result<Value, ImageGenFailure> {
    serde_json::from_str(body).map_err(|err| ImageGenFailure::Shape {
        detail: format!("not JSON: {err}"),
    })
}

/// base64 を戻す。戻せなければ `Shape`。
fn decode_b64(data: &str, what: &str) -> Result<Vec<u8>, ImageGenFailure> {
    base64::engine::general_purpose::STANDARD
        .decode(data.trim())
        .map_err(|_| ImageGenFailure::Shape {
            detail: format!("{what} is not base64"),
        })
}

/// 200 で `error` が入っていたら `Api`。
fn embedded_error(v: &Value) -> Option<ImageGenFailure> {
    let err = v.get("error").filter(|e| !e.is_null())?;
    let message = err
        .get("message")
        .and_then(Value::as_str)
        .map_or_else(|| err.to_string(), str::to_owned);
    Some(ImageGenFailure::Api {
        status: 200,
        detail: head(&message),
    })
}

/// OpenAI 形の `usage.{input_tokens, output_tokens}`。
fn token_usage(v: &Value) -> ImageUsage {
    ImageUsage {
        input_tokens: v.pointer("/usage/input_tokens").and_then(Value::as_u64),
        output_tokens: v.pointer("/usage/output_tokens").and_then(Value::as_u64),
        cost_ticks: None,
    }
}

/// OpenAI の応答（`data[0].b64_json`）。
///
/// # Errors
/// 形が違えば `Shape`、200 で `error` が入っていれば `Api`。
pub fn decode_openai(body: &str) -> Result<GeneratedImage, ImageGenFailure> {
    let v = parse(body)?;
    if let Some(err) = embedded_error(&v) {
        return Err(err);
    }
    let data = v
        .pointer("/data/0/b64_json")
        .and_then(Value::as_str)
        .ok_or_else(|| ImageGenFailure::Shape {
            detail: "no data[0].b64_json".to_owned(),
        })?;
    Ok(GeneratedImage {
        bytes: decode_b64(data, "data[0].b64_json")?,
        usage: token_usage(&v),
    })
}

/// Gemini の応答（`candidates[0].content.parts[].inlineData`）。**断っても 200 で中身が空**なので、
/// `promptFeedback.blockReason` と候補の `finishReason`（STOP 以外）を拒否の理由にする。
///
/// # Errors
/// 拒否は `Blocked`、形が違えば `Shape`。
pub fn decode_gemini(body: &str) -> Result<GeneratedImage, ImageGenFailure> {
    let v = parse(body)?;
    if let Some(err) = embedded_error(&v) {
        return Err(err);
    }
    if let Some(reason) = v.pointer("/promptFeedback/blockReason").and_then(Value::as_str) {
        return Err(ImageGenFailure::Blocked {
            reason: reason.to_owned(),
        });
    }
    let usage = ImageUsage {
        input_tokens: v.pointer("/usageMetadata/promptTokenCount").and_then(Value::as_u64),
        output_tokens: v
            .pointer("/usageMetadata/candidatesTokenCount")
            .and_then(Value::as_u64),
        cost_ticks: None,
    };
    if let Some(parts) = v.pointer("/candidates/0/content/parts").and_then(Value::as_array) {
        for part in parts {
            if let Some(data) = part.pointer("/inlineData/data").and_then(Value::as_str) {
                return Ok(GeneratedImage {
                    bytes: decode_b64(data, "inlineData.data")?,
                    usage,
                });
            }
        }
    }
    if let Some(reason) = v.pointer("/candidates/0/finishReason").and_then(Value::as_str)
        && reason != "STOP"
    {
        return Err(ImageGenFailure::Blocked {
            reason: reason.to_owned(),
        });
    }
    Err(ImageGenFailure::Shape {
        detail: "no inlineData (only text came back)".to_owned(),
    })
}

/// Meta の応答（`output[]` の `image_generation_call.result`）。画像が無いときは `status` と
/// `incomplete_details` とモデルの返事を手がかりに載せる（拒否の形は未観測なので `Blocked` と決め打たない）。
///
/// # Errors
/// 形が違えば `Shape`、200 で `error` が入っていれば `Api`。
pub fn decode_meta(body: &str) -> Result<GeneratedImage, ImageGenFailure> {
    let v = parse(body)?;
    if let Some(err) = embedded_error(&v) {
        return Err(err);
    }
    let output = v.get("output").and_then(Value::as_array).cloned().unwrap_or_default();
    for item in &output {
        if item.get("type").and_then(Value::as_str) == Some("image_generation_call")
            && let Some(data) = item.get("result").and_then(Value::as_str).filter(|s| !s.is_empty())
        {
            return Ok(GeneratedImage {
                bytes: decode_b64(data, "image_generation_call.result")?,
                usage: token_usage(&v),
            });
        }
    }
    let status = v.get("status").and_then(Value::as_str).unwrap_or("?");
    let mut detail = format!("no image (status={status}");
    if let Some(reason) = v.pointer("/incomplete_details/reason").and_then(Value::as_str) {
        detail.push_str(&format!(", reason={reason}"));
    }
    detail.push(')');
    let said: Vec<String> = output
        .iter()
        .filter(|i| i.get("type").and_then(Value::as_str) == Some("message"))
        .flat_map(|i| i.get("content").and_then(Value::as_array).cloned().unwrap_or_default())
        .filter_map(|c| c.get("text").and_then(Value::as_str).map(str::to_owned))
        .collect();
    let said = said.join(" ");
    if !said.trim().is_empty() {
        detail.push_str(&format!(" model said: {}", head(said.trim())));
    }
    Err(ImageGenFailure::Shape { detail })
}

/// xAI の応答（`data[0].b64_json`）。`data[0].respect_moderation: false` は拒否
/// （xAI の文書の記述。Lorekeel でも未観測）。
///
/// # Errors
/// 拒否は `Blocked`、形が違えば `Shape`、200 で `error` が入っていれば `Api`。
pub fn decode_xai(body: &str) -> Result<GeneratedImage, ImageGenFailure> {
    let v = parse(body)?;
    if let Some(err) = embedded_error(&v) {
        return Err(err);
    }
    let item = v.pointer("/data/0");
    if item
        .and_then(|i| i.get("respect_moderation"))
        .and_then(Value::as_bool)
        == Some(false)
    {
        return Err(ImageGenFailure::Blocked {
            reason: "moderation".to_owned(),
        });
    }
    let data = item
        .and_then(|i| i.get("b64_json"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ImageGenFailure::Shape {
            detail: "no data[0].b64_json".to_owned(),
        })?;
    Ok(GeneratedImage {
        bytes: decode_b64(data, "data[0].b64_json")?,
        usage: ImageUsage {
            input_tokens: None,
            output_tokens: None,
            cost_ticks: v.pointer("/usage/cost_in_usd_ticks").and_then(Value::as_u64),
        },
    })
}

/// reqwest の失敗を分類する（タイムアウトか、繋がらないか）。
fn transport_failure(err: &reqwest::Error) -> ImageGenFailure {
    if err.is_timeout() {
        return ImageGenFailure::Timeout;
    }
    let mut detail = err.to_string();
    let mut cause = std::error::Error::source(err);
    while let Some(inner) = cause {
        detail.push_str(": ");
        detail.push_str(&inner.to_string());
        cause = inner.source();
    }
    ImageGenFailure::Network { detail: head(&detail) }
}

/// 鍵を載せた要求を組む（Gemini だけ `x-goog-api-key`）。
fn authorized(builder: reqwest::RequestBuilder, provider: ImageProvider, key: &str) -> reqwest::RequestBuilder {
    match provider {
        ImageProvider::Gemini => builder.header("x-goog-api-key", key),
        ImageProvider::Openai | ImageProvider::Meta | ImageProvider::Xai => builder.bearer_auth(key),
    }
}

/// 1 回送って、応答を解釈する（打ち切りは見ない）。
async fn send(
    http: &reqwest::Client,
    config: &ImageGenConfig,
    key: &str,
    request: &ImageRequest<'_>,
) -> Result<GeneratedImage, ImageGenFailure> {
    let with_refs = !request.refs.is_empty();
    let url = generate_url(config, with_refs);
    let builder = http.post(&url).timeout(config.timeout());
    let builder = match config.provider {
        ImageProvider::Openai if with_refs => {
            let boundary = format!("fuseforks-{}", uuid::Uuid::new_v4().simple());
            let (content_type, body) = encode_openai_edit(config, request, &boundary);
            builder.header(reqwest::header::CONTENT_TYPE, content_type).body(body)
        }
        ImageProvider::Openai => builder.json(&encode_openai(config, request)),
        ImageProvider::Gemini => builder.json(&encode_gemini(config, request)),
        ImageProvider::Meta => builder.json(&encode_meta(config, request)),
        ImageProvider::Xai => builder.json(&encode_xai(config, request)),
    };
    let response = authorized(builder, config.provider, key)
        .send()
        .await
        .map_err(|err| transport_failure(&err))?;
    let status = response.status().as_u16();
    let body = response
        .text()
        .await
        .map_err(|err| transport_failure(&err))?;
    if !(200..300).contains(&status) {
        return Err(classify_status(status, &body));
    }
    match config.provider {
        ImageProvider::Openai => decode_openai(&body),
        ImageProvider::Gemini => decode_gemini(&body),
        ImageProvider::Meta => decode_meta(&body),
        ImageProvider::Xai => decode_xai(&body),
    }
}

/// 1 枚生成する。**打ち切り（Spec 10）は要求ごと切る**（契約の不変条件 8 — Spec 52 D4 と逆）:
/// `cancel` が先に鳴ったら要求の future を落とし、応答を待たない。自動の再試行はしない。
///
/// # Errors
/// 打ち切りは [`GenerateError::Interrupted`]、それ以外は 8 区分。
pub async fn generate(
    http: &reqwest::Client,
    config: &ImageGenConfig,
    key: &str,
    request: &ImageRequest<'_>,
    cancel: Option<&tokio_util::sync::CancellationToken>,
) -> Result<GeneratedImage, GenerateError> {
    let fut = send(http, config, key, request);
    match cancel {
        Some(token) => {
            if token.is_cancelled() {
                return Err(GenerateError::Interrupted);
            }
            tokio::select! {
                biased;
                () = token.cancelled() => Err(GenerateError::Interrupted),
                result = fut => result.map_err(GenerateError::Failed),
            }
        }
        None => fut.await.map_err(GenerateError::Failed),
    }
}

/// 接続確認（画像を作らない GET を 1 本。契約の不変条件 14）。戻り値は往復の実測。
///
/// # Errors
/// 401 / 403 は `Auth`、Gemini・Meta・xAI の 404 は `Config`（モデル名）、それ以外は 8 区分。
pub async fn probe(
    http: &reqwest::Client,
    config: &ImageGenConfig,
    key: &str,
) -> Result<Duration, ImageGenFailure> {
    let started = Instant::now();
    let builder = http
        .get(probe_url(config))
        .timeout(Duration::from_secs(PROBE_TIMEOUT_SECS));
    let response = authorized(builder, config.provider, key)
        .send()
        .await
        .map_err(|err| transport_failure(&err))?;
    let status = response.status().as_u16();
    if (200..300).contains(&status) {
        return Ok(started.elapsed());
    }
    let body = response.text().await.unwrap_or_default();
    Err(match (status, config.provider) {
        (404, ImageProvider::Openai) => classify_status(status, &body),
        (404, _) => ImageGenFailure::Config {
            detail: format!("model '{}' not found (404)", config.model_or_default()),
        },
        _ => classify_status(status, &body),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(provider: ImageProvider) -> ImageGenConfig {
        ImageGenConfig::new(provider)
    }

    fn webp_ref() -> PromptAttachment {
        PromptAttachment::new(PromptMediaType::Webp, "UklGRg==")
    }

    #[test]
    fn urls_follow_lorekeel_and_fill_the_version_segment() {
        let openai = config(ImageProvider::Openai);
        assert_eq!(generate_url(&openai, false), "https://api.openai.com/v1/images/generations");
        assert_eq!(generate_url(&openai, true), "https://api.openai.com/v1/images/edits");
        assert_eq!(probe_url(&openai), "https://api.openai.com/v1/models");

        let gemini = config(ImageProvider::Gemini);
        assert_eq!(
            generate_url(&gemini, true),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.1-flash-lite-image:generateContent"
        );
        assert_eq!(
            probe_url(&gemini),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.1-flash-lite-image"
        );

        let meta = config(ImageProvider::Meta);
        assert_eq!(generate_url(&meta, false), "https://api.meta.ai/v1/responses");
        assert_eq!(probe_url(&meta), "https://api.meta.ai/v1/models/muse-image-1.0");

        let xai = config(ImageProvider::Xai);
        assert_eq!(generate_url(&xai, true), "https://api.x.ai/v1/images/edits");
        assert_eq!(
            probe_url(&xai),
            "https://api.x.ai/v1/image-generation-models/grok-imagine-image-2.0"
        );

        // ホスト直を書いた接続先には版の段を補い、末尾の `/` は落とす。
        let mut custom = config(ImageProvider::Xai);
        custom.base_url = "http://127.0.0.1:9000/".to_owned();
        assert_eq!(generate_url(&custom, false), "http://127.0.0.1:9000/v1/images/generations");
    }

    #[test]
    fn timeouts_are_per_provider_with_a_floor() {
        assert_eq!(config(ImageProvider::Gemini).timeout(), Duration::from_secs(90));
        assert_eq!(config(ImageProvider::Openai).timeout(), Duration::from_secs(120));
        let mut tiny = config(ImageProvider::Meta);
        tiny.timeout_secs = Some(1);
        assert_eq!(tiny.timeout(), Duration::from_secs(MIN_TIMEOUT_SECS));
    }

    #[test]
    fn meta_carries_only_the_aspect() {
        let mut meta = config(ImageProvider::Meta);
        meta.resolution = ImageResolution::Highest;
        let request = ImageRequest {
            prompt: "cat",
            aspect: ImageAspect::Landscape,
            refs: &[],
        };
        let body = encode_meta(&meta, &request);
        let tool = &body["tools"][0];
        assert_eq!(tool["size"], "1536x1024");
        assert!(tool.get("quality").is_none(), "Meta に解像度を写さない");
        assert_eq!(body["store"], false);
        assert!(!ImageProvider::Meta.honors_resolution());
    }

    #[test]
    fn references_go_before_the_text() {
        let refs = [webp_ref()];
        let request = ImageRequest {
            prompt: "make it green",
            aspect: ImageAspect::Square,
            refs: &refs,
        };
        let gemini = encode_gemini(&config(ImageProvider::Gemini), &request);
        assert_eq!(gemini["contents"][0]["parts"][0]["inlineData"]["mimeType"], "image/webp");
        assert_eq!(gemini["contents"][0]["parts"][1]["text"], "make it green");
        let meta = encode_meta(&config(ImageProvider::Meta), &request);
        assert_eq!(
            meta["input"][0]["content"][0]["image_url"],
            "data:image/webp;base64,UklGRg=="
        );
        let xai = encode_xai(&config(ImageProvider::Xai), &request);
        assert_eq!(xai["images"][0]["url"], "data:image/webp;base64,UklGRg==");
        assert_eq!(xai["quality"], "low", "quality は毎回明示する");
    }

    #[test]
    fn openai_edit_is_multipart_without_input_fidelity() {
        let refs = [webp_ref()];
        let request = ImageRequest {
            prompt: "make it green",
            aspect: ImageAspect::Portrait,
            refs: &refs,
        };
        let (content_type, body) =
            encode_openai_edit(&config(ImageProvider::Openai), &request, "BOUNDARY");
        assert_eq!(content_type, "multipart/form-data; boundary=BOUNDARY");
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("name=\"image[]\"; filename=\"ref1.webp\"\r\nContent-Type: image/webp"));
        assert!(text.contains("name=\"size\"\r\n\r\n1024x1536"));
        assert!(text.contains("name=\"quality\"\r\n\r\nlow"));
        assert!(!text.contains("input_fidelity"));
        assert!(text.ends_with("--BOUNDARY--\r\n"));
    }

    #[test]
    fn status_codes_map_to_the_closed_classes() {
        assert_eq!(classify_status(401, ""), ImageGenFailure::Auth);
        assert_eq!(classify_status(403, ""), ImageGenFailure::Auth);
        assert_eq!(classify_status(429, "").as_str(), "rate_limited");
        assert_eq!(classify_status(404, "").as_str(), "config");
        assert_eq!(
            classify_status(500, r#"{"error":{"message":"boom"}}"#),
            ImageGenFailure::Api {
                status: 500,
                detail: "boom".to_owned()
            }
        );
        // Google の 429 はクォータの行を名指しする。
        let google = r#"{"error":{"message":"* Quota exceeded for metric: generate_content_free_tier_requests, limit: 0\nother"}}"#;
        assert_eq!(
            classify_status(429, google),
            ImageGenFailure::RateLimited {
                detail: "Quota exceeded for metric: generate_content_free_tier_requests, limit: 0".to_owned()
            }
        );
    }

    #[test]
    fn decoders_read_the_image_and_the_usage() {
        let png = base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\nrest");
        let openai = decode_openai(&format!(
            r#"{{"data":[{{"b64_json":"{png}"}}],"usage":{{"input_tokens":38,"output_tokens":272}}}}"#
        ))
        .unwrap();
        assert_eq!(openai.ext(), "png");
        assert_eq!(openai.usage.output_tokens, Some(272));

        let gemini = decode_gemini(&format!(
            r#"{{"candidates":[{{"content":{{"parts":[{{"text":"x"}},{{"inlineData":{{"mimeType":"image/png","data":"{png}"}}}}]}},"finishReason":"STOP"}}],"usageMetadata":{{"promptTokenCount":29,"candidatesTokenCount":1526}}}}"#
        ))
        .unwrap();
        assert_eq!(gemini.usage.input_tokens, Some(29));

        let meta = decode_meta(&format!(
            r#"{{"output":[{{"type":"image_generation_call","result":"{png}"}}],"usage":{{"input_tokens":4031,"output_tokens":919}}}}"#
        ))
        .unwrap();
        assert_eq!(meta.usage.input_tokens, Some(4031));

        let xai = decode_xai(&format!(
            r#"{{"data":[{{"b64_json":"{png}"}}],"usage":{{"cost_in_usd_ticks":400000000}}}}"#
        ))
        .unwrap();
        assert_eq!(xai.usage.cost_ticks, Some(400_000_000));
    }

    #[test]
    fn refusals_are_blocked_not_shape() {
        assert_eq!(
            decode_gemini(r#"{"promptFeedback":{"blockReason":"SAFETY"}}"#).unwrap_err(),
            ImageGenFailure::Blocked {
                reason: "SAFETY".to_owned()
            }
        );
        assert_eq!(
            decode_gemini(r#"{"candidates":[{"content":{"parts":[{"text":"no"}]},"finishReason":"IMAGE_SAFETY"}]}"#)
                .unwrap_err()
                .as_str(),
            "blocked"
        );
        assert_eq!(
            decode_xai(r#"{"data":[{"b64_json":"","respect_moderation":false}]}"#).unwrap_err(),
            ImageGenFailure::Blocked {
                reason: "moderation".to_owned()
            }
        );
        // 画像の無い STOP は拒否ではなく形の違い。
        assert_eq!(
            decode_gemini(r#"{"candidates":[{"content":{"parts":[{"text":"hi"}]},"finishReason":"STOP"}]}"#)
                .unwrap_err()
                .as_str(),
            "shape"
        );
        // Meta は拒否の形が未観測なので、手がかりを載せて shape。
        let meta = decode_meta(
            r#"{"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"I can't"}]}]}"#,
        )
        .unwrap_err();
        assert_eq!(meta.as_str(), "shape");
        assert!(matches!(meta, ImageGenFailure::Shape { ref detail } if detail.contains("I can't")));
    }

    #[test]
    fn an_error_inside_a_200_is_api() {
        assert_eq!(
            decode_meta(r#"{"error":{"message":"bad"}}"#).unwrap_err(),
            ImageGenFailure::Api {
                status: 200,
                detail: "bad".to_owned()
            }
        );
    }

    #[test]
    fn the_ext_comes_from_the_bytes() {
        assert_eq!(sniff_ext(b"\xFF\xD8\xFF\xE0"), "jpg");
        assert_eq!(sniff_ext(b"RIFF\0\0\0\0WEBPVP8 "), "webp");
        assert_eq!(sniff_ext(b"\x89PNG\r\n\x1a\n"), "png");
        assert_eq!(sniff_ext(b"GIF89a"), "bin");
    }

    #[test]
    fn failure_labels_are_the_closed_eight() {
        let labels: Vec<&str> = [
            ImageGenFailure::Auth,
            ImageGenFailure::RateLimited { detail: String::new() },
            ImageGenFailure::Blocked { reason: String::new() },
            ImageGenFailure::Timeout,
            ImageGenFailure::Network { detail: String::new() },
            ImageGenFailure::Shape { detail: String::new() },
            ImageGenFailure::Api { status: 500, detail: String::new() },
            ImageGenFailure::Config { detail: String::new() },
        ]
        .iter()
        .map(ImageGenFailure::as_str)
        .collect();
        assert_eq!(
            labels,
            ["auth", "rate_limited", "blocked", "timeout", "network", "shape", "api", "config"]
        );
    }
}
