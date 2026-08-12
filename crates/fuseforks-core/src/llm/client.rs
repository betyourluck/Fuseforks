//! HTTP クライアント核。プロバイダ判定・認証・再試行を持ち、形の翻訳は adapter に委ねる。
//!
//! 責務の線引き（Kataribe から継承）:
//! - **adapter**: canonical ⇄ wire の純関数。ネットワークを知らない。
//! - **client 核**: URL 組み立て・ヘッダ・タイムアウト・指数バックオフ再試行。wire の中身を知らない。
//!
//! この分離があると、プロバイダを 1 つ足す作業が adapter 1 ファイルと
//! [`Provider`] への 1 バリアント追加に閉じる。

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::canonical::{ChatRequest, ChatResponse, Effort};
use super::error::LlmError;
use super::{
    BackendResolution, LlmBackend, anthropic, gemini, openai_compat, openai_responses, wire,
    xai_responses,
};
use crate::model::{CredentialSource, ModelTemplate};
use crate::secret::SecretStore;

/// エラー応答本文をログ・UI に載せる際の最大文字数。
/// プロバイダによっては HTML を丸ごと返すため、青天井にしない。
const MAX_ERROR_BODY: usize = 2_000;

/// LLM の話し方（ワイヤプロトコル）。
///
/// OpenAI 互換層はプロンプトキャッシュの指示を通せないため、Anthropic へは
/// ネイティブ Messages API を使う。マルチエージェントでは同じシステムプロンプトを
/// エージェント数ぶん毎ターン送るので、キャッシュの有無が運用コストを直接左右する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    /// `POST {base_url}/chat/completions` + `Authorization: Bearer`。
    /// OpenAI / Grok / ローカル互換サーバ。
    OpenAiCompat,
    /// `POST {base_url}/messages` + `x-api-key` + `anthropic-version`。
    Anthropic,
    /// `POST {base_url}/models/{model}:generateContent` + `x-goog-api-key`。
    ///
    /// Gemini は OpenAI 互換の口も持っており、**関数呼び出しだけならそちらで足りる**。
    /// この経路が要るのは Google 検索による接地を使うときだけで、互換層は
    /// `google_search` を `400 Invalid tool type` で拒否する（実測 2026-07-29）。
    Gemini,
    /// `POST {base_url}/responses` + `Authorization: Bearer`（Spec 31）。
    ///
    /// xAI も OpenAI 互換の口を持っており、**関数呼び出しだけならそちらで足りる**。
    /// この経路が要るのは Grok の Live Search を使うときだけで、legacy の
    /// `search_parameters` は HTTP 410 Gone（実測 2026-08-09）。Gemini と同じ構図の 2 例目。
    XaiResponses,
    /// `POST {base_url}/responses` + `Authorization: Bearer`（Spec 34）。
    ///
    /// OpenAI も互換の口を持っており、**関数呼び出しだけならそちらで足りる**。
    /// この経路が要るのは 3 つ — 思考の**要約本文**（互換の口には来ない）/
    /// web 検索（一般の gpt-5 系では Responses 専用）/ `failures.md` #77 の代償
    /// （互換の口はツールと思考を併用できず `reasoning_effort: "none"` を
    /// 強制していた）。gemini / xai_responses と同じ構図の 3 例目。
    OpenAiResponses,
}

impl Provider {
    /// base URL からワイヤプロトコルを推定する。
    ///
    /// 明示設定があればそちらが優先で、これは未設定時の既定。
    /// 判定できないホスト（自前プロキシなど）は OpenAI 互換に落とす。
    /// 互換を名乗るサーバのほうが圧倒的に多く、外したときの傷が浅いため。
    ///
    /// **`generativelanguage.googleapis.com` を Gemini へ倒さないのは意図的。**
    /// 既存のテンプレートはその base URL を OpenAI 互換として使って動いており、
    /// 自動判定を変えると、設定を触っていない利用者のエージェントが黙って
    /// 別のワイヤへ移る。ネイティブ経路は明示選択でだけ有効になる。
    pub fn detect(base_url: &str) -> Self {
        if base_url.contains("api.anthropic.com") {
            Self::Anthropic
        } else {
            Self::OpenAiCompat
        }
    }

    /// **このワイヤがその種別の添付を運べるか**（Spec 36 D2。契約は
    /// `attachment_contract` の carries 表）。
    ///
    /// **全 20 マスを P0 の probe で観測して決めた表**（2026-08-12）。判定は
    /// HTTP 200 ではなく内容の照合で、`false` は 400 の受理集合列挙
    /// （OpenAI / Anthropic）か untagged enum の 422（xAI）による構造判定。
    ///
    /// **これは「ワイヤに書ける形があるか」だけを答える。**
    /// そのモデルが受理するかは別の層で、400 として画面へ出る
    /// （互換の口に `input_audio` は実在するが、音声非対応モデルは拒む）。
    /// 構造的に不可能なものと実行時拒否を同じ層で扱うと、画面で区別できなくなる。
    ///
    /// **`match` を網羅で書くのは、Provider に 6 値目を足す人へ問いを出すため** —
    /// 新しいワイヤが 4 種別それぞれを運べるかは、実装者が probe で決めるしかない。
    pub fn carries(self, kind: crate::attachment::AttachmentKind) -> bool {
        use crate::attachment::AttachmentKind as K;
        match (self, kind) {
            // 画像は全ワイヤが運ぶ（Spec 36 D9 で Responses 2 本と Gemini を回収した）。
            (_, K::Image) => true,
            // PDF も全ワイヤが運ぶ。**xAI は公式文書に記述が無いのに通った**
            // （文書と実装の乖離 2 例目。1 例目は Spec 23 の WebP）。
            (_, K::Pdf) => true,
            // 音声は Gemini ネイティブと OpenAI 互換だけ。互換は形が実在する
            // という意味で、受理はモデル次第（gpt-5.6-luna は拒む）。
            (Self::Gemini | Self::OpenAiCompat, K::Audio) => true,
            (_, K::Audio) => false,
            // 動画は Gemini ネイティブだけ。
            (Self::Gemini, K::Video) => true,
            (_, K::Video) => false,
        }
    }

    /// base URL に付けるパス。
    ///
    /// Gemini だけモデル名が URL に埋まるため `&'static str` を返せない。
    fn path(self, model: &str) -> String {
        match self {
            Self::OpenAiCompat => "/chat/completions".to_owned(),
            Self::Anthropic => "/messages".to_owned(),
            Self::Gemini => gemini::path(model),
            Self::XaiResponses | Self::OpenAiResponses => "/responses".to_owned(),
        }
    }
}

/// 1 バックエンド分の接続設定。
///
/// [`ModelTemplate`] は「ユーザーが GUI で編集する設定」、こちらは
/// 「API キーを解決した実行時の設定」。秘密の解決点をここ 1 箇所に閉じている。
#[derive(Debug, Clone)]
pub struct LlmConfig {
    /// 例: `https://api.openai.com/v1`。末尾のスラッシュは除去される。
    pub base_url: String,
    /// 解決済みの API キー。
    pub api_key: String,
    /// モデル名。
    pub model: String,
    /// サンプリング温度。`None` なら送らない。
    pub temperature: Option<f32>,
    /// 最大出力トークン数。
    pub max_tokens: u32,
    /// 1 リクエストのタイムアウト。
    pub request_timeout: Duration,
    /// 最大試行回数（初回を含む）。
    pub max_retries: u32,
    /// ツール呼び出しを使うか。`false` ならプロンプトで JSON 出力を指示する。
    /// `Provider::Anthropic` では無視される（ネイティブ経路は常にツールを使う）。
    pub use_tools: bool,
    /// ワイヤプロトコル。
    pub provider: Provider,
    /// 推論の深さ。`None` なら送らない。
    pub effort: Option<Effort>,
    /// Google 検索による接地を有効にするか。[`Provider::Gemini`] でのみ効く。
    pub google_search: bool,
    /// Grok の Live Search（web 検索）。[`Provider::XaiResponses`] でのみ効く。
    /// 値は [`ModelTemplate::xai_web_search_active`] の判定済み（AND 述語の 1 実装）。
    pub xai_web_search: bool,
    /// Grok の Live Search（X 検索）。[`Provider::XaiResponses`] でのみ効く。
    pub xai_x_search: bool,
    /// OpenAI の web 検索。[`Provider::OpenAiResponses`] でのみ効く。
    /// 値は [`ModelTemplate::openai_web_search_active`] の判定済み。
    pub openai_web_search: bool,
    /// OpenAI の Pro 推論モード（`reasoning.mode = "pro"`）。
    /// [`Provider::OpenAiResponses`] でのみ効く。
    pub openai_reasoning_pro: bool,
}

impl LlmConfig {
    /// GUI が保持する [`ModelTemplate`] から実行時設定を解決する。
    ///
    /// テンプレートは秘密を持たず、「どこから取るか」だけを持つ。実値はここで
    /// [`SecretStore`] から解決し、失敗した場合はネットワークへ出る前に弾く。
    ///
    /// 秘密が生きるのはこの関数の戻り値から HTTP ヘッダまでの区間だけで、
    /// 設定ファイルにもイベントにもエラーメッセージにも現れない。
    pub fn from_template(
        template: &ModelTemplate,
        secrets: &dyn SecretStore,
    ) -> Result<Self, LlmError> {
        let api_key = match template.credential {
            // 未設定のまま送らない。認証ヘッダ無しで外部へ出すと、ローカルで
            // 捕まえられるはずの設定不備がサーバー側の 401 になり、原因が遠くなる。
            CredentialSource::Unset => {
                return Err(LlmError::Config(format!(
                    "モデルテンプレート `{}` の API キーが未登録です\
                     （「モデルテンプレートを管理」の画面でキーを登録するか、\
                     認証不要であればその旨を指定してください）",
                    template.name
                )));
            }
            // 認証不要だとユーザーが明示した場合のみ、空キーで送る。
            CredentialSource::NotRequired => String::new(),
            CredentialSource::Keyring => secrets
                .get(template.id.as_str())
                .map_err(|err| LlmError::Config(err.to_string()))?
                .ok_or_else(|| {
                    LlmError::Config(format!(
                        "モデルテンプレート `{}` の API キーが資格情報ストアに見つかりません\
                         （「モデルテンプレートを管理」の画面から登録し直してください）",
                        template.name
                    ))
                })?,
        };

        let provider = template.effective_provider();

        Ok(Self {
            base_url: template.base_url.trim_end_matches('/').to_owned(),
            api_key,
            model: template.model.clone(),
            temperature: template.temperature,
            max_tokens: template.max_output_tokens,
            request_timeout: Duration::from_secs(template.request_timeout_secs.into()),
            max_retries: template.max_retries,
            use_tools: template.use_tools,
            provider,
            effort: template.effort,
            google_search: template.google_search,
            // フラグそのものではなく AND 述語を通した値を持つ。ワイヤ側は
            // XaiResponses でしか読まないが、判定の実装を 2 箇所にしない (Spec 31 D4)。
            xai_web_search: template.xai_web_search_active(),
            xai_x_search: template.xai_x_search_active(),
            openai_web_search: template.openai_web_search_active(),
            openai_reasoning_pro: template.openai_reasoning_pro_active(),
        })
    }
}

/// reqwest を使う実バックエンド。
pub struct HttpLlmBackend {
    config: LlmConfig,
    http: reqwest::Client,
}

impl HttpLlmBackend {
    /// 設定から HTTP バックエンドを組み立てる。
    pub fn new(config: LlmConfig) -> Result<Self, LlmError> {
        let http = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(LlmError::from)?;
        Ok(Self { config, http })
    }

    /// 1 回ぶんの往復。再試行は [`HttpLlmBackend::chat`] 側が担う。
    async fn attempt(&self, req: &ChatRequest) -> Result<ChatResponse, LlmError> {
        let url = format!(
            "{}{}",
            self.config.base_url,
            self.config.provider.path(&self.config.model)
        );

        let builder = match self.config.provider {
            Provider::OpenAiCompat => {
                let body = openai_compat::encode(req, self.config.use_tools);
                let mut b = self.http.post(&url).json(&body);
                if !self.config.api_key.is_empty() {
                    b = b.bearer_auth(&self.config.api_key);
                }
                b
            }
            Provider::Anthropic => {
                let body = anthropic::encode(req);
                self.http
                    .post(&url)
                    .header("x-api-key", &self.config.api_key)
                    .header("anthropic-version", "2023-06-01")
                    // 5 分を超えるキャッシュ TTL の要求に必要。無いと ttl が
                    // 黙って無視され、既定の 5 分に戻る（指定したのに命中率が
                    // 伸びない、という気づきにくい形で表面化する）。
                    .header("anthropic-beta", anthropic::EXTENDED_CACHE_BETA)
                    .json(&body)
            }
            Provider::Gemini => {
                let body = gemini::encode(req, self.config.google_search);
                self.http
                    .post(&url)
                    .header(gemini::AUTH_HEADER, &self.config.api_key)
                    .json(&body)
            }
            Provider::XaiResponses => {
                let body = xai_responses::encode(
                    req,
                    self.config.use_tools,
                    self.config.xai_web_search,
                    self.config.xai_x_search,
                );
                let mut b = self.http.post(&url).json(&body);
                if !self.config.api_key.is_empty() {
                    b = b.bearer_auth(&self.config.api_key);
                }
                b
            }
            Provider::OpenAiResponses => {
                let body = openai_responses::encode(
                    req,
                    self.config.use_tools,
                    self.config.openai_web_search,
                    self.config.openai_reasoning_pro,
                );
                let mut b = self.http.post(&url).json(&body);
                if !self.config.api_key.is_empty() {
                    b = b.bearer_auth(&self.config.api_key);
                }
                b
            }
        };

        let response = builder.send().await.map_err(LlmError::from)?;
        let status = response.status();

        if !status.is_success() {
            let mut body = response.text().await.unwrap_or_default();
            body.truncate(MAX_ERROR_BODY);
            return Err(LlmError::Api {
                status: status.as_u16(),
                body,
            });
        }

        // 成功ステータスでも本文が期待の形とは限らないので、raw を保持して失敗させる。
        let raw = response.text().await.map_err(LlmError::from)?;

        match self.config.provider {
            Provider::OpenAiCompat => {
                let parsed: wire::OaiResponse =
                    serde_json::from_str(&raw).map_err(|source| LlmError::Parse {
                        source,
                        raw: raw.clone(),
                    })?;
                let decoded = openai_compat::decode(parsed)?;
                openai_compat::reject_empty_reasoning(decoded, req.max_tokens)
            }
            Provider::Anthropic => {
                let parsed: wire::AnthropicResponse =
                    serde_json::from_str(&raw).map_err(|source| LlmError::Parse {
                        source,
                        raw: raw.clone(),
                    })?;
                let decoded = anthropic::decode(parsed)?;
                openai_compat::reject_empty_reasoning(decoded, req.max_tokens)
            }
            Provider::Gemini => {
                let parsed: wire::GeminiResponse =
                    serde_json::from_str(&raw).map_err(|source| LlmError::Parse {
                        source,
                        raw: raw.clone(),
                    })?;
                let decoded = gemini::decode(parsed)?;
                openai_compat::reject_empty_reasoning(decoded, req.max_tokens)
            }
            Provider::XaiResponses => {
                let parsed: wire::ResponsesResponse =
                    serde_json::from_str(&raw).map_err(|source| LlmError::Parse {
                        source,
                        raw: raw.clone(),
                    })?;
                let decoded = xai_responses::decode(parsed)?;
                openai_compat::reject_empty_reasoning(decoded, req.max_tokens)
            }
            // 応答の型は xAI と共有する（Spec 34 D2 rev6 — 11/11 発の serde 実測）。
            // 分けたのは encode と、トップレベルの要求構造体だけ。
            Provider::OpenAiResponses => {
                let parsed: wire::ResponsesResponse =
                    serde_json::from_str(&raw).map_err(|source| LlmError::Parse {
                        source,
                        raw: raw.clone(),
                    })?;
                let decoded = openai_responses::decode(parsed)?;
                openai_compat::reject_empty_reasoning(decoded, req.max_tokens)
            }
        }
    }
}

impl HttpLlmBackend {
    /// 指数バックオフつきの往復（JPEG フォールバックの内側）。
    async fn chat_with_backoff(&self, req: &ChatRequest) -> Result<ChatResponse, LlmError> {
        let attempts = self.config.max_retries.max(1);
        let mut last_error: Option<LlmError> = None;

        for attempt in 0..attempts {
            match self.attempt(req).await {
                Ok(resp) => return Ok(resp),
                Err(err) if err.is_transient() && attempt + 1 < attempts => {
                    // 200ms, 400ms, 800ms, ... 上限 5s。
                    let backoff =
                        Duration::from_millis(200u64.saturating_mul(1 << attempt)).min(Duration::from_secs(5));
                    tokio::time::sleep(backoff).await;
                    last_error = Some(err);
                }
                Err(err) => return Err(err),
            }
        }

        Err(last_error.unwrap_or(LlmError::EmptyResponse))
    }
}

/// WebP の添付を JPEG へ再エンコードしたリクエストを作る（Spec 23 D3 の純関数）。
///
/// 変換できない添付が 1 つでもあれば `None` — 半分だけ変換した形で
/// 再送しても、拒否された WebP が残っている限り同じ 400 が返る。
///
/// **画像以外は触らない**（Spec 36）。音声・PDF に画像デコーダを当てても
/// 失敗するだけで、その `None` が**画像の再送まで道連れにする**。
fn with_jpeg_attachments(req: &ChatRequest) -> Option<ChatRequest> {
    use base64::Engine as _;
    let engine = base64::engine::general_purpose::STANDARD;
    let mut converted = req.clone();
    let mut any = false;
    for message in &mut converted.messages {
        for attachment in &mut message.attachments {
            if attachment.media_type != super::canonical::PromptMediaType::Webp {
                continue;
            }
            let bytes = engine.decode(&attachment.data).ok()?;
            let decoded =
                image::load_from_memory_with_format(&bytes, image::ImageFormat::WebP).ok()?;
            // JPEG はアルファを持てないので RGB へ落としてから書く。
            let rgb = image::DynamicImage::ImageRgb8(decoded.to_rgb8());
            let mut jpeg = std::io::Cursor::new(Vec::new());
            rgb.write_to(&mut jpeg, image::ImageFormat::Jpeg).ok()?;
            attachment.data = engine.encode(jpeg.into_inner());
            attachment.media_type = super::canonical::PromptMediaType::Jpeg;
            any = true;
        }
    }
    any.then_some(converted)
}

/// リクエストに WebP の添付が載っているか（D3 の発火条件の半分）。
fn has_webp_attachments(req: &ChatRequest) -> bool {
    req.messages.iter().any(|m| {
        m.attachments
            .iter()
            .any(|a| a.media_type == super::canonical::PromptMediaType::Webp)
    })
}

#[async_trait]
impl LlmBackend for HttpLlmBackend {
    fn name(&self) -> &str {
        match self.config.provider {
            Provider::OpenAiCompat => "openai-compat",
            Provider::Anthropic => "anthropic",
            Provider::Gemini => "gemini",
            Provider::XaiResponses => "xai-responses",
            Provider::OpenAiResponses => "openai-responses",
        }
    }

    /// 指数バックオフつきで往復する。
    ///
    /// 再試行するのは [`LlmError::is_transient`] が真のものだけ。
    /// 安全フィルタによる拒否やスキーマ不一致は、同じ入力を再送しても回復しないため
    /// 即座に呼び出し側へ返す。無駄な再試行は課金とレイテンシだけを増やす。
    ///
    /// **例外が 1 つ**（Spec 23 D3）: `OpenAiCompat` で WebP の添付つき
    /// リクエストが 400 で拒否されたときだけ、**JPEG へ再エンコードして
    /// 1 回だけ**再送する。互換サーバには WebP のデコーダを持たないものが
    /// あり（xAI は公式文書上 jpg/png のみ）、形式を変えれば通る可能性がある。
    /// それでも落ちたら「画像を受け付けない接続先」として理由を本文へ書く。
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        match self.chat_with_backoff(&req).await {
            Err(LlmError::Api { status: 400, body })
                if self.config.provider == Provider::OpenAiCompat
                    && has_webp_attachments(&req) =>
            {
                let Some(jpeg_req) = with_jpeg_attachments(&req) else {
                    return Err(LlmError::Api { status: 400, body });
                };
                crate::note!(
                    "attachment fallback: model={} retrying with jpeg after 400",
                    self.config.model,
                );
                match self.chat_with_backoff(&jpeg_req).await {
                    Ok(resp) => Ok(resp),
                    // 両形式とも拒否 = この接続先は画像を受け付けない。
                    // 生の 400 本文だけでは利用者が「画像が原因」へ辿り着けない。
                    Err(LlmError::Api {
                        status: 400,
                        body: second,
                    }) => Err(LlmError::Api {
                        status: 400,
                        body: format!(
                            "この接続先は画像を受け付けません（WebP と JPEG の両方が\
                             拒否されました）。画像なしで送り直してください。\
                             プロバイダの応答: {second}"
                        ),
                    }),
                    Err(other) => Err(other),
                }
            }
            other => other,
        }
    }
}

/// テンプレートから実 HTTP バックエンドを組み立てるファクトリ。
///
/// 設定不備のときの扱いを 2 通りから選ぶ。GUI は
/// [`HttpBackendFactory::echo_on_failure`] を使い、キー未設定でもアプリが
/// 沈黙しないようにする。ただし退避したことと**その理由**は必ず表に出す。
pub struct HttpBackendFactory {
    secrets: std::sync::Arc<dyn SecretStore>,
    echo_on_failure: bool,
}

impl HttpBackendFactory {
    /// 設定不備を素直に失敗させるファクトリ。
    pub fn strict(secrets: std::sync::Arc<dyn SecretStore>) -> Self {
        Self {
            secrets,
            echo_on_failure: false,
        }
    }

    /// 設定不備のとき、理由を名乗るエコー応答へ退避するファクトリ。
    pub fn echo_on_failure(secrets: std::sync::Arc<dyn SecretStore>) -> Self {
        Self {
            secrets,
            echo_on_failure: true,
        }
    }
}

impl super::BackendFactory for HttpBackendFactory {
    fn create(&self, template: &ModelTemplate) -> Result<BackendResolution, LlmError> {
        match LlmConfig::from_template(template, self.secrets.as_ref())
            .and_then(HttpLlmBackend::new)
        {
            Ok(backend) => Ok(BackendResolution::healthy(std::sync::Arc::new(backend))),

            Err(err) if self.echo_on_failure => {
                // 退避先の応答そのものに理由を書く。会話ログを見ている人が
                // 別の画面を探さずに原因へ辿り着けるようにするため。
                // 「エコー応答」とだけ名乗る実装では、どの設定が欠けているのか
                // 分からず、キーを直したつもりのまま延々と偽の応答が続く。
                let reason = err.to_string();
                Ok(BackendResolution::degraded(
                    std::sync::Arc::new(super::EchoBackend::new(format!("[エコー応答 / {reason}]"))),
                    reason,
                ))
            }

            Err(err) => Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::BackendFactory;
    use crate::model::ModelTemplate;
    use crate::secret::InMemorySecretStore;

    #[test]
    fn provider_detection_defaults_to_compat() {
        assert_eq!(
            Provider::detect("https://api.anthropic.com/v1"),
            Provider::Anthropic
        );
        assert_eq!(
            Provider::detect("https://api.openai.com/v1"),
            Provider::OpenAiCompat
        );
        // 判定不能なホストは互換に落とす（安全側）。
        assert_eq!(
            Provider::detect("http://localhost:8080/v1"),
            Provider::OpenAiCompat
        );
        // api.x.ai を XaiResponses へ倒さないのは意図的（Spec 31 P1 / 契約）。
        // 既存の互換運用の村（grok-* を open_ai_compat で回している個体）を
        // 黙って別のワイヤへ移さない。gemini と同じ理由の 2 例目。
        // **親切心でここへ自動判定を足すと、その村の全ターンが新ワイヤへ移る。**
        assert_eq!(
            Provider::detect("https://api.x.ai/v1"),
            Provider::OpenAiCompat
        );
    }

    fn keyring_template() -> ModelTemplate {
        let mut template = ModelTemplate::new("tpl_1", "テスト", "claude-sonnet-5");
        template.base_url = "https://api.anthropic.com/v1".into();
        template.credential = CredentialSource::Keyring;
        template
    }

    /// keyring 指定なのにストアが空 = 何らかの理由で消えた状態。
    /// 「未設定」とは別の文言にして、どちらの状況か区別できるようにする。
    #[test]
    fn a_missing_keyring_entry_fails_before_any_network_call() {
        let secrets = InMemorySecretStore::new();
        let err = LlmConfig::from_template(&keyring_template(), &secrets).unwrap_err();

        assert_eq!(err.code(), "LLM_CONFIG");
        assert!(err.to_string().contains("見つかりません"));
    }

    #[test]
    fn registered_credential_is_resolved_from_the_store() {
        let secrets = InMemorySecretStore::new();
        secrets.set("tpl_1", "resolved-secret").unwrap();

        let config = LlmConfig::from_template(&keyring_template(), &secrets).unwrap();
        assert_eq!(config.api_key, "resolved-secret");
        assert_eq!(config.provider, Provider::Anthropic);
    }

    #[test]
    fn explicitly_auth_free_templates_send_an_empty_key() {
        let mut template = ModelTemplate::new("tpl_local", "ローカル", "qwen3");
        template.base_url = "http://localhost:11434/v1/".into();
        template.credential = CredentialSource::NotRequired;

        let config = LlmConfig::from_template(&template, &InMemorySecretStore::new()).unwrap();
        assert_eq!(config.base_url, "http://localhost:11434/v1");
        assert_eq!(config.provider, Provider::OpenAiCompat);
        assert!(config.api_key.is_empty());
    }

    /// 未設定は「認証不要」と読み替えない。
    ///
    /// 読み替えると、キーを入れていないだけのテンプレートが認証ヘッダ無しで
    /// 外部へ送られ、サーバー側の 401 になる。実際にそれが起きた。
    #[test]
    fn an_unset_credential_is_refused_before_the_request_leaves() {
        let template = ModelTemplate::new("tpl", "既定", "claude-sonnet-5");
        assert_eq!(template.credential, CredentialSource::Unset);

        let err = LlmConfig::from_template(&template, &InMemorySecretStore::new()).unwrap_err();
        assert_eq!(err.code(), "LLM_CONFIG");
        assert!(err.to_string().contains("未登録"));
    }

    #[tokio::test]
    async fn degraded_fallback_names_the_cause_instead_of_going_quiet() {
        let secrets = std::sync::Arc::new(InMemorySecretStore::new());
        let template = keyring_template();

        // strict はそのまま失敗させる。
        assert!(
            HttpBackendFactory::strict(secrets.clone())
                .create(&template)
                .is_err()
        );

        let resolution = HttpBackendFactory::echo_on_failure(secrets)
            .create(&template)
            .expect("退避してでもバックエンドは返る");

        // 退避したことが戻り値の型に載っている。
        let reason = resolution
            .degraded_reason
            .as_deref()
            .expect("退避理由が付くこと");
        assert!(
            reason.contains("見つかりません"),
            "原因を名指しすること: {reason}"
        );

        // 応答本文自体にも理由が乗る。会話ログだけ見ていても原因に辿り着ける。
        let response = resolution
            .backend
            .chat(crate::llm::ChatRequest::plain(
                "claude-sonnet-5",
                vec![crate::llm::ChatMessage::user("やあ")],
                64,
            ))
            .await
            .unwrap();
        let text = response.text.unwrap_or_default();
        assert!(
            text.contains("見つかりません"),
            "応答が理由を名乗ること: {text}"
        );
    }

    #[test]
    fn a_working_template_is_not_reported_as_degraded() {
        let secrets = std::sync::Arc::new(InMemorySecretStore::new());
        secrets.set("tpl_1", "resolved-secret").unwrap();

        let resolution = HttpBackendFactory::echo_on_failure(secrets)
            .create(&keyring_template())
            .unwrap();

        assert!(resolution.degraded_reason.is_none());
        assert_eq!(resolution.backend.name(), "anthropic");
    }

    /// WebP の添付が JPEG へ再エンコードされること（Spec 23 D3 の変換部）。
    ///
    /// 実 WebP を image crate で作る — 手組みのヘッダだけではデコーダが
    /// 本文を要求して落ちる。変換後は media_type が jpeg になり、
    /// data は JPEG のマジック（FF D8）で始まる。
    #[test]
    fn jpeg_fallback_converts_webp_attachments() {
        use crate::llm::canonical::{ChatMessage, PromptAttachment, PromptMediaType};
        use base64::Engine as _;

        let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            4,
            4,
            image::Rgb([255, 0, 0]),
        ));
        let mut webp = std::io::Cursor::new(Vec::new());
        img.write_to(&mut webp, image::ImageFormat::WebP).unwrap();
        let data = base64::engine::general_purpose::STANDARD.encode(webp.into_inner());

        let req = ChatRequest::plain(
            "gpt-x",
            vec![ChatMessage::user_with_attachments(
                "見て",
                vec![PromptAttachment::new(PromptMediaType::Webp, data)],
            )],
            64,
        );
        assert!(has_webp_attachments(&req));

        let jpeg = with_jpeg_attachments(&req).expect("変換できること");
        let att = &jpeg.messages[0].attachments[0];
        assert_eq!(att.media_type, PromptMediaType::Jpeg);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&att.data)
            .unwrap();
        assert_eq!(&bytes[0..2], &[0xFF, 0xD8], "JPEG のマジックで始まること");
        // 元のリクエストは無傷（clone してから変換している）。
        assert_eq!(
            req.messages[0].attachments[0].media_type,
            PromptMediaType::Webp
        );
    }

    /// 添付の無いリクエストではフォールバックの発火条件が立たないこと。
    /// ここが崩れると、画像と無関係な 400（パラメータ誤りなど）まで
    /// もう 1 回課金して再送する。
    #[test]
    fn requests_without_webp_attachments_do_not_arm_the_fallback() {
        use crate::llm::canonical::ChatMessage;
        let req = ChatRequest::plain("gpt-x", vec![ChatMessage::user("hi")], 64);
        assert!(!has_webp_attachments(&req));
        assert!(with_jpeg_attachments(&req).is_none());
    }

    /// 秘密が拒否経路へ漏れないこと。
    #[test]
    fn a_resolved_secret_never_appears_in_error_text() {
        let secrets = InMemorySecretStore::new();
        secrets.set("tpl_1", "sk-ant-SUPER-SECRET").unwrap();

        let mut template = keyring_template();
        // タイムアウト 0 秒で reqwest の構築を失敗させ、エラー経路を通す。
        template.request_timeout_secs = 0;

        if let Err(err) = LlmConfig::from_template(&template, &secrets).and_then(HttpLlmBackend::new)
        {
            assert!(!err.to_string().contains("SUPER-SECRET"));
        }
    }
}
