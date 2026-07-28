//! エラー型と、UI へ伝播させるためのシリアライズ可能なペイロード。
//!
//! 設計方針:
//! - コア層の内部表現は [`CoreError`]（`std::error::Error` 実装、`source` 連鎖を保つ）。
//! - GUI へ渡る境界では [`ErrorPayload`] に落とす。こちらは `Clone + Serialize` で、
//!   イベント配信（`broadcast`）にもそのまま載せられる。
//! - [`ErrorPayload::code`] は**安定した機械可読コード**であり、UI 側の分岐に使う。
//!   人間向け文言 (`message`) は自由に変えてよいが、`code` の値は契約として維持する。

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// コア層で発生しうる失敗の全体集合。
#[derive(Debug, Error)]
pub enum CoreError {
    /// 指定 ID のエージェントが登録簿に存在しない。
    #[error("エージェント `{0}` は登録されていません")]
    AgentNotFound(String),

    /// 既に同じ ID のエージェントが登録済み。
    #[error("エージェント `{0}` は既に登録されています")]
    DuplicateAgent(String),

    /// 参照されたモデルテンプレートが存在しない。
    #[error("モデルテンプレート `{0}` は登録されていません")]
    ModelTemplateNotFound(String),

    /// トポロジー（接続関係）が不正。自己ループや未登録先への接続など。
    #[error("トポロジーが不正です: {reason}")]
    InvalidTopology {
        /// 不正と判断した具体的な理由。
        reason: String,
    },

    /// 稼働中のエージェントに対して再度起動を要求した。
    #[error("エージェント `{agent_id}` は既に稼働中です")]
    AlreadyRunning {
        /// 対象エージェント ID。
        agent_id: String,
    },

    /// 停止中のエージェントに対して停止・配送を要求した。
    #[error("エージェント `{agent_id}` は稼働していません")]
    NotRunning {
        /// 対象エージェント ID。
        agent_id: String,
    },

    /// 受信箱が詰まっており、メッセージを受け付けられない（背圧）。
    #[error("エージェント `{agent_id}` の受信箱が飽和しています（capacity={capacity}）")]
    MailboxFull {
        /// 対象エージェント ID。
        agent_id: String,
        /// 受信箱の容量。
        capacity: usize,
    },

    /// 設定ファイルの読み書きに失敗した。
    #[error("設定ファイル `{path}` の入出力に失敗しました")]
    ConfigIo {
        /// 対象パス（ワークスペース相対）。
        path: String,
        /// 元の I/O エラー。
        #[source]
        source: std::io::Error,
    },

    /// エージェント ID などの識別子が命名規約に反しており、パスとして安全でない。
    #[error("識別子 `{value}` は使用できません（許可: 英数字・`-`・`_`、1〜64 文字）")]
    UnsafeIdentifier {
        /// 拒否した入力値。
        value: String,
    },

    /// LLM 境界の失敗。詳細な分類は [`crate::llm::LlmError`] が持つ。
    #[error(transparent)]
    Llm(#[from] crate::llm::LlmError),

    /// Rayon 側へ逃がした CPU バウンド処理が失敗した。
    #[error("計算タスクが失敗しました: {0}")]
    Compute(String),

    /// JSON の直列化・逆直列化に失敗した。
    #[error("直列化に失敗しました: {0}")]
    Serde(#[from] serde_json::Error),
}

impl CoreError {
    /// UI 側の分岐に使う安定コードを返す。
    ///
    /// この文字列は外部契約として扱い、変更する場合は TypeScript 側の
    /// `ErrorCode` と同時に更新すること。
    pub fn code(&self) -> &'static str {
        match self {
            Self::AgentNotFound(_) => "AGENT_NOT_FOUND",
            Self::DuplicateAgent(_) => "DUPLICATE_AGENT",
            Self::ModelTemplateNotFound(_) => "MODEL_TEMPLATE_NOT_FOUND",
            Self::InvalidTopology { .. } => "INVALID_TOPOLOGY",
            Self::AlreadyRunning { .. } => "ALREADY_RUNNING",
            Self::NotRunning { .. } => "NOT_RUNNING",
            Self::MailboxFull { .. } => "MAILBOX_FULL",
            Self::ConfigIo { .. } => "CONFIG_IO",
            Self::UnsafeIdentifier { .. } => "UNSAFE_IDENTIFIER",
            // LLM 境界のコードはそのまま透過させ、UI 側で 1 つの体系として扱えるようにする。
            Self::Llm(err) => err.code(),
            Self::Compute(_) => "COMPUTE_FAILED",
            Self::Serde(_) => "SERDE_FAILED",
        }
    }

    /// このエラーが特定のエージェントに帰属する場合、その ID を返す。
    ///
    /// UI 側はこれを使って、該当カードだけを失敗表示に切り替える。
    pub fn agent_id(&self) -> Option<&str> {
        match self {
            Self::AgentNotFound(id) | Self::DuplicateAgent(id) => Some(id),
            Self::AlreadyRunning { agent_id }
            | Self::NotRunning { agent_id }
            | Self::MailboxFull { agent_id, .. } => Some(agent_id),
            _ => None,
        }
    }

    /// 呼び出し側の再試行で回復しうるかどうかの目安。
    ///
    /// UI はこの値を見て「再試行」ボタンを出すかどうかを決める。
    /// LLM 境界の判定は [`crate::llm::LlmError::is_transient`] に委譲する
    /// （安全フィルタ拒否やスキーマ不一致は再送しても回復しないため、そこで弾かれる）。
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::MailboxFull { .. } | Self::Compute(_) => true,
            Self::Llm(err) => err.is_transient(),
            _ => false,
        }
    }

    /// 構造化出力のパースに失敗した際の生応答。再生成プロンプトへ添える燃料。
    pub fn raw_output(&self) -> Option<&str> {
        match self {
            Self::Llm(err) => err.raw_output(),
            _ => None,
        }
    }
}

/// GUI 境界を越えるためのエラー表現。
///
/// `source` 連鎖は [`ErrorPayload::detail`] に文字列として畳み込まれる。
/// これは「型を渡す」のではなく「表示可能な事実を渡す」ための構造体である。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorPayload {
    /// 安定した機械可読コード。UI の分岐キー。
    pub code: String,
    /// 人間向けの一行説明。
    pub message: String,
    /// `source` 連鎖を畳み込んだ詳細（存在する場合）。
    pub detail: Option<String>,
    /// 帰属するエージェント ID（特定できる場合）。
    pub agent_id: Option<String>,
    /// 再試行で回復しうるか。
    pub retryable: bool,
}

impl From<&CoreError> for ErrorPayload {
    fn from(err: &CoreError) -> Self {
        // source 連鎖を辿り、根本原因まで含めて 1 本の文字列に畳み込む。
        let mut detail = String::new();
        let mut cursor: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(err);
        while let Some(cause) = cursor {
            if !detail.is_empty() {
                detail.push_str(" <- ");
            }
            detail.push_str(&cause.to_string());
            cursor = cause.source();
        }

        Self {
            code: err.code().to_owned(),
            message: err.to_string(),
            detail: if detail.is_empty() { None } else { Some(detail) },
            agent_id: err.agent_id().map(str::to_owned),
            retryable: err.is_retryable(),
        }
    }
}

impl From<CoreError> for ErrorPayload {
    fn from(err: CoreError) -> Self {
        Self::from(&err)
    }
}

/// Tauri コマンドの戻り値 `Result<T, CoreError>` をそのまま使えるようにする。
///
/// Tauri v2 はエラー型に `Serialize` を要求するため、ここで [`ErrorPayload`] へ
/// 変換して送出する。これにより GUI 層に変換の責務を持たせずに済む。
impl Serialize for CoreError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        ErrorPayload::from(self).serialize(serializer)
    }
}

/// コア層の標準 `Result`。
pub type CoreResult<T> = Result<T, CoreError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_carries_stable_code_and_agent_id() {
        let err = CoreError::AlreadyRunning {
            agent_id: "agent_01".into(),
        };
        let payload = ErrorPayload::from(&err);

        assert_eq!(payload.code, "ALREADY_RUNNING");
        assert_eq!(payload.agent_id.as_deref(), Some("agent_01"));
        assert!(!payload.retryable);
    }

    #[test]
    fn payload_folds_source_chain_into_detail() {
        let err = CoreError::ConfigIo {
            path: "agents/agent_01/SKILL.md".into(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "見つかりません"),
        };
        let payload = ErrorPayload::from(&err);

        assert_eq!(payload.code, "CONFIG_IO");
        assert_eq!(payload.detail.as_deref(), Some("見つかりません"));
    }

    #[test]
    fn core_error_serializes_as_payload() {
        let err = CoreError::Compute("rayon worker が落ちました".into());
        let json = serde_json::to_value(&err).expect("シリアライズできること");

        assert_eq!(json["code"], "COMPUTE_FAILED");
        assert_eq!(json["retryable"], true);
    }
}
