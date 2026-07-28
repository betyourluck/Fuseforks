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
use super::{BackendResolution, LlmBackend, anthropic, openai_compat, wire};
use crate::model::ModelTemplate;

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
}

impl Provider {
    /// base URL からワイヤプロトコルを推定する。
    ///
    /// 明示設定があればそちらが優先で、これは未設定時の既定。
    /// 判定できないホスト（自前プロキシなど）は OpenAI 互換に落とす。
    /// 互換を名乗るサーバのほうが圧倒的に多く、外したときの傷が浅いため。
    pub fn detect(base_url: &str) -> Self {
        if base_url.contains("api.anthropic.com") {
            Self::Anthropic
        } else {
            Self::OpenAiCompat
        }
    }

    /// base URL に付けるパス。
    fn path(self) -> &'static str {
        match self {
            Self::OpenAiCompat => "/chat/completions",
            Self::Anthropic => "/messages",
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
}

impl LlmConfig {
    /// GUI が保持する [`ModelTemplate`] から実行時設定を解決する。
    ///
    /// API キーはテンプレートに実値を持たず、環境変数名だけを持つ契約になっている。
    /// ここで解決に失敗した場合はネットワークへ出る前に [`LlmError::Config`] で弾く。
    pub fn from_template(template: &ModelTemplate) -> Result<Self, LlmError> {
        let api_key = match &template.api_key_env {
            Some(var) => std::env::var(var).map_err(|_| {
                LlmError::Config(format!(
                    "環境変数 `{var}` が未設定です (モデルテンプレート `{}` の API キー)",
                    template.name
                ))
            })?,
            // ローカル推論サーバは認証不要なことが多い。空キーを許す。
            None => String::new(),
        };

        let provider = template
            .provider
            .unwrap_or_else(|| Provider::detect(&template.base_url));

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
        let url = format!("{}{}", self.config.base_url, self.config.provider.path());

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
                    .json(&body)
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
                openai_compat::reject_empty_reasoning(decoded)
            }
            Provider::Anthropic => {
                let parsed: wire::AnthropicResponse =
                    serde_json::from_str(&raw).map_err(|source| LlmError::Parse {
                        source,
                        raw: raw.clone(),
                    })?;
                let decoded = anthropic::decode(parsed)?;
                openai_compat::reject_empty_reasoning(decoded)
            }
        }
    }
}

#[async_trait]
impl LlmBackend for HttpLlmBackend {
    fn name(&self) -> &str {
        match self.config.provider {
            Provider::OpenAiCompat => "openai-compat",
            Provider::Anthropic => "anthropic",
        }
    }

    /// 指数バックオフつきで往復する。
    ///
    /// 再試行するのは [`LlmError::is_transient`] が真のものだけ。
    /// 安全フィルタによる拒否やスキーマ不一致は、同じ入力を再送しても回復しないため
    /// 即座に呼び出し側へ返す。無駄な再試行は課金とレイテンシだけを増やす。
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let attempts = self.config.max_retries.max(1);
        let mut last_error: Option<LlmError> = None;

        for attempt in 0..attempts {
            match self.attempt(&req).await {
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

/// テンプレートから実 HTTP バックエンドを組み立てるファクトリ。
///
/// 設定不備のときの扱いを 2 通りから選ぶ。GUI は
/// [`HttpBackendFactory::echo_on_failure`] を使い、キー未設定でもアプリが
/// 沈黙しないようにする。ただし退避したことと**その理由**は必ず表に出す。
pub struct HttpBackendFactory {
    echo_on_failure: bool,
}

impl HttpBackendFactory {
    /// 設定不備を素直に失敗させるファクトリ。
    pub fn strict() -> Self {
        Self {
            echo_on_failure: false,
        }
    }

    /// 設定不備のとき、理由を名乗るエコー応答へ退避するファクトリ。
    pub fn echo_on_failure() -> Self {
        Self {
            echo_on_failure: true,
        }
    }
}

impl super::BackendFactory for HttpBackendFactory {
    fn create(&self, template: &ModelTemplate) -> Result<BackendResolution, LlmError> {
        match LlmConfig::from_template(template).and_then(HttpLlmBackend::new) {
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
    }

    #[test]
    fn missing_api_key_env_fails_before_any_network_call() {
        let mut template = ModelTemplate::new("tpl_1", "テスト", "gpt-4o");
        template.api_key_env = Some("CONCORDIA_TEST_KEY_THAT_DOES_NOT_EXIST".into());

        let err = LlmConfig::from_template(&template).unwrap_err();
        assert_eq!(err.code(), "LLM_CONFIG");
    }

    #[test]
    fn template_without_key_env_is_allowed_for_local_servers() {
        let mut template = ModelTemplate::new("tpl_local", "ローカル", "qwen3");
        template.base_url = "http://localhost:11434/v1/".into();
        template.api_key_env = None;

        let config = LlmConfig::from_template(&template).unwrap();
        assert_eq!(config.base_url, "http://localhost:11434/v1");
        assert_eq!(config.provider, Provider::OpenAiCompat);
        assert!(config.api_key.is_empty());
    }

    #[tokio::test]
    async fn degraded_fallback_names_the_missing_variable_instead_of_going_quiet() {
        let mut template = ModelTemplate::new("tpl", "テスト", "gpt-4o");
        template.api_key_env = Some("CONCORDIA_TEST_KEY_THAT_DOES_NOT_EXIST".into());

        // strict はそのまま失敗させる。
        assert!(HttpBackendFactory::strict().create(&template).is_err());

        let resolution = HttpBackendFactory::echo_on_failure()
            .create(&template)
            .expect("退避してでもバックエンドは返る");

        // 退避したことが戻り値の型に載っている。
        let reason = resolution
            .degraded_reason
            .as_deref()
            .expect("退避理由が付くこと");
        assert!(
            reason.contains("CONCORDIA_TEST_KEY_THAT_DOES_NOT_EXIST"),
            "どの環境変数が欠けているか名指しすること: {reason}"
        );

        // 応答本文自体にも理由が乗る。会話ログだけ見ていても原因に辿り着ける。
        let response = resolution
            .backend
            .chat(crate::llm::ChatRequest::plain(
                "gpt-4o",
                vec![crate::llm::ChatMessage::user("やあ")],
                64,
            ))
            .await
            .unwrap();
        let text = response.text.unwrap_or_default();
        assert!(
            text.contains("CONCORDIA_TEST_KEY_THAT_DOES_NOT_EXIST"),
            "応答が理由を名乗ること: {text}"
        );
    }

    #[test]
    fn a_working_template_is_not_reported_as_degraded() {
        let mut template = ModelTemplate::new("tpl_local", "ローカル", "qwen3");
        template.base_url = "http://localhost:11434/v1".into();
        template.api_key_env = None;

        let resolution = HttpBackendFactory::echo_on_failure().create(&template).unwrap();
        assert!(resolution.degraded_reason.is_none());
        assert_eq!(resolution.backend.name(), "openai-compat");
    }
}
