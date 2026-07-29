//! ドメインの名詞（Data-First Grounding）。
//!
//! ここに定義される型が、Rust / IPC / TypeScript の三者で共有される唯一の契約である。
//! フィールドを増減させた場合は `apps/gui-tauri/src/types.ts` を必ず同時に更新すること。

use serde::{Deserialize, Serialize};
use std::fmt;

/// 識別子として許可する最大文字数。ファイルシステム上のディレクトリ名に使うため制限する。
const MAX_IDENT_LEN: usize = 64;

/// 識別子が「ファイル名として安全か」を判定する。
///
/// 許可するのは英数字・`-`・`_` のみ。`.` や `/` を弾くことで、
/// エージェント ID を経由したパストラバーサルを型の入口で封じる。
pub fn is_safe_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_IDENT_LEN
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// エージェントの一意識別子。
///
/// `String` の newtype にすることで、モデルテンプレート ID や RAG ソース名との
/// 取り違えをコンパイル時に検出する。ワイヤ表現は透過的な文字列。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentId(String);

impl AgentId {
    /// 検証なしで生成する。永続化された値の復元など、既に検証済みの経路で使う。
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// 内部文字列を借用する。
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// ファイルシステム上のディレクトリ名として安全か判定する。
    pub fn is_safe(&self) -> bool {
        is_safe_identifier(&self.0)
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for AgentId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for AgentId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// モデルテンプレートの一意識別子。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ModelTemplateId(String);

impl ModelTemplateId {
    /// 検証なしで生成する。
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// 内部文字列を借用する。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModelTemplateId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ModelTemplateId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for ModelTemplateId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// エージェントのライフサイクル状態。
///
/// 失敗理由をこの enum に持たせず [`AgentSnapshot::last_error`] へ分離しているのは、
/// ワイヤ表現を `"running"` のような素の文字列に保ち、UI 側の分岐を単純にするため。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    /// 停止しており、受信箱も存在しない。
    #[default]
    Idle,
    /// 起動処理の最中。
    Starting,
    /// 稼働中。受信箱がメッセージを受け付ける。
    Running,
    /// 停止処理の最中。
    Stopping,
    /// 直前の実行が失敗して停止した。詳細は `last_error` に入る。
    Failed,
}

impl AgentStatus {
    /// 稼働時間の計測対象となる状態か。
    pub fn is_active(self) -> bool {
        matches!(self, Self::Starting | Self::Running)
    }
}

/// エージェントの永続的な設定。ユーザーが編集する対象。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSpec {
    /// 一意識別子。設定ファイル置き場のディレクトリ名にもなる。
    pub id: AgentId,
    /// 画面に表示する名前。
    pub name: String,
    /// 使用するモデルテンプレート。
    pub model_template_id: ModelTemplateId,
    /// 参照する RAG ソース名の一覧。
    #[serde(default)]
    pub rag_sources: Vec<String>,
    /// このエージェントが発話を届けられる相手。有向グラフの出辺。
    #[serde(default)]
    pub connected_agents: Vec<AgentId>,
    /// 左ペインでの表示順。小さいほど上。
    #[serde(default)]
    pub order: u32,
    /// 同梱ツール（grep / diff）が読める作業フォルダの絶対パス。
    ///
    /// `None` なら未設定で、ツールは「設定されていない」と答えるだけになる。
    /// エージェントはプロンプトインジェクションを受けうるため、読める範囲は
    /// **ユーザーが明示したフォルダ**に限る。範囲の強制は設定値の検査ではなく
    /// ツール実行時の canonicalize + 前方一致で行う（symlink 経由の脱出は
    /// パス文字列の検査では塞げない）。
    #[serde(default)]
    pub work_dir: Option<String>,
    /// 1 回の発話処理で許すツール実行の回数（エージェント個別の上乗せ）。
    ///
    /// `None` なら [`crate::orchestrator::OrchestratorConfig::max_tool_iterations`]
    /// の既定値。コーディング用エージェントは調査（grep / fd / 読み比べ）の
    /// ツール往復が多く、既定の上限では調査の途中で打ち切られやすい。
    /// 上限に達したときの応答が空にならない保証は別にある
    /// （orchestrator の空応答フォールバック）ので、これは打ち切り頻度の調整。
    #[serde(default)]
    pub max_tool_iterations: Option<u8>,
    /// 提示する同梱ツール名の集合（Spec 02）。
    ///
    /// - `None` = 「既定に従う」— 全同梱ツールを提示。新しい同梱ツールが
    ///   増えれば自動で提示される。**新規作成時の保存値はこちら**
    /// - `Some(list)` = 「必要な道具だけ」— 列挙した分だけ提示。
    ///   新しい同梱ツールは自動で増えない（それが明示の意味）。空なら 0 本
    ///
    /// 対象は同梱ツールのみ（MCP 由来・転送・委譲は対象外）。
    /// 作業フォルダ未設定によるファイル系の自動除外は、このリストより優先される。
    #[serde(default)]
    pub enabled_tools: Option<Vec<String>>,
    /// 広場ログ（他エージェント同士の会話）を受け取るか（Spec 03）。
    ///
    /// **受信側だけの設定。** `false` にしても自分の発話は他エージェントの
    /// 広場ログに従来どおり載る — プライバシー機能ではなくコスト機能で、
    /// 毎ターン最大 `room_log_window × room_log_excerpt_chars` の固定費を絞る。
    #[serde(default = "default_true")]
    pub hears_room_log: bool,
}

impl AgentSpec {
    /// 最低限の設定でエージェント定義を作る。
    pub fn new(
        id: impl Into<AgentId>,
        name: impl Into<String>,
        model_template_id: impl Into<ModelTemplateId>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            model_template_id: model_template_id.into(),
            rag_sources: Vec::new(),
            connected_agents: Vec::new(),
            order: 0,
            work_dir: None,
            max_tool_iterations: None,
            enabled_tools: None,
            hears_room_log: true,
        }
    }
}

/// 秘密の取得元。
///
/// 「どこから取るか」だけを保持し、**秘密そのものを保持できるバリアントを持たない**。
/// 平文の設定ファイルに秘密が入りうる経路を、型の段階で存在させないための形。
///
/// [`Unset`](Self::Unset) と [`NotRequired`](Self::NotRequired) を分けているのは、
/// **「まだ入れていない」と「要らない」は別の状態**だから。1 つにまとめると、
/// キー未登録のテンプレートが「認証不要」と解釈され、認証ヘッダ無しのリクエストが
/// 外部へ出ていく。ローカルで捕まえられるはずの設定不備が、サーバー側の 401 になる。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialSource {
    /// 未設定。既定値。
    ///
    /// 旧版の `"none"` もここへ落とす。旧 `"none"` は実質「まだ入れていない」であり、
    /// 「認証不要」と読み替えると未登録のまま外部へ送ることになる。
    #[default]
    #[serde(alias = "none")]
    Unset,
    /// 認証不要であるとユーザーが明示した。ローカル推論サーバなど。
    NotRequired,
    /// OS の資格情報ストアから取得する。キーはテンプレート ID。
    Keyring,
}

impl CredentialSource {
    /// 送信前に設定不備として弾くべき状態か。
    pub fn is_unresolved(self) -> bool {
        matches!(self, Self::Unset)
    }
}

/// LLM 接続設定のテンプレート。複数登録して各エージェントから参照する。
///
/// **この構造体は秘密を保持しない。** 保持するのは
/// [`ModelTemplate::credential`]、すなわち「どこから取るか」だけ。
/// 設定は平文のファイルに保存されるため、秘密を書ける場所を型から取り除いてある。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTemplate {
    /// 一意識別子。
    pub id: ModelTemplateId,
    /// 画面に表示する名前。
    pub name: String,
    /// API の base URL（例: `https://api.openai.com/v1`）。
    ///
    /// エンドポイントの完全形ではなく base を保持するのは、`/chat/completions` と
    /// `/messages` のどちらを付けるかがプロバイダごとに違うため。パスの決定は adapter の責務。
    pub base_url: String,
    /// プロバイダに渡すモデル名（例: `gpt-4o`）。
    pub model: String,
    /// モデルのコンテキスト長。プロンプト構築時の切り詰め判断に使う。
    pub context_length: u32,
    /// サンプリング温度。
    ///
    /// **`None` なら送らない。** 新しめのモデルは `temperature` 非対応で、
    /// 送ると 400 を返す。既定値を勝手に補うとそのモデルで恒久的に失敗する。
    #[serde(default)]
    pub temperature: Option<f32>,
    /// 1 応答あたりの最大出力トークン数。
    pub max_output_tokens: u32,
    /// 認証情報の取得元。
    ///
    /// 旧版の `apiKeyEnv`（環境変数名）は廃止した。読み込み時に未知フィールドとして
    /// 無視され、`credential` は既定の [`CredentialSource::Unset`] になる。
    /// 移行にあたって利用者はキーを画面から入れ直すことになるが、
    /// 旧フィールドは名前しか持っておらず、そこから移せる値が存在しない。
    #[serde(default)]
    pub credential: CredentialSource,
    /// ワイヤプロトコルの明示指定。`None` なら `base_url` から自動判定する。
    #[serde(default)]
    pub provider: Option<crate::llm::Provider>,
    /// ツール呼び出し（function calling）を使うか。
    ///
    /// `tool_choice` を実装していない互換サーバ向けに `false` へ倒すと、
    /// スキーマをプロンプトへ載せるフォールバック経路に切り替わる。
    #[serde(default = "default_true")]
    pub use_tools: bool,
    /// 推論の深さ。`None` なら送らない。
    #[serde(default)]
    pub effort: Option<crate::llm::Effort>,
    /// Google 検索による接地を有効にするか。
    ///
    /// **[`crate::llm::Provider::Gemini`] を明示したテンプレートでのみ効く。**
    /// OpenAI 互換の口は `google_search` を `400 Invalid tool type` で拒否するため、
    /// 互換経路のまま真にしても接地は起きない（UI 側でも Gemini 選択時だけ出す）。
    ///
    /// 関数呼び出しとは併用できる。検索 → 関数呼び出しが 1 応答の中で連鎖するので、
    /// これを有効にしても `transfer_to_*` による委譲は止まらない（実測 2026-07-29）。
    #[serde(default)]
    pub google_search: bool,
    /// 1 リクエストのタイムアウト秒数。
    #[serde(default = "default_timeout_secs")]
    pub request_timeout_secs: u32,
    /// 最大試行回数（初回を含む）。
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
}

/// `use_tools` の serde 既定値。
fn default_true() -> bool {
    true
}

/// `request_timeout_secs` の serde 既定値。
fn default_timeout_secs() -> u32 {
    120
}

/// `max_retries` の serde 既定値。
fn default_max_retries() -> u32 {
    3
}

impl ModelTemplate {
    /// 実際に使われるワイヤプロトコル。`provider` が未指定なら `base_url` から判定する。
    pub fn effective_provider(&self) -> crate::llm::Provider {
        self.provider
            .unwrap_or_else(|| crate::llm::Provider::detect(&self.base_url))
    }

    /// Google 検索による接地が**実際に起きる**か。
    ///
    /// `google_search` が真でも、ワイヤが Gemini ネイティブでなければ接地は起きない
    /// （OpenAI 互換の口は `google_search` を 400 で拒否する）。UI はこの組み合わせを
    /// 作らせないが、`world.json` を直接編集すれば作れてしまう。
    ///
    /// **フラグではなくこの関数を判定に使うこと。** フラグだけを見てシステムプロンプトに
    /// 「検索で裏を取れます」と書くと、検索できないモデルにできると教えることになる。
    /// 接地の告知は「持っていない情報を埋める」ための節なので、そこで嘘をつくと
    /// 処方そのものが毒になる。
    pub fn grounding_active(&self) -> bool {
        self.google_search && self.effective_provider() == crate::llm::Provider::Gemini
    }

    /// 汎用的な既定値でテンプレートを作る。
    pub fn new(
        id: impl Into<ModelTemplateId>,
        name: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            base_url: "https://api.openai.com/v1".to_owned(),
            model: model.into(),
            context_length: 128_000,
            temperature: None,
            max_output_tokens: 4_096,
            credential: CredentialSource::Unset,
            provider: None,
            use_tools: true,
            effort: None,
            google_search: false,
            request_timeout_secs: default_timeout_secs(),
            max_retries: default_max_retries(),
        }
    }
}

/// 発話の送り手・受け手。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Endpoint {
    /// 人間のオペレーター。
    User,
    /// オーケストレーター自身（システム通知）。
    System,
    /// 登録済みエージェント。
    Agent {
        /// 対象エージェント ID。
        id: AgentId,
    },
}

impl Endpoint {
    /// エージェントを指す場合、その ID を返す。
    pub fn agent_id(&self) -> Option<&AgentId> {
        match self {
            Self::Agent { id } => Some(id),
            _ => None,
        }
    }
}

/// エージェント間・ユーザー間でやり取りされる 1 発話。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessage {
    /// 発話の一意 ID（UUID v4）。
    pub id: String,
    /// 送り手。
    pub from: Endpoint,
    /// 受け手。
    pub to: Endpoint,
    /// 本文。
    pub content: String,
    /// この発話の生成に要したトークン数（prompt + completion）。
    pub tokens: u32,
    /// UNIX エポックからのミリ秒。
    pub ts_ms: u64,
    /// ユーザー入力を起点とした転送回数。無限往復を止めるための燃料。
    pub hop: u8,
    /// 同報の全宛先（受信者自身を含む）。単独宛では空。
    ///
    /// 同報であることが受信者に見えないと、各エージェントは「自分しか
    /// 聞いていない」と判断して接続先へ律儀に転送し、反響が起きる。
    /// この情報は**宛先本人の封筒にだけ**載る — 宛先外へは配送自体が
    /// 行われないので、発話の存在ごと見えない。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub co_recipients: Vec<AgentId>,
}

impl AgentMessage {
    /// 発話を新規生成する。ID と時刻は自動採番される。
    pub fn new(from: Endpoint, to: Endpoint, content: impl Into<String>, hop: u8) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            from,
            to,
            content: content.into(),
            tokens: 0,
            ts_ms: now_ms(),
            hop,
            co_recipients: Vec::new(),
        }
    }
}

/// UI へ渡すエージェントの現在像。仕様と実行時統計を 1 枚に畳んだ読み取り専用ビュー。
///
/// 形は要件の入力例（`id` / `name` / `model` / `status` / `uptime_secs` /
/// `total_tokens` / `rag_sources` / `connected_agents`）に一致させてある。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSnapshot {
    /// 一意識別子。
    pub id: AgentId,
    /// 表示名。
    pub name: String,
    /// 解決済みのモデル名。テンプレートが失われている場合は `"<unknown>"`。
    pub model: String,
    /// 参照元のモデルテンプレート ID。
    pub model_template_id: ModelTemplateId,
    /// ライフサイクル状態。
    pub status: AgentStatus,
    /// 累積稼働秒数。停止しても保持され、再起動で加算される。
    pub uptime_secs: u64,
    /// 累積トークン数。
    pub total_tokens: u64,
    /// 参照 RAG ソース。
    pub rag_sources: Vec<String>,
    /// 発話を届けられる相手。
    pub connected_agents: Vec<AgentId>,
    /// 左ペインでの表示順。
    pub order: u32,
    /// 同梱ツール（grep / diff）の作業フォルダ。未設定なら `None`。
    pub work_dir: Option<String>,
    /// ツール実行回数の個別上限。`None` なら既定値。
    pub max_tool_iterations: Option<u8>,
    /// 提示する同梱ツール名。`None` なら既定（全提示）。
    pub enabled_tools: Option<Vec<String>>,
    /// 広場ログを受け取るか。
    pub hears_room_log: bool,
    /// 直近の失敗（あれば）。`status == Failed` の理由表示に使う。
    pub last_error: Option<crate::error::ErrorPayload>,
}

/// エージェントごとの設定ファイル種別。
///
/// GUI からは列挙値のみを受け取り、実ファイル名の解決はコア層が行う。
/// 任意のファイル名を IPC で受け取らないことで、書き込み先を閉じた集合に保つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigFileKind {
    /// エージェントの能力・振る舞いの定義。
    Skill,
    /// 長期記憶。
    Memory,
    /// 構成・制約の宣言。
    Construct,
    /// エージェント別の MCP サーバー宣言（Spec 02）。
    ///
    /// 他の 3 つと違い自由テキストではない — 書き込み時に JSON パース検証が
    /// あり、失敗すると保存拒否（mcp_contract の失敗二分類 (1)）。
    /// プロンプト素材ではないため `compose_system_prompt` には入らない。
    Mcp,
}

impl ConfigFileKind {
    /// 実ファイル名を返す。
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Skill => "SKILL.md",
            Self::Memory => "Memory.md",
            Self::Construct => "Construct.md",
            Self::Mcp => "mcp.json",
        }
    }

    /// 全種別。GUI のタブ生成に使う。
    pub fn all() -> [Self; 4] {
        [Self::Skill, Self::Memory, Self::Construct, Self::Mcp]
    }
}

/// トポロジーの 1 本の有向辺。Vue Flow のエッジに 1 対 1 対応する。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopologyEdge {
    /// 発話元。
    pub source: AgentId,
    /// 発話先。
    pub target: AgentId,
}

/// 現在時刻を UNIX エポックからのミリ秒で返す。
///
/// システム時計が 1970 年より前を指す異常系では 0 を返し、パニックを避ける。
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_id_is_transparent_on_the_wire() {
        let id = AgentId::from("agent_01");
        assert_eq!(serde_json::to_string(&id).unwrap(), r#""agent_01""#);
    }

    #[test]
    fn status_serializes_as_bare_snake_case_string() {
        assert_eq!(
            serde_json::to_string(&AgentStatus::Running).unwrap(),
            r#""running""#
        );
    }

    #[test]
    fn identifier_guard_rejects_path_traversal() {
        assert!(is_safe_identifier("agent_01"));
        assert!(is_safe_identifier("planner-01"));
        assert!(!is_safe_identifier("../etc"));
        assert!(!is_safe_identifier("agents/01"));
        assert!(!is_safe_identifier(""));
        assert!(!is_safe_identifier(&"a".repeat(MAX_IDENT_LEN + 1)));
    }

    #[test]
    fn template_has_no_field_that_can_hold_a_secret() {
        // 秘密が入りうる場所を型から消したことを、直列化結果で固定する。
        // 平文の設定ファイルに秘密が現れる経路は「置ける場所があること」から始まる。
        let json = serde_json::to_value(ModelTemplate::new("tpl", "既定", "gpt-4o")).unwrap();
        let object = json.as_object().unwrap();

        assert!(!object.contains_key("apiKey"));
        assert!(!object.contains_key("apiKeyEnv"));
        assert_eq!(object["credential"], "unset");
    }

    #[test]
    fn old_templates_with_api_key_env_still_load() {
        // 旧版のファイルを開けなくしない。未知フィールドは無視し、
        // credential は既定（認証不要）へ落ちる。
        let legacy = r#"{
            "id": "tpl", "name": "旧設定",
            "baseUrl": "https://api.anthropic.com/v1",
            "model": "claude-sonnet-5", "contextLength": 128000,
            "temperature": null, "maxOutputTokens": 4096,
            "apiKeyEnv": "ANTHROPIC_API_KEY",
            "provider": "anthropic", "useTools": true, "effort": null,
            "requestTimeoutSecs": 120, "maxRetries": 3
        }"#;

        let template: ModelTemplate = serde_json::from_str(legacy).expect("旧形式も開けること");
        assert_eq!(template.credential, CredentialSource::Unset);
        assert_eq!(template.model, "claude-sonnet-5");
    }

    #[test]
    fn endpoint_is_a_tagged_union_for_typescript() {
        let ep = Endpoint::Agent {
            id: AgentId::from("agent_02"),
        };
        assert_eq!(
            serde_json::to_value(&ep).unwrap(),
            serde_json::json!({ "kind": "agent", "id": "agent_02" })
        );
    }

    /// 接地は「フラグが真」ではなく「実際に Gemini へ出る」ときだけ有効なこと。
    ///
    /// UI はこの組み合わせを作らせないが、world.json の直接編集では作れる。
    /// フラグだけで判定すると、検索できないモデルに「検索できます」と教える。
    #[test]
    fn grounding_is_inactive_unless_the_wire_is_actually_gemini() {
        let mut template = ModelTemplate::new("tpl", "ジェミー", "gemini-3.6-flash");
        template.base_url = "https://generativelanguage.googleapis.com/v1beta".into();
        template.google_search = true;

        // provider 未指定 = 自動判定 = OpenAI 互換。接地は起きない。
        assert_eq!(
            template.effective_provider(),
            crate::llm::Provider::OpenAiCompat
        );
        assert!(!template.grounding_active(), "互換経路では接地しない");

        // 明示選択して初めて有効になる。
        template.provider = Some(crate::llm::Provider::Gemini);
        assert!(template.grounding_active());

        // チェックを外せば当然無効。
        template.google_search = false;
        assert!(!template.grounding_active());
    }

    #[test]
    fn config_file_kinds_map_to_expected_names() {
        let names: Vec<_> = ConfigFileKind::all()
            .into_iter()
            .map(ConfigFileKind::file_name)
            .collect();
        assert_eq!(names, vec!["SKILL.md", "Memory.md", "Construct.md", "mcp.json"]);
    }
}
