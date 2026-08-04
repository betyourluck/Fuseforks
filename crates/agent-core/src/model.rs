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

    /// 表示語彙。UI の `STATUS_LABELS`（types.ts）と**同一**であること。
    ///
    /// 顔ぶれ（Spec 06）はこの語彙でプロンプトに載る。画面とプロンプトで
    /// 同じ相手が違う言葉で呼ばれると、利用者とエージェントの会話が
    /// 噛み合わなくなる（「停止中って出てますよ」「こちらでは Idle です」）。
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "停止中",
            Self::Starting => "起動中",
            Self::Running => "稼働中",
            Self::Stopping => "停止処理中",
            Self::Failed => "失敗",
        }
    }
}

/// 役職の一意識別子（Spec 14）。
///
/// **表示名ではなく id で指す。** 役職を改名してもサーヴァント側の参照は
/// 切れない — 変わりやすいもの（表示名）と変わりにくいもの（id）を同じ語に
/// 縛らないため（`AgentId` と表示名の関係と同じ形。`failures.md` #51 一般化 2）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentRoleId(String);

impl AgentRoleId {
    /// 検証なしで生成する。
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// 内部文字列を借用する。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentRoleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for AgentRoleId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for AgentRoleId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// 役職バッジの色（Spec 14）。**閉じた列挙**で、実際の色値は持たない。
///
/// # なぜ自由な色文字列にしないか
///
/// 1. **配色は `style.css` の `@theme` 1 箇所で決める**という規律を保つため。
///    `world.json` に生の色が入ると、配色がテーマの外へ漏れる
/// 2. **読みやすさを構造で保証するため。** 明度と彩度は実装側で固定し、
///    変わるのは色相だけ（`avatarHue` と同じ形）。自由入力にすると
///    暗い背景に暗い色を選べてしまい、読めないバッジが作れる
/// 3. 村を配ったときに**受け取った側のテーマでも成立する**
///
/// `None` = 色なし（既定の枠線と字色。今までのバッジと同じ見た目）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoleColor {
    /// 赤。
    Red,
    /// 橙。
    Orange,
    /// 琥珀。
    Amber,
    /// 緑。
    Green,
    /// 青緑。
    Teal,
    /// 青。
    Blue,
    /// 菫。
    Violet,
    /// 桃。
    Pink,
}

/// 役職（Spec 14）。**雛形**と**ラベル**の 2 役を兼ねる。
///
/// 兼ねるが、混ぜない — [`AgentRoleDefaults`] は**新規作成のときだけ**効き
/// （そこで `AgentSpec` と `Construct.md` へ**コピー**される）、
/// [`AgentRole::name`] は**動いている間ずっと**効く（`role_id` から**参照**で引かれる）。
///
/// この非対称が Spec 14 の核。中身を参照にすると「後から直すと全員に効く共有
/// 規則」の層が条例と 2 つになり、名前をコピーにすると改名が伝わらない。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRole {
    /// 一意識別子。
    pub id: AgentRoleId,
    /// 画面に表示する名前。バッジと顔ぶれの行に出る。
    pub name: String,
    /// 人が読む説明。**プロンプトには入らない**（`role_contract` 凍結 6）。
    ///
    /// 読み手は「どの雛形を選ぶか」を決める人だけ。顔ぶれは毎ターン・全員ぶんを
    /// 素の値段で払うので、名前（3〜5 トークン）ではなく説明（50〜200）を
    /// 載せると固定費になる。自分の役職の説明は `Construct.md` に全文入っており、
    /// そちらは安定部なのでキャッシュが効く。
    #[serde(default)]
    pub description: String,
    /// バッジの色。`None` = 色なし（既定の枠線と字色）。
    ///
    /// **`name` と同じ「参照」側**（`defaults` ではない）。色を変えると
    /// **既にいる全個体のバッジが追従する** — 表示の属性であって、
    /// 作成時にコピーされる設定ではないため（機構 3 の分割）。
    ///
    /// **プロンプトには入らない。** 顔ぶれに載るのは役職名だけで、色は
    /// 画面のためだけにある（`role_contract` 凍結 6）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<RoleColor>,
    /// 新規作成のときだけ流し込まれる既定値。
    #[serde(default)]
    pub defaults: AgentRoleDefaults,
}

/// 役職が持つ既定値（Spec 14。`role_contract` 凍結 2 の「入れる」5 欄）。
///
/// **`AgentSpec` の全 11 欄のうち、ここに来るのは 4 欄だけ**（+ `construct` は
/// ファイルなので `AgentSpec` の欄ではない）。入れない 5 欄と、その理由:
///
/// - `connected_agents` — 入れると**役職を選んだ瞬間に線が引かれる**。
///   「線は人が引く」（AdaptOrch を採らなかった根拠）が崩れる
/// - `work_dir` — 絶対パスで端末ごとに違う。村を配ると存在しないパスを指す
/// - `order` — 左ペインの並び順。役職の性質ではない
/// - `batch_start` — 一括起動の対象。運用の選択
/// - `hears_room_log` — コスト設定（Spec 03）。村の懐事情で決まる
///
/// `id` / `name` は個体固有なので対象外。**11 = 対象外 2 + 入れる 4 + 入れない 5。**
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRoleDefaults {
    /// `Construct.md` へ書き込む本文。役職の本体。
    #[serde(default)]
    pub construct: String,
    /// 使用するモデルテンプレート。`None` = 役職として意見を持たない。
    ///
    /// **ここだけ存在検査が掛かる**（`role_contract` 凍結 2）。`world.json` に
    /// 宣言された登録簿なので、無い id はその場で分かる。
    #[serde(default)]
    pub model_template_id: Option<ModelTemplateId>,
    /// 参照する RAG ソース名。
    ///
    /// **存在検査は掛けない。そのまま写す。** `RagIndex` は断片を索引した瞬間に
    /// キーが生える実行時の器で、宣言された登録簿が無い。作成時点では索引が
    /// ほぼ必ず空なので、検査すると「調査役を作る → あとで資料を食わせる」
    /// という正しい順序を壊す。
    #[serde(default)]
    pub rag_sources: Vec<String>,
    /// 提示する同梱ツール名の集合。`None` = 既定に従う（全同梱ツール）。
    #[serde(default)]
    pub enabled_tools: Option<Vec<String>>,
    /// 1 回の発話処理で許すツール実行の回数。`None` = 既定に従う。
    #[serde(default)]
    pub max_tool_iterations: Option<u8>,
}

impl AgentRoleDefaults {
    /// 既定値を `AgentSpec` へ流し込む（**新規作成のときだけ呼ぶ**）。
    ///
    /// `role_contract` 凍結 4 のとおり、**流し込みの発火点は新規作成ただ 1 つ**。
    /// 既存のサーヴァントに役職を付けるときは `role_id` だけを差し替え、
    /// この関数は呼ばない（上書きは取り消せない）。
    ///
    /// **触るのは 4 欄だけ。** `connected_agents` / `work_dir` / `order` /
    /// `batch_start` / `hears_room_log` には一切代入しない。
    ///
    /// `template_exists` はモデルテンプレートの存在判定。純関数に保つため
    /// `World` を直接受けない（テストが登録簿を組まずに書ける）。
    ///
    /// 戻りは**参照先が見つからず落とした欄の説明**。空でなければ呼び出し側が
    /// WARN を 1 行出す。**黙って落とさない** — `World::from_persisted` が
    /// 未登録先への接続を黙って落とすのは復元の場面で、こちらは人が今まさに
    /// 操作している最中なので、黙ると「入れたはずの設定が入っていない」が
    /// 見えない。`Construct.md` へ書く本文は [`Self::construct`] を直接読む
    /// （戻り値に同じものを載せると、2 つの真実ができる）。
    #[must_use = "落とした欄を捨てると「入れたはずの設定が入っていない」が見えなくなる"]
    pub fn apply_to(
        &self,
        spec: &mut AgentSpec,
        template_exists: impl Fn(&ModelTemplateId) -> bool,
    ) -> Vec<String> {
        let mut dropped = Vec::new();

        if let Some(template_id) = &self.model_template_id {
            if template_exists(template_id) {
                spec.model_template_id = template_id.clone();
            } else {
                // その欄だけ落とす。作成そのものは通す — 役職 1 つが壊れている
                // ことでサーヴァントを作れなくしない。
                dropped.push(format!("モデルテンプレート `{template_id}`"));
            }
        }

        // 検査しない（RagIndex は実行時に育つ器で、宣言された登録簿が無い）。
        if !self.rag_sources.is_empty() {
            spec.rag_sources = self.rag_sources.clone();
        }
        if self.enabled_tools.is_some() {
            spec.enabled_tools = self.enabled_tools.clone();
        }
        if self.max_tool_iterations.is_some() {
            spec.max_tool_iterations = self.max_tool_iterations;
        }

        dropped
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
    /// 一括起動（左ペインの ▶）の対象にするか。
    ///
    /// **自動起動ではない。** アプリを開いた時点で勝手に走り出すことはなく、
    /// 利用者が ▶ を押したときに「どれを起こすか」の選択だけを持つ。
    /// 起動は明示操作のまま — 開いただけで課金が始まる作りにしない。
    ///
    /// 既定は真。村の全員を起こすのが通常で、外すのは例外（重いモデルや
    /// 実験中の個体）だから、既定が偽だと ▶ が初回に何もしないボタンになる。
    ///
    /// **稼働状態ではない**（それは [`AgentStatus`]）。この 2 つを 1 つの
    /// トグルに兼ねさせていたのが元の UI で、「起動する」と「起動対象に含める」
    /// を区別できなかった。
    #[serde(default = "default_true")]
    pub batch_start: bool,
    /// この個体が**どの役職を雛形にして作られたか**（Spec 14）。
    ///
    /// **表示のためだけに持つ。** 実行経路でこの値を読んで分岐する箇所は
    /// 1 つも無い（`role_contract` 凍結 7）。設定の中身は作成時に
    /// [`AgentRoleDefaults::apply_to`] でコピー済みなので、**役職を削除しても
    /// このサーヴァントの動作は 1 ミリも変わらない**（バッジが消えるだけ）。
    ///
    /// **バッジは由来であって現在の中身を保証しない。** 作成後に
    /// `Construct.md` も `enabled_tools` も手で変えられるため、`role_id` が
    /// 「調査役」のまま中身が別、という個体が正当な操作で生まれる。これは
    /// コピー方式の必然で、**実装しても落ちないのでテストでは出ない** —
    /// ゆえに言葉のほうを実態に合わせてある（`role_contract` 凍結の外）。
    #[serde(default)]
    pub role_id: Option<AgentRoleId>,
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
            batch_start: true,
            role_id: None,
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
    ///
    /// **小さすぎる値は診断しにくい失敗を生む。** 上限を超えると本文もツール
    /// 呼び出しも成立せず、`LLM_OUTPUT_TRUNCATED` になる（失敗の方向が非対称 —
    /// 大きすぎる値は API が 400 で理由つきに弾くので気づけるが、小さすぎる値は
    /// 生成物の大きさ次第でしか表に出ない）。既定は 8,192。
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
            max_output_tokens: 8_192,
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
    /// この発話を作る過程でプロバイダが代行した接地の来歴（Spec 05 P4）。
    ///
    /// **表示層専用。読むのは人間だけで、モデルへは戻さない。**
    /// プロンプトを組む経路（`compose_room_log` / `push_exchange`）は
    /// `content` しか見ないので、ここへ足しても発話の中身は変わらない。
    /// 戻さない理由は時系列 — 接地はそのターンの中で起き、参照元は答えと
    /// 同時に返る。次ターンのプロンプトへ入れれば、それは前の話題の出典であり、
    /// モデルが今引用したい相手ではない（Spec 05 Notes 9）。
    #[serde(default, skip_serializing_if = "crate::llm::Grounding::is_empty")]
    pub grounding: crate::llm::Grounding,
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
            grounding: crate::llm::Grounding::default(),
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
    /// うち入力トークン数。キャッシュ率の分母（出力はキャッシュできない）。
    pub prompt_tokens: u64,
    /// うちプロンプトキャッシュから読まれた入力トークン数。
    ///
    /// 合計だけではキャッシュの効き具合が見えない（無キャッシュでも同じ数字）。
    /// 画面では割合として出す。
    pub cached_tokens: u64,
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
    /// 一括起動（▶）の対象か。稼働状態とは別（`status` がそちら）。
    pub batch_start: bool,
    /// どの役職を雛形にして作られたか（Spec 14）。`None` = 役職なし。
    ///
    /// **投影にも載せる理由が 2 つある。**
    ///
    /// 1. **バッジはカードに出る**（S2）。カードが描くのはこの投影なので、
    ///    ここに無いと画面に出しようがない
    /// 2. **無いと保存のたびに消える。** 設定ダイアログと一括起動トグルは
    ///    投影から `AgentSpec` を組み直して `update_agent` へ渡す作りで、
    ///    投影に無い欄は往復のたびに既定値へ落ちる。P1 の型検査がこの経路を
    ///    3 箇所で捕まえた（`AgentSpec` にだけ足して投影に足さないと、
    ///    **設定を保存した瞬間に役職が外れる**）
    pub role_id: Option<AgentRoleId>,
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
    /// エージェント別のコマンド許容規則（Spec 15 rev4）。
    ///
    /// `mcp` と同じく JSON で、書き込み時にパース検証がある。
    /// **`mcp` と違い、人と機械の両方が書く唯一の設定ファイル** —
    /// エージェントが `pending` を足すので、書き込みは全文上書きにせず
    /// `pending` だけを差分適用する（`command_tool_contract`）。
    ///
    /// **ファイル名は `run.json`** — 設定する対象（`run` ツール）の名前をそのまま
    /// 採る（`mcp.json` と同じ規則）。旧名 `shell.json` は 2026-08-05 に改名した。
    /// 実行はシェルを介さず `command` + `args` 配列のままなので、`shell` を
    /// 名乗ると**弁解の doc コメントが要る名前**になっていた。
    Run,
}

impl ConfigFileKind {
    /// 実ファイル名を返す。
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Skill => "SKILL.md",
            Self::Memory => "Memory.md",
            Self::Construct => "Construct.md",
            Self::Mcp => "mcp.json",
            Self::Run => "run.json",
        }
    }

    /// 全種別。**ファイル名の一覧をテストで固定するために使う** —
    /// GUI のタブは TypeScript 側の `CONFIG_FILE_LABELS` が生やしている。
    ///
    /// 種別を足したらここの長さも直すこと。**`[Self; N]` の `N` は、足した
    /// バリアント名で grep しても引っかからない**（Spec 15 で `Run` を足した
    /// とき、ここが 4 のまま取り残された）。
    pub fn all() -> [Self; 5] {
        [
            Self::Skill,
            Self::Memory,
            Self::Construct,
            Self::Mcp,
            Self::Run,
        ]
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
        assert!(!template.grounding_active(), "互換経路ではグラウンディングしない");

        // 明示選択して初めて有効になる。
        template.provider = Some(crate::llm::Provider::Gemini);
        assert!(template.grounding_active());

        // チェックを外せば当然無効。
        template.google_search = false;
        assert!(!template.grounding_active());
    }

    /// 接地しなかった発話に来歴の欄を生やさないこと（Spec 05 Phase 4）。
    ///
    /// 全発話の 9 割以上は接地を持たない。空の欄を毎回吐くと、ログの JSON が
    /// 意味の無いキーで膨らみ、`grounding` が付いている発話を目で探せなくなる。
    #[test]
    fn a_message_without_grounding_serializes_without_the_field() {
        let message = AgentMessage::new(
            Endpoint::User,
            Endpoint::Agent {
                id: AgentId::from("agent_a"),
            },
            "こんにちは",
            0,
        );
        let json = serde_json::to_value(&message).unwrap();

        assert!(json.get("grounding").is_none(), "空の来歴は書き出さない: {json}");
    }

    /// 来歴の欄を持たない旧ログがそのまま読めること。
    #[test]
    fn a_message_recorded_before_grounding_still_loads() {
        let json = serde_json::json!({
            "id": "m1",
            "from": { "kind": "user" },
            "to": { "kind": "agent", "id": "agent_a" },
            "content": "こんにちは",
            "tokens": 0,
            "tsMs": 1_700_000_000_000u64,
            "hop": 0
        });

        let message: AgentMessage = serde_json::from_value(json).unwrap();
        assert!(message.grounding.is_empty());
    }

    /// 状態の表示語彙が UI（types.ts の STATUS_LABELS）と一致すること。
    ///
    /// 顔ぶれ（Spec 06）はこの語彙でプロンプトに載る。画面とプロンプトで
    /// 同じ相手が違う言葉で呼ばれると、利用者とエージェントの会話が噛み合わない。
    /// TS 側を変えたらこのテストも落として直すこと（二言語の契約）。
    #[test]
    fn status_labels_match_the_ui_vocabulary() {
        assert_eq!(AgentStatus::Idle.label(), "停止中");
        assert_eq!(AgentStatus::Starting.label(), "起動中");
        assert_eq!(AgentStatus::Running.label(), "稼働中");
        assert_eq!(AgentStatus::Stopping.label(), "停止処理中");
        assert_eq!(AgentStatus::Failed.label(), "失敗");
    }

    #[test]
    fn config_file_kinds_map_to_expected_names() {
        let names: Vec<_> = ConfigFileKind::all()
            .into_iter()
            .map(ConfigFileKind::file_name)
            .collect();
        assert_eq!(
            names,
            vec![
                "SKILL.md",
                "Memory.md",
                "Construct.md",
                "mcp.json",
                "run.json",
            ]
        );
    }

    // ---- 役職の流し込み（Spec 14 P1） ---------------------------------------

    /// 契約の分類表（`role_contract` 凍結 2）で「入れない」とした 5 欄。
    ///
    /// **名指しで持つ。** rev1 の査読が「線と場所が入らないこと」では対象が
    /// 曖昧でテストに落とせないと指摘した箇所で、ここが表と実装の唯一の接点。
    const NEVER_APPLIED: [&str; 5] = [
        "connected_agents",
        "work_dir",
        "order",
        "batch_start",
        "hears_room_log",
    ];

    fn full_defaults() -> AgentRoleDefaults {
        AgentRoleDefaults {
            construct: "あなたは調査役です。".into(),
            model_template_id: Some("tpl".into()),
            rag_sources: vec!["docs".into()],
            enabled_tools: Some(vec!["grep".into()]),
            max_tool_iterations: Some(24),
        }
    }

    /// 触ってよい 4 欄が入る。
    #[test]
    fn apply_to_fills_the_four_spec_fields() {
        let mut spec = AgentSpec::new("agent_1", "ザリ", "既定");
        let dropped = full_defaults().apply_to(&mut spec, |_| true);

        assert!(dropped.is_empty());
        assert_eq!(spec.model_template_id, "tpl".into());
        assert_eq!(spec.rag_sources, vec!["docs".to_string()]);
        assert_eq!(spec.enabled_tools, Some(vec!["grep".to_string()]));
        assert_eq!(spec.max_tool_iterations, Some(24));
    }

    /// **入れない 5 欄は流し込み後も既定のまま。**
    ///
    /// `connected_agents` が入ると「役職を選んだ瞬間に線が引かれる」で
    /// **線は人が引く**が崩れ、`work_dir` が入ると村を配ったとき存在しない
    /// パスを指す。この 2 つが契約の主眼（`role_contract` 凍結 2）。
    #[test]
    fn apply_to_never_touches_the_five_excluded_fields() {
        let baseline = AgentSpec::new("agent_1", "ザリ", "既定");
        let mut spec = baseline.clone();
        let _ = full_defaults().apply_to(&mut spec, |_| true);

        assert_eq!(
            spec.connected_agents, baseline.connected_agents,
            "{} は雛形から入ってはいけない（線は人が引く）",
            NEVER_APPLIED[0]
        );
        assert_eq!(spec.work_dir, baseline.work_dir, "{}", NEVER_APPLIED[1]);
        assert_eq!(spec.order, baseline.order, "{}", NEVER_APPLIED[2]);
        assert_eq!(spec.batch_start, baseline.batch_start, "{}", NEVER_APPLIED[3]);
        assert_eq!(
            spec.hears_room_log, baseline.hears_room_log,
            "{}",
            NEVER_APPLIED[4]
        );
    }

    /// 分類表の数が実装と合う。**11 = 対象外 2 + 入れる 4 + 入れない 5。**
    ///
    /// rev1 は箇条書き 4 本を「4 欄」と数えて実体の 6 フィールドとズレた。
    /// 数え落としは要約から生まれるので、数そのものをテストで留める。
    #[test]
    fn the_classification_table_adds_up() {
        const AGENT_SPEC_FIELDS: usize = 11;
        const EXEMPT: usize = 2; // id / name
        const APPLIED: usize = 4; // model_template_id / rag_sources / enabled_tools / max_tool_iterations
        assert_eq!(AGENT_SPEC_FIELDS, EXEMPT + APPLIED + NEVER_APPLIED.len());
    }

    /// 未登録のモデルテンプレートは**その欄だけ落ちる**。他は入る。
    #[test]
    fn missing_template_drops_only_that_field() {
        let mut spec = AgentSpec::new("agent_1", "ザリ", "既定");
        let dropped = full_defaults().apply_to(&mut spec, |_| false);

        assert_eq!(dropped.len(), 1);
        assert!(dropped[0].contains("tpl"), "落とした欄の名前が分かること");
        // 落ちたのはテンプレートだけ。作成そのものは通り、他の欄は入っている。
        assert_eq!(spec.model_template_id, "既定".into());
        assert_eq!(spec.rag_sources, vec!["docs".to_string()]);
        assert_eq!(spec.enabled_tools, Some(vec!["grep".to_string()]));
    }

    /// **`rag_sources` には存在検査を掛けない。**
    ///
    /// `RagIndex` は断片を索引した瞬間にキーが生える実行時の器で、宣言された
    /// 登録簿が無い。作成時点の索引はほぼ必ず空なので、検査すると
    /// 「調査役を作る → あとで資料を食わせる」という正しい順序を壊す。
    #[test]
    fn rag_sources_are_copied_without_any_existence_check() {
        let mut spec = AgentSpec::new("agent_1", "ザリ", "既定");
        let defaults = AgentRoleDefaults {
            rag_sources: vec!["まだ索引していない資料".into()],
            ..AgentRoleDefaults::default()
        };
        let dropped = defaults.apply_to(&mut spec, |_| false);

        assert!(dropped.is_empty(), "索引が空でも落とさない");
        assert_eq!(spec.rag_sources, vec!["まだ索引していない資料".to_string()]);
    }

    /// 空の既定値は何も上書きしない（役職が意見を持たない欄はそのまま）。
    #[test]
    fn empty_defaults_leave_the_spec_untouched() {
        let baseline = AgentSpec::new("agent_1", "ザリ", "既定");
        let mut spec = baseline.clone();
        let dropped = AgentRoleDefaults::default().apply_to(&mut spec, |_| true);

        assert!(dropped.is_empty());
        assert_eq!(spec, baseline);
    }

    /// 流し込みは `role_id` を触らない（付けるのは呼び出し側の責務）。
    #[test]
    fn apply_to_does_not_set_role_id() {
        let mut spec = AgentSpec::new("agent_1", "ザリ", "既定");
        let _ = full_defaults().apply_to(&mut spec, |_| true);
        assert_eq!(spec.role_id, None);
    }
}
