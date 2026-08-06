//! LLM 境界。
//!
//! 層構造（上から）:
//!
//! ```text
//! orchestrator          canonical 型だけを組む。wire を一切見ない。
//!   ↓
//! LlmBackend (trait)    差し替え点。実 HTTP / モックの両方がここに嵌まる。
//!   ↓
//! client                URL・ヘッダ・タイムアウト・再試行。wire の中身を見ない。
//!   ↓
//! adapter               canonical ⇄ wire の純関数。ネットワークを知らない。
//!   ↓
//! wire                  プロバイダの生 JSON 形。ここが唯一の真実。
//! ```
//!
//! この分離の実利は 2 つある。プロバイダ追加が adapter 1 ファイルに閉じること、
//! そして **ネットワークなしでオーケストレーターを全部テストできる**こと
//! （[`EchoBackend`] を挿すだけで済む）。

pub mod anthropic;
pub mod canonical;
pub mod client;
pub mod error;
pub mod gemini;
pub mod openai_compat;
pub mod wire;

use std::sync::Arc;

use async_trait::async_trait;

pub use canonical::{
    ChatMessage, ChatRequest, ChatResponse, Effort, Finish, Grounding, GroundingSource,
    ImageAttachment, ImageMediaType, Role, ToolCall, ToolChoice, ToolSpec, Usage,
};
pub use client::{HttpBackendFactory, HttpLlmBackend, LlmConfig, Provider};
pub use error::LlmError;

/// LLM バックエンドの差し替え点。
///
/// オーケストレーターはこの trait だけに依存する。実 HTTP 実装
/// （[`HttpLlmBackend`]）とテスト用の [`EchoBackend`] が同じ穴に嵌まるため、
/// エージェントのライフサイクルとメッセージ配送をネットワークなしで検証できる。
#[async_trait]
pub trait LlmBackend: Send + Sync {
    /// 診断表示用の名前。
    fn name(&self) -> &str;

    /// 1 往復。再試行の要否は実装が判断する。
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError>;
}

/// バックエンドの解決結果。
///
/// 「組めたか」だけでなく「**要求どおりに組めたか**」を返すのが要点。
/// 設定不備で代替へ退避したとき、その事実を戻り値の型に載せておかないと、
/// 呼び出し側は退避を検知できず、応答は返るのに設定が効いていない状態が続く。
pub struct BackendResolution {
    /// 実際に使うバックエンド。
    pub backend: Arc<dyn LlmBackend>,
    /// 要求どおりに組めなかった場合の理由。`None` なら正常。
    pub degraded_reason: Option<String>,
}

impl BackendResolution {
    /// 要求どおりに組めた場合。
    pub fn healthy(backend: Arc<dyn LlmBackend>) -> Self {
        Self {
            backend,
            degraded_reason: None,
        }
    }

    /// 代替へ退避した場合。
    pub fn degraded(backend: Arc<dyn LlmBackend>, reason: impl Into<String>) -> Self {
        Self {
            backend,
            degraded_reason: Some(reason.into()),
        }
    }
}

/// モデルテンプレートからバックエンドを組み立てる差し替え点。
///
/// バックエンドを 1 個に固定できないのは、テンプレートごとに
/// `base_url` / プロバイダ / API キーが違うため。エージェント A が OpenAI、
/// エージェント B がローカル互換サーバ、という構成がこのシステムの前提にある。
pub trait BackendFactory: Send + Sync {
    /// テンプレートに対応するバックエンドを作る。
    ///
    /// # Errors
    /// 設定が不完全で、代替への退避も許可されていない場合。
    fn create(&self, template: &crate::model::ModelTemplate)
    -> Result<BackendResolution, LlmError>;
}

/// 常に同じバックエンドを返すファクトリ。テストと初回起動用。
pub struct FixedBackendFactory(Arc<dyn LlmBackend>);

impl FixedBackendFactory {
    /// 固定のバックエンドを包む。
    pub fn new(backend: Arc<dyn LlmBackend>) -> Self {
        Self(backend)
    }

    /// [`EchoBackend`] を返すファクトリを作る。
    pub fn echo(prefix: impl Into<String>) -> Self {
        Self(Arc::new(EchoBackend::new(prefix)))
    }
}

impl BackendFactory for FixedBackendFactory {
    fn create(
        &self,
        _template: &crate::model::ModelTemplate,
    ) -> Result<BackendResolution, LlmError> {
        // 意図してこのバックエンドを選んでいるので、退避ではない。
        Ok(BackendResolution::healthy(Arc::clone(&self.0)))
    }
}

/// ネットワークを使わない決定論バックエンド。
///
/// 用途は 2 つ:
/// - **単体テスト**: 応答が決定論的なので、配送とライフサイクルだけを検証できる。
/// - **初回起動**: API キー未設定でもアプリが動き、GUI の配線を確認できる。
///   キーを入れる前に「何も起きない」状態を見せると、設定不備と実装不具合の
///   区別がつかなくなる。
pub struct EchoBackend {
    prefix: String,
}

impl EchoBackend {
    /// 応答本文に付ける接頭辞を指定して生成する。
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
        }
    }
}

impl Default for EchoBackend {
    fn default() -> Self {
        Self::new("[echo]")
    }
}

#[async_trait]
impl LlmBackend for EchoBackend {
    fn name(&self) -> &str {
        "echo"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let last_user = req
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .map(|m| m.content.as_str())
            .unwrap_or("");

        // 概算のトークン数。実測ではないので、統計を見る側が誤解しないよう 4 文字 = 1 トークンの
        // 粗い近似であることを応答の形からは隠さない（EchoBackend は診断用途に限る）。
        let prompt_chars: usize = req.messages.iter().map(|m| m.content.chars().count()).sum();
        let text = format!("{} {}", self.prefix, last_user);
        let completion = (text.chars().count() / 4).max(1) as u64;

        Ok(ChatResponse {
            text: Some(text),
            tool_calls: Vec::new(),
            finish: Finish::Stop,
            usage: Usage {
                prompt: (prompt_chars / 4) as u64,
                completion,
                cache_read: 0,
            },
            // このプロバイダは接地を代行しない。
            grounding: Grounding::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn echo_backend_reflects_last_user_turn() {
        let backend = EchoBackend::new("[test]");
        let req = ChatRequest::plain(
            "mock",
            vec![
                ChatMessage::system("システム"),
                ChatMessage::user("こんにちは"),
            ],
            128,
        );

        let resp = backend.chat(req).await.unwrap();
        assert_eq!(resp.text.as_deref(), Some("[test] こんにちは"));
        assert_eq!(resp.finish, Finish::Stop);
        assert!(resp.usage.total() > 0);
    }
}
