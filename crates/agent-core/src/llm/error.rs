//! LLM 境界の失敗型。
//!
//! Kataribe `crates/llm_client/src/error.rs` の設計を継承している。要点は 3 つ:
//!
//! 1. **`Parse` は raw を保持する** — 却下理由と一緒にモデルへ差し戻して再生成させる燃料になる。
//!    raw を捨てると「JSON が壊れていた」以上のことが言えなくなり、自己修復が組めない。
//! 2. **`Blocked` を `EmptyResponse` から分離する** — 安全フィルタで弾かれた応答を
//!    200 + 空本文で返すプロバイダがあり、理由を捨てると一律「空の応答」になって診断不能になる。
//! 3. **`is_transient` が再試行の唯一の判断軸** — HTTP 障害 / 429 / 5xx / 推論の空応答だけが
//!    再試行対象。`Blocked` と `Parse` は同じ入力を再送しても回復しないので対象外。

use thiserror::Error;

/// LLM 呼び出しで発生しうる失敗。
#[derive(Debug, Error)]
pub enum LlmError {
    /// 設定不備（API キー未設定など）。ネットワークへ出る前に弾く。
    #[error("LLM 設定エラー: {0}")]
    Config(String),

    /// HTTP 層の失敗（接続不能・タイムアウト・TLS）。
    ///
    /// `detail` は **source 連鎖を平坦化した文面**。reqwest の
    /// "error sending request for url" は真因をラップして隠すため、
    /// "…: operation timed out" のような根本原因まで surface させる。
    #[error("HTTP エラー: {detail}")]
    Http {
        /// 元の reqwest エラー。
        #[source]
        source: reqwest::Error,
        /// source 連鎖を平坦化した詳細。
        detail: String,
    },

    /// API がエラーステータスを返した。`status` で再試行可否を判断する。
    #[error("API エラー (status={status}): {body}")]
    Api {
        /// HTTP ステータスコード。
        status: u16,
        /// 応答本文（先頭のみ）。
        body: String,
    },

    /// 応答が空。推論モデルが出力予算を思考に使い切った場合に起きる（`finish_reason=length`）。
    /// 再抽選で回復しうるため一過性として扱う。
    #[error("LLM が空の応答を返しました")]
    EmptyResponse,

    /// プロバイダが応答をブロックした（安全フィルタ・利用規約）。
    /// 同じ入力の再送では回復しないので非一過性。
    #[error("プロバイダが応答をブロックしました (理由: {reason})")]
    Blocked {
        /// プロバイダが示した理由。
        reason: String,
    },

    /// ツール呼び出しを強制したのに、tool_call も JSON 本文も得られなかった。
    #[error("構造化出力が得られませんでした (tool_call 不在かつ本文も JSON ではない)")]
    NoStructuredOutput,

    /// 構造化出力の JSON パースに失敗した。**`raw` を保持**して再生成に使えるようにする。
    #[error("構造化出力のパースに失敗しました: {source}")]
    Parse {
        /// 元のパースエラー。
        #[source]
        source: serde_json::Error,
        /// パースに失敗した生文字列。再生成プロンプトに添える。
        raw: String,
    },
}

/// reqwest エラーから [`LlmError::Http`] を作る。
///
/// source 連鎖を平坦化して `detail` に畳み込み、
/// "error sending request for url (...)" の下に隠れた timeout / connect / TLS の真因を見せる。
impl From<reqwest::Error> for LlmError {
    fn from(source: reqwest::Error) -> Self {
        let mut detail = source.to_string();
        let mut cause: Option<&(dyn std::error::Error + 'static)> =
            std::error::Error::source(&source);
        while let Some(inner) = cause {
            detail.push_str(": ");
            detail.push_str(&inner.to_string());
            cause = inner.source();
        }
        LlmError::Http { source, detail }
    }
}

impl LlmError {
    /// UI 側の分岐に使う安定コード。
    pub fn code(&self) -> &'static str {
        match self {
            Self::Config(_) => "LLM_CONFIG",
            Self::Http { .. } => "LLM_HTTP",
            Self::Api { .. } => "LLM_API",
            Self::EmptyResponse => "LLM_EMPTY_RESPONSE",
            Self::Blocked { .. } => "LLM_BLOCKED",
            Self::NoStructuredOutput => "LLM_NO_STRUCTURED_OUTPUT",
            Self::Parse { .. } => "LLM_PARSE",
        }
    }

    /// 再試行で回復しうるか。
    ///
    /// 対象は HTTP 障害・429・5xx・推論モデルの空応答のみ。
    /// `Blocked` / `Parse` / `Config` は同じ入力を再送しても回復しないため除外する。
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Http { source, .. } => {
                source.is_timeout() || source.is_connect() || source.is_request()
            }
            Self::Api { status, .. } => *status == 429 || (500..600).contains(status),
            Self::EmptyResponse => true,
            _ => false,
        }
    }

    /// 再生成プロンプトへ添えるための生文字列（保持している場合）。
    pub fn raw_output(&self) -> Option<&str> {
        match self {
            Self::Parse { raw, .. } => Some(raw),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_classification_matches_retry_policy() {
        assert!(LlmError::EmptyResponse.is_transient());
        assert!(
            LlmError::Api {
                status: 429,
                body: String::new()
            }
            .is_transient()
        );
        assert!(
            LlmError::Api {
                status: 503,
                body: String::new()
            }
            .is_transient()
        );
        assert!(
            !LlmError::Api {
                status: 400,
                body: String::new()
            }
            .is_transient()
        );
        assert!(
            !LlmError::Blocked {
                reason: "SAFETY".into()
            }
            .is_transient()
        );
        assert!(!LlmError::Config("キー未設定".into()).is_transient());
    }

    #[test]
    fn parse_error_preserves_raw_for_regeneration() {
        let raw = r#"{"broken": "#;
        let source = serde_json::from_str::<serde_json::Value>(raw).unwrap_err();
        let err = LlmError::Parse {
            source,
            raw: raw.to_owned(),
        };

        assert_eq!(err.raw_output(), Some(raw));
        assert!(!err.is_transient());
    }
}
