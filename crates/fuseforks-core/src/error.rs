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

    /// 既に同じ表示名のエージェントが登録済み（Spec 06）。
    ///
    /// 表示名は会話・束ね・入退室通知・顔ぶれの語彙で、重複すると
    /// それら全部が「どちらの話か」を失う。ID と違い構造では守られないので、
    /// 書き込みの入口（登録・改名）で弾く。
    #[error("表示名 `{0}` は既に使われています。別の名前を付けてください")]
    DuplicateAgentName(String),

    /// 参照されたモデルテンプレートが存在しない。
    #[error("モデルテンプレート `{0}` は登録されていません")]
    ModelTemplateNotFound(String),

    /// 参照された役職が存在しない（Spec 14）。
    ///
    /// **表示側はこれをエラーとして出さない。** 役職が引けないときは表示ごと
    /// 省く（`role_contract` 凍結 5）ので、この型が利用者に届くのは
    /// 「削除しようとした役職が既に無い」のような明示操作のときだけ。
    #[error("役職 `{0}` は登録されていません")]
    RoleNotFound(String),

    /// 指定 ID の予定が存在しない（Spec 07）。
    #[error("予定 `{0}` は登録されていません")]
    ScheduleNotFound(String),

    /// 予定の再現規則が不正（`hour > 23` / `minute > 59` / `everyMinutes == 0`）。
    #[error("予定の再現規則が不正です: {reason}")]
    InvalidSchedule {
        /// 不正と判断した具体的な理由。
        reason: String,
    },

    /// `schedules.json` が JSON として読めないため、予定の書き込みを保護している。
    ///
    /// 読めなかったファイルへ上書きすると、利用者が直せば戻ったはずの予定を
    /// 消すことになる。ファイルを直すか削除すれば次の起動から書き込める。
    #[error(
        "schedules.json が壊れているため予定を変更できません。\
         ファイルを修正するか削除してください: {reason}"
    )]
    ScheduleStoreBlocked {
        /// 読み込みが失敗した理由。
        reason: String,
    },

    /// 指定 ID の会話セッションが `sessions.redb` に存在しない（Spec 12）。
    #[error("会話 `{0}` は保存されていません")]
    SessionNotFound(String),

    /// 飛行中のターンがあるため会話を切り替えられない（Spec 12 の不変条件 11）。
    ///
    /// 飛行中に切り替えると、**答えが別のセッションへ着地する** — 頼んだ会話と
    /// 答えが載る会話が食い違い、しかも画面上その 2 つは区別が付かない。
    /// 自動で打ち切ってから切り替えることはしない（線は人が引く）。
    #[error(
        "飛行中のターンが {in_flight} 件あるため会話を切り替えられません。\
         「■ 停止」または「全ターン停止」で止めてから切り替えてください"
    )]
    SessionSwitchBlocked {
        /// 飛行中のターン数。
        in_flight: usize,
    },

    /// 会話の保存先（`sessions.redb`）の操作に失敗した（Spec 12）。
    ///
    /// **どの操作で落ちたか**を必ず伴わせる。redb の失敗は開く・読む・書く・
    /// commit のどこでも起きえて、原因（ロック競合 / 破損 / 権限）が段によって違う。
    #[error("会話の保存先 `{path}` の{operation}に失敗しました: {reason}")]
    SessionStore {
        /// 対象パス。
        path: String,
        /// 失敗した操作（開く / 読み込み / 書き込み / 確定）。
        operation: &'static str,
        /// redb 側から得た説明。
        reason: String,
    },

    /// トークン予算の天井として受け付けられない値（Spec 13）。
    ///
    /// `0` は「即打ち切りの村」ではなく不正値（`token_budget` 契約の ceiling —
    /// 0 のマジック値を作らない。天井なしは `None` で表現する）。読み込み時の
    /// `Some(0) → None` 正規化は外部編集の遡及回収であって、設定経路の代役では
    /// ない — ここで受け付けて黙って倒すと「保存したのに別の値になる」。
    // 文言は UI の語彙（トークン制限 / 制限なし）に合わせる。内部の設計語は
    // 「天井」のまま（settings_contract の用語境界。接地／グラウンディングと同型）。
    #[error("トークン制限に 0 は設定できません。制限を外すには「制限なし」を選んでください")]
    InvalidTokenBudget,

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

    /// 単価表を取得できなかった（Spec 41）。
    ///
    /// **`ConfigIo` へ畳まない。** あちらは `source` を表示に出さないので、
    /// 「取得先が未設定」「https でない」「通信に失敗」「表が壊れている」が
    /// **すべて「設定ファイルの入出力に失敗」という嘘の 1 文へ落ちる**
    /// （実際には `pricing.json` は壊れていない）。**次の手が理由ごとに違う**
    /// ので、理由を本文で運ぶ。
    #[error("単価表を取得できませんでした: {reason}")]
    PricingFetch {
        /// 取得できなかった理由（そのまま画面へ出る）。
        reason: String,
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

    /// 黒板の付箋を消せなかった（Spec 無し・2026-08-12 の UI 追加）。
    ///
    /// **完全削除へは倒さない。** ごみ箱が使えない環境（一部の Linux 構成・
    /// ネットワークドライブ）でこれが出る。`file` ツールの remove と同じ規律で、
    /// **取り消せない操作へ勝手に格上げしない**。
    #[error("黒板の付箋 `{name}` をごみ箱へ移せませんでした: {reason}")]
    BlackboardDeleteFailed {
        /// 対象のファイル名。
        name: String,
        /// 失敗の理由。
        reason: String,
    },

    /// エージェント ID などの識別子が命名規約に反しており、パスとして安全でない。
    #[error("識別子 `{value}` は使用できません（許可: 英数字・`-`・`_`、1〜64 文字）")]
    UnsafeIdentifier {
        /// 拒否した入力値。
        value: String,
    },

    /// アイコン画像が受け入れ条件（WebP 形式・サイズ上限）を満たさない。
    ///
    /// 変換は UI 層の責務（canvas で WebP 化してから送る契約）。コアは検証だけを持ち、
    /// 任意のバイト列が IPC 経由でワークスペースへ書かれる経路を塞ぐ。
    #[error("アイコン画像を受け付けられません: {reason}")]
    InvalidIcon {
        /// 拒否した具体的な理由。
        reason: String,
    },

    /// 添付画像が受け入れ条件を満たさない（`attachment_contract` 凍結 4 / Spec 23）。
    ///
    /// アイコンとは**別の変種**にする — 上限も文言も違うものを 1 つに畳むと、
    /// 画面の辞書が「どちらの上限の話か」を区別できなくなる。共有するのは
    /// WebP のマジックバイト判定（`attachment::is_webp`）だけ。
    // 文言は UI の語彙で書く（このメッセージは利用者に見える）。
    #[error("添付を受け付けられません: {reason}")]
    InvalidAttachment {
        /// 拒否した具体的な理由。
        reason: String,
    },

    /// 宛先のワイヤがその種別の添付を運べない（Spec 36 D2 / `carries` 表）。
    ///
    /// **[`Self::InvalidAttachment`] と別の変種にする — 人の次の手が違う。**
    /// あちらは「そのファイルが条件を満たさない」（次の手 = 別のファイル）、
    /// こちらは「ファイルは正しいが宛先が受けない」（次の手 = **別の宛先**）。
    /// 1 つに畳むと、画面が「小さくすれば送れる」と読める文言しか出せなくなる
    /// （`AGENT_NOT_FOUND` と `EXTERNAL_RECEPTION_UNSET` を分けたのと同じ判断）。
    ///
    /// **これは構造の話で、モデルの受理とは層が違う。** ワイヤに書ける形が
    /// あっても受理しないモデルは 400 を返し、そちらは画面へ別経路で出る。
    // 文言は UI の語彙で書く（このメッセージは利用者に見える）。
    #[error("この宛先へは{kind}を送れません（接続先: {provider}）。{hint}")]
    AttachmentNotCarried {
        /// 種別の表示名（画像 / 音声 / 動画 / PDF）。
        kind: String,
        /// ワイヤの名前（`turn:` 行の `backend=` と同じ語）。
        provider: String,
        /// 次の手（どの接続先なら運べるか）。
        hint: String,
    },

    /// 利用者の呼び名が受け入れ条件を満たさない（`user_identity_contract` 凍結 4）。
    ///
    /// 呼び名は封筒 `【送り手: {名前}】` としてサーヴァントのプロンプトと履歴の
    /// 両方へ入るので、書式が壊れると 1 つの発話が 2 つに読める。**注意書きでは
    /// 塞げない** — `api_key_env` は 4 箇所に注意文を置いて型は素の `String` の
    /// ままで、実キーが平文で保存された。
    ///
    /// **`reason` に入力値そのものを載せない。** 拒否の過程で壊れた値を再放流
    /// しないため、載せてよいのは規則と字数だけ。
    // 文言は UI の語彙で書く（`InvalidTokenBudget` と同じ。settings_contract の
    // 用語境界 — このメッセージは利用者に見える）。
    #[error("呼び名を受け付けられません: {reason}")]
    InvalidUserName {
        /// 拒否した具体的な理由。**入力値は含めない。**
        reason: String,
    },

    /// 外部からの依頼を受ける窓口が未設定（Spec 25 D2）。
    ///
    /// **「窓口が消えた」とは別のエラーにする** — 前者は設定し直す操作、
    /// 後者は初めて設定する操作で、人がとる次の手が違う。窓口が削除済みの
    /// 場合は [`Self::AgentNotFound`] が返る（`mcp_server_contract` 凍結 7）。
    #[error("外部からの依頼を受け取る窓口が設定されていません（システム設定で選んでください）")]
    ExternalReceptionUnset,

    /// 別の外部依頼を処理中（Spec 25 D7 — 同時 1 本）。
    ///
    /// **待たせずに即断る。** 待つと、村が自分自身を MCP サーバーとして
    /// 登録した閉路のデッドロックが `ask_timeout` ぶん居座り、呼ぶ側からは
    /// 「重い依頼で遅い」と区別が付かなくなる。
    #[error("別の外部依頼を処理中です。終わってからもう一度お試しください")]
    ExternalBusy,

    /// OS の資格情報ストアの操作に失敗した。
    ///
    /// **秘密そのものはこのエラーに載せない。** 保管の失敗を伝えるために
    /// 保管対象を露出させたら本末転倒になる。
    #[error("資格情報ストアの{operation}に失敗しました: {message}")]
    SecretStore {
        /// 失敗した操作（取得 / 保存 / 削除）。
        operation: &'static str,
        /// OS 側から得た説明。秘密は含まない。
        message: String,
    },

    /// 認証が必要なテンプレートなのに、資格情報ストアにキーが登録されていない。
    #[error("モデルテンプレート `{template}` の API キーが未登録です（「モデルテンプレートを管理」の画面から登録してください）")]
    CredentialMissing {
        /// 対象テンプレートの表示名。
        template: String,
    },

    /// LLM 境界の失敗。詳細な分類は [`crate::llm::LlmError`] が持つ。
    #[error(transparent)]
    Llm(#[from] crate::llm::LlmError),

    /// MCP サーバーとのやり取りに失敗した。
    ///
    /// MCP サーバーは外部コマンドであり、未インストール・パス違い・権限で
    /// 普通に落ちる。**どのサーバーか**を必ず伴わせる — 複数台繋がっている状態で
    /// 「MCP が失敗しました」とだけ言われても、利用者は直しようがない。
    #[error("MCP サーバー `{server}` との通信に失敗しました: {message}")]
    Mcp {
        /// 対象サーバーの設定上の名前。
        server: String,
        /// 失敗の説明。
        message: String,
    },

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
            Self::DuplicateAgentName(_) => "DUPLICATE_AGENT_NAME",
            Self::ModelTemplateNotFound(_) => "MODEL_TEMPLATE_NOT_FOUND",
            Self::RoleNotFound(_) => "ROLE_NOT_FOUND",
            Self::ScheduleNotFound(_) => "SCHEDULE_NOT_FOUND",
            Self::InvalidSchedule { .. } => "INVALID_SCHEDULE",
            Self::ScheduleStoreBlocked { .. } => "SCHEDULE_STORE_BLOCKED",
            Self::SessionNotFound(_) => "SESSION_NOT_FOUND",
            Self::SessionStore { .. } => "SESSION_STORE_FAILED",
            Self::SessionSwitchBlocked { .. } => "SESSION_SWITCH_BLOCKED",
            Self::InvalidTokenBudget => "INVALID_TOKEN_BUDGET",
            Self::InvalidTopology { .. } => "INVALID_TOPOLOGY",
            Self::AlreadyRunning { .. } => "ALREADY_RUNNING",
            Self::NotRunning { .. } => "NOT_RUNNING",
            Self::MailboxFull { .. } => "MAILBOX_FULL",
            Self::PricingFetch { .. } => "PRICING_FETCH",
            Self::ConfigIo { .. } => "CONFIG_IO",
            Self::BlackboardDeleteFailed { .. } => "BLACKBOARD_DELETE_FAILED",
            Self::UnsafeIdentifier { .. } => "UNSAFE_IDENTIFIER",
            Self::InvalidIcon { .. } => "INVALID_ICON",
            Self::InvalidAttachment { .. } => "INVALID_ATTACHMENT",
            Self::AttachmentNotCarried { .. } => "ATTACHMENT_NOT_CARRIED",
            Self::InvalidUserName { .. } => "INVALID_USER_NAME",
            Self::ExternalReceptionUnset => "EXTERNAL_RECEPTION_UNSET",
            Self::ExternalBusy => "EXTERNAL_BUSY",
            Self::SecretStore { .. } => "SECRET_STORE_FAILED",
            Self::CredentialMissing { .. } => "CREDENTIAL_MISSING",
            // LLM 境界のコードはそのまま透過させ、UI 側で 1 つの体系として扱えるようにする。
            Self::Llm(err) => err.code(),
            Self::Mcp { .. } => "MCP_FAILED",
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

    /// エージェントの稼働を降ろすべき失敗か。
    ///
    /// **[`Self::is_retryable`] とは別の問い。** 再試行の可否は「**この 1 件**を
    /// もう一度投げて意味があるか」で、こちらは「**以後のすべての依頼**が同じく
    /// 失敗するか」を問う。設定不備（API キー不在）は後者に当たるので降ろすが、
    /// 出力上限（[`crate::llm::LlmError::OutputTruncated`]）は当たらない —
    /// **その依頼の生成物が大きすぎただけ**で、次が小さければ普通に通る。
    ///
    /// この 2 つを 1 つの述語に畳んでいると、上限超えのたびにエージェントが
    /// 停止する（2026-07-31 に分離。failures.md #40）。
    pub fn stops_the_agent(&self) -> bool {
        match self {
            Self::Llm(crate::llm::LlmError::OutputTruncated { .. }) => false,
            other => !other.is_retryable(),
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

    /// 出力上限は「この依頼は無理」であって「このエージェントは無理」ではない。
    #[test]
    fn output_truncation_is_not_retryable_but_keeps_the_agent_running() {
        let err = CoreError::Llm(crate::llm::LlmError::OutputTruncated {
            limit: 4_096,
            usage: crate::llm::Usage::default(),
        });
        assert!(!err.is_retryable(), "同じ依頼を再送しても同じ所で切れる");
        assert!(
            !err.stops_the_agent(),
            "依頼が小さければ次は通るので、稼働は降ろさない"
        );

        // 設定不備は対照的に、以後のすべての依頼が同じく失敗する。
        let config = CoreError::Llm(crate::llm::LlmError::Config("キー不在".into()));
        assert!(config.stops_the_agent());
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
