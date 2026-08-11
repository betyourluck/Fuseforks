//! ワークスペース上の設定ファイル入出力。
//!
//! レイアウト:
//!
//! ```text
//! {workspace}/
//!   world.json              エージェント定義とモデルテンプレート
//!   schedules.json          時刻で発火する依頼（Spec 07。エージェント定義ではないので別ファイル）
//!   Ordinance.md            村の条例（全エージェント共通の規則。プロンプト最上段に入る）
//!   mcp.json                共通 MCP サーバー宣言（全エージェントに提示）
//!   agents/{agent_id}/
//!     SKILL.md              能力・振る舞いの定義
//!     Memory.md             長期記憶
//!     Construct.md          構成・制約の宣言
//!     mcp.json              エージェント別 MCP（このエージェントにだけ提示。Spec 02）
//!     icon.webp             アイコン（設定時のみ。UI が WebP へ変換して送る）
//! ```
//!
//! 書き込み先は **エージェント ID と [`ConfigFileKind`] の組み合わせでしか指定できない**。
//! GUI から任意のパス文字列を受け取らないことで、IPC 経由の任意ファイル書き込みを構造で封じる。
//! ID 側も [`AgentId::is_safe`] で英数字・`-`・`_` に限っており、`..` は入口で弾かれる。

use std::path::{Path, PathBuf};

use crate::command::CommandPolicy;
use crate::error::{CoreError, CoreResult};
use crate::model::{AgentId, AgentSpec, ConfigFileKind};
use crate::schedule::ScheduledTask;
use crate::world::{Language, PersistedWorld};

/// 登録簿の永続化ファイル名。
const WORLD_FILE: &str = "world.json";

/// 予定の永続化ファイル名（Spec 07）。
///
/// `world.json` に入れないのは、予定が**エージェントの定義ではない**から。
/// `Ordinance.md` / `mcp.json` が別ファイルなのと同じ理由。
const SCHEDULES_FILE: &str = "schedules.json";

/// 村の識別子のファイル名（Spec 28）。
///
/// **村の内容物**なので配布に載る（`world.json` と同じ棚）。
/// **人に見せる名前ではない** — 表示名や村の説明に使わないこと。
/// 使い始めると「読みやすい id」への要望が生まれ、乱数である性質が壊れる。
const VILLAGE_ID_FILE: &str = "village_id";

/// 村の識別子として受け付けられる書式か（UUID の 16 進小文字 + ハイフン 36 字）。
///
/// **厳密な UUID の構文検査まではしない。** ここが守っているのは
/// 「承認鍵に混ぜても壊れない字」であって、版番号やバリアントの正しさではない。
fn is_village_id(text: &str) -> bool {
    text.len() == 36
        && text
            .chars()
            .all(|c| c == '-' || (c.is_ascii_hexdigit() && !c.is_ascii_uppercase()))
}

/// MCP サーバー宣言のファイル名。
///
/// Claude Desktop の `claude_desktop_config.json` と同じ `mcpServers` 形式を採る
/// ので、利用者が既に持っている設定をそのまま貼れる。
const MCP_FILE: &str = "mcp.json";

/// アイコンのファイル名。**中身は WebP に固定する。**
///
/// 変換（png / jpg → WebP・リサイズ）は UI 層の責務で、コアは受け入れ検証だけを持つ。
/// 形式を 1 つに固定すると、表示側は MIME 判定なしで `image/webp` として扱える。
///
/// **アイコンは 2 種類ある**（`icon_contract`）— エージェントの
/// `agents/{id}/icon.webp` と利用者の `user/icon.webp`（Spec 19）。
/// **ファイル名も検証も共有し、違うのは親フォルダだけ。**
const ICON_FILE: &str = "icon.webp";

/// 利用者の設定を置くフォルダ名（Spec 19）。
///
/// `agents/` と並ぶ位置に置く。**`AgentId` に「利用者」を表す特別な値を作らない** —
/// 作ると、その値が `world.agents` の検索や `Endpoint::Agent` の照合へ漏れる。
const USER_DIR: &str = "user";

/// 外部クライアントの設定を置くフォルダ名（Spec 25）。
///
/// `user/` と並ぶ位置に置く。**利用者のフォルダを共用しない** — 共用すると
/// 「外の道具が頼んだこと」と「自分の依頼」が同じ顔で並ぶ。
const EXTERNAL_DIR: &str = "external";

/// アイコンの許容上限（bytes）。
///
/// UI 側は 256px 角へ縮小してから送るため、通常は数十 KB に収まる。
/// 上限はその 1 桁上に置き、変換を通さず巨大ファイルを流し込む経路を塞ぐ。
const ICON_MAX_BYTES: usize = 512 * 1024;

/// 村の条例（ワークスペース全体の規則）のファイル名。
///
/// エージェント個別ではなく**場**に属するので、`agents/` の外に置く。
/// 規則の序列は「ベンダーの憲法（モデル側） > 村の条例 > 各エージェントの
/// 個別設定（Construct / SKILL / Memory）」。序列はそのままプロンプトの
/// 物理的な順序（条例が最上段）として表現される。
const ORDINANCE_FILE: &str = "Ordinance.md";

/// 設定ファイルの読み書きを担う。
#[derive(Debug, Clone)]
pub struct ConfigStore {
    root: PathBuf,
}

impl ConfigStore {
    /// ワークスペースのルートを指定して作る。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// ワークスペースのルートパス。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// この村の識別子（`{workspace}/village_id`）。無ければ作る。
    ///
    /// **村の内容物なので配布に載る。** 秘密ではないが、**攻撃者が被害者の値を
    /// 知り得ない**ことに意味がある — 前判定の承認鍵に混ぜてあるので、
    /// 差し替えた村は別の識別子を運び、承認が構造的に外れる（Spec 28 D10）。
    ///
    /// **絶対パスでは代用できない。** この村の workspace は
    /// `{app_data_dir}/workspace` に固定されており、村の入れ替えは「別のパスを
    /// 開く」ではなく**同じパスの中身の差し替え**として起きる。パスを鍵に混ぜても
    /// 同一機では常に同じ値になり、差し替えを 1 件も止められない。
    ///
    /// 書式に合わない中身は**作り直す** — 検査に落ちた識別子を返すと、
    /// その村では承認鍵が永遠に組めない。
    ///
    /// # Errors
    /// 読み書きに失敗した場合 [`CoreError::ConfigIo`]。
    pub async fn village_id(&self) -> CoreResult<String> {
        let path = self.root.join(VILLAGE_ID_FILE);
        if let Ok(text) = tokio::fs::read_to_string(&path).await {
            let trimmed = text.trim();
            if is_village_id(trimmed) {
                return Ok(trimmed.to_owned());
            }
        }
        let fresh = uuid::Uuid::new_v4().to_string();
        self.write_atomic(VILLAGE_ID_FILE, &fresh).await?;
        Ok(fresh)
    }

    /// エージェントの設定ディレクトリを解決する。
    ///
    /// # Errors
    /// ID が命名規約に反する場合 [`CoreError::UnsafeIdentifier`]。
    fn agent_dir(&self, id: &AgentId) -> CoreResult<PathBuf> {
        if !id.is_safe() {
            return Err(CoreError::UnsafeIdentifier {
                value: id.to_string(),
            });
        }
        Ok(self.root.join("agents").join(id.as_str()))
    }

    /// I/O エラーへパス情報を添える。
    fn io_err(path: &Path, source: std::io::Error) -> CoreError {
        CoreError::ConfigIo {
            path: path.display().to_string(),
            source,
        }
    }

    /// 設定ファイルを読む。存在しない場合は空文字を返す。
    ///
    /// 未作成を [`Err`] にしないのは、新規エージェントを開いた直後に
    /// エディタがエラー表示になるのを避けるため。空のファイルと未作成は UI 上同義。
    pub async fn read_config(&self, id: &AgentId, kind: ConfigFileKind) -> CoreResult<String> {
        let path = self.agent_dir(id)?.join(kind.file_name());
        match tokio::fs::read_to_string(&path).await {
            Ok(text) => Ok(text),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(err) => Err(Self::io_err(&path, err)),
        }
    }

    /// 設定ファイルを書く。親ディレクトリは必要に応じて作る。
    ///
    /// # Errors
    /// `kind` が [`ConfigFileKind::Mcp`] で内容が JSON として不正な場合、
    /// **書かずに**エラーを返す（mcp_contract の失敗二分類 (1)。UI 保存経路の
    /// 不変条件: UI 経由の保存後、ディスクは常に正しい JSON か不在）。
    /// 空文字は「未設定」として許す。
    pub async fn write_config(
        &self,
        id: &AgentId,
        kind: ConfigFileKind,
        content: &str,
    ) -> CoreResult<()> {
        if !content.trim().is_empty() {
            // JSON の設定ファイルは書き込み時にパース検証する。壊れた JSON を
            // 保存すると、次の起動で **allow が黙って消えて全部 pending になる**か
            // **deny が黙って消える**かのどちらかで、後者は危険側へ倒れる。
            match kind {
                ConfigFileKind::Mcp => {
                    serde_json::from_str::<crate::mcp::McpConfig>(content)
                        .map_err(CoreError::from)?;
                }
                ConfigFileKind::Run => {
                    serde_json::from_str::<crate::command::CommandPolicy>(content)
                        .map_err(CoreError::from)?;
                }
                _ => {}
            }
        }
        let dir = self.agent_dir(id)?;
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| Self::io_err(&dir, e))?;

        let path = dir.join(kind.file_name());
        tokio::fs::write(&path, content)
            .await
            .map_err(|e| Self::io_err(&path, e))
    }

    /// エージェントのアイコンを読む。未設定なら `None`。
    pub async fn read_icon(&self, id: &AgentId) -> CoreResult<Option<Vec<u8>>> {
        Self::read_icon_at(self.agent_dir(id)?.join(ICON_FILE)).await
    }

    /// エージェントのアイコンを書く。
    ///
    /// # Errors
    /// WebP でない・サイズ上限超過の場合 [`CoreError::InvalidIcon`]。
    /// IPC から来る任意のバイト列をそのまま書くと、ワークスペースが
    /// 「画像のふりをした何か」の置き場になる。マジック番号とサイズで入口を絞る。
    pub async fn write_icon(&self, id: &AgentId, bytes: &[u8]) -> CoreResult<()> {
        let dir = self.agent_dir(id)?;
        Self::write_icon_at(dir, bytes).await
    }

    /// アイコンの受け入れ検証（`icon_contract`）。
    ///
    /// **2 種類のアイコンが同じ述語を通る。** 種類ごとに書き込み関数を分けても、
    /// 検証だけは 1 実装にする — 2 箇所に書くと、片方の上限を変えたときに
    /// もう片方が古い値のまま通し続ける。
    fn validate_icon(bytes: &[u8]) -> CoreResult<()> {
        if bytes.len() > ICON_MAX_BYTES {
            return Err(CoreError::InvalidIcon {
                reason: format!(
                    "サイズが上限を超えています（{} bytes > {} bytes）",
                    bytes.len(),
                    ICON_MAX_BYTES
                ),
            });
        }
        // WebP のマジックバイト判定は添付（Spec 23）と共有する。
        // 共有するのは**この述語だけ** — 上限（ICON_MAX_BYTES）は共有しない。
        // 上限まで一般化すると「2 種類のアイコンが 1 つの上限を共有する」という
        // この関数の不変条件が消える。
        if !crate::attachment::is_webp(bytes) {
            return Err(CoreError::InvalidIcon {
                reason: "WebP 形式ではありません（UI 側で変換してから送る契約）".to_owned(),
            });
        }
        Ok(())
    }

    /// 検証してから、指定フォルダへアイコンを書く。
    ///
    /// 親フォルダの解決だけが呼び出し側の責務で、**検証と書き込みはここに 1 つ**。
    async fn write_icon_at(dir: PathBuf, bytes: &[u8]) -> CoreResult<()> {
        Self::validate_icon(bytes)?;
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| Self::io_err(&dir, e))?;
        let path = dir.join(ICON_FILE);
        tokio::fs::write(&path, bytes)
            .await
            .map_err(|e| Self::io_err(&path, e))
    }

    /// アイコンを読む共通実装。未設定なら `None`。
    async fn read_icon_at(path: PathBuf) -> CoreResult<Option<Vec<u8>>> {
        match tokio::fs::read(&path).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(Self::io_err(&path, err)),
        }
    }

    /// アイコンを削除する共通実装。未設定でも成功（削除は冪等）。
    async fn delete_icon_at(path: PathBuf) -> CoreResult<()> {
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(Self::io_err(&path, err)),
        }
    }

    /// 利用者のアイコンのパス（`{workspace}/user/icon.webp`）。
    fn user_icon_path(&self) -> PathBuf {
        self.root.join(USER_DIR).join(ICON_FILE)
    }

    /// 利用者のアイコンを読む。未設定なら `None`（Spec 19）。
    pub async fn read_user_icon(&self) -> CoreResult<Option<Vec<u8>>> {
        Self::read_icon_at(self.user_icon_path()).await
    }

    /// 利用者のアイコンを書く（Spec 19）。
    ///
    /// # Errors
    /// WebP でない・サイズ上限超過の場合 [`CoreError::InvalidIcon`]。
    /// **検証はエージェントのアイコンと同じ述語**（`validate_icon`）を通る。
    pub async fn write_user_icon(&self, bytes: &[u8]) -> CoreResult<()> {
        Self::write_icon_at(self.root.join(USER_DIR), bytes).await
    }

    /// 利用者のアイコンを削除する。未設定でも成功として扱う（Spec 19）。
    ///
    /// **利用者は削除されない**ので、エージェントのような「設定ディレクトリごと
    /// 消える」連鎖が無い。消えるのはここを明示的に呼んだときだけ。
    pub async fn delete_user_icon(&self) -> CoreResult<()> {
        Self::delete_icon_at(self.user_icon_path()).await
    }

    /// 外部クライアントのアイコンのパス（`{workspace}/external/icon.webp`）。
    fn external_icon_path(&self) -> PathBuf {
        self.root.join(EXTERNAL_DIR).join(ICON_FILE)
    }

    /// 外部クライアントのアイコンを読む。未設定なら `None`（Spec 25）。
    pub async fn read_external_icon(&self) -> CoreResult<Option<Vec<u8>>> {
        Self::read_icon_at(self.external_icon_path()).await
    }

    /// 外部クライアントのアイコンを書く（Spec 25）。
    ///
    /// # Errors
    /// WebP でない・サイズ上限超過の場合 [`CoreError::InvalidIcon`]。
    /// **検証はエージェント・利用者と同じ述語**（`validate_icon`）を通る。
    /// 上限が 1 つであること自体が不変条件なので、ここで別の値を持たせない。
    pub async fn write_external_icon(&self, bytes: &[u8]) -> CoreResult<()> {
        Self::write_icon_at(self.root.join(EXTERNAL_DIR), bytes).await
    }

    /// 外部クライアントのアイコンを削除する。未設定でも成功として扱う（Spec 25）。
    pub async fn delete_external_icon(&self) -> CoreResult<()> {
        Self::delete_icon_at(self.external_icon_path()).await
    }

    /// エージェントのアイコンを削除する。未設定でも成功として扱う（削除は冪等）。
    pub async fn delete_icon(&self, id: &AgentId) -> CoreResult<()> {
        Self::delete_icon_at(self.agent_dir(id)?.join(ICON_FILE)).await
    }

    /// `run.json` を読む（Spec 20 で共有化）。
    ///
    /// **未作成・空は既定**（`allow` が空 = 全部 pending = fail closed）。
    /// **壊れた JSON は `Err`。既定へ落とさない。**
    ///
    /// # 落とすと何が起きるか
    ///
    /// 読むだけなら既定へ落としても安全側（`allow` も `deny` も消えるので
    /// 全部 pending になる）。**壊れるのは、その既定を書き戻したとき** —
    /// 人が書いた `allow` / `deny` が丸ごと消えたファイルで上書きされる。
    /// 実測で `allow: ["git log *"]` / `deny: ["rm *"]` が両方消えた。
    /// **読みの寛容さと書きの寛容さは別物**なので、`Err` を返せる形にして
    /// 判断を呼び出し側へ渡す。
    pub async fn read_command_policy(&self, id: &AgentId) -> CoreResult<CommandPolicy> {
        let text = self.read_config(id, ConfigFileKind::Run).await?;
        if text.trim().is_empty() {
            return Ok(CommandPolicy::default());
        }
        serde_json::from_str::<CommandPolicy>(&text).map_err(CoreError::from)
    }

    /// `run.json` を**読み直してから**変更を適用して書き戻す（Spec 20 で共有化）。
    ///
    /// **全文上書きにしない**ための唯一の経路。人が `allow` / `deny` /
    /// `timeoutSecs` を編集していても、その編集の上へ差分だけを乗せる。
    /// 書き込みに失敗したら最大 3 回まで読み直しからやり直す。
    ///
    /// **壊れた JSON では何もせず `Err`。** 上の doc のとおり、ここで既定へ
    /// 落として書くと人の編集が消える。
    pub async fn update_command_policy<T>(
        &self,
        id: &AgentId,
        mut apply: impl FnMut(&mut CommandPolicy) -> T,
    ) -> CoreResult<T> {
        let mut last = None;
        for _ in 0..3 {
            let mut policy = self.read_command_policy(id).await?;
            let outcome = apply(&mut policy);
            let text = serde_json::to_string_pretty(&policy).map_err(CoreError::from)?;
            match self.write_config(id, ConfigFileKind::Run, &text).await {
                Ok(()) => return Ok(outcome),
                Err(err) => last = Some(err),
            }
        }
        Err(last.expect("3 回とも失敗したなら理由が 1 つは残っている"))
    }

    /// 村の条例を読む。未設定なら空文字。
    pub async fn read_ordinance(&self) -> CoreResult<String> {
        let path = self.root.join(ORDINANCE_FILE);
        match tokio::fs::read_to_string(&path).await {
            Ok(text) => Ok(text),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(err) => Err(Self::io_err(&path, err)),
        }
    }

    /// 村の条例を書く。
    pub async fn write_ordinance(&self, content: &str) -> CoreResult<()> {
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(|e| Self::io_err(&self.root, e))?;
        let path = self.root.join(ORDINANCE_FILE);
        tokio::fs::write(&path, content)
            .await
            .map_err(|e| Self::io_err(&path, e))
    }

    /// エージェント別の MCP サーバー宣言を読む（`agents/{id}/mcp.json`）。
    ///
    /// 未作成・空なら空の集合。**壊れた JSON はエラー**（外部編集起因の
    /// 失敗二分類 (1')。呼び出し側は起動を止めず、読み込み失敗として保持する）。
    pub async fn read_agent_mcp_config(&self, id: &AgentId) -> CoreResult<crate::mcp::McpConfig> {
        let path = self.agent_dir(id)?.join(ConfigFileKind::Mcp.file_name());
        match tokio::fs::read_to_string(&path).await {
            Ok(text) if text.trim().is_empty() => Ok(crate::mcp::McpConfig::default()),
            Ok(text) => serde_json::from_str(&text).map_err(CoreError::from),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                Ok(crate::mcp::McpConfig::default())
            }
            Err(err) => Err(Self::io_err(&path, err)),
        }
    }

    /// MCP サーバー宣言を読む。未作成なら空の集合。
    ///
    /// **壊れた JSON は空にせずエラーにする。** 空として扱うと、書き間違えた瞬間に
    /// 全ツールが黙って消え、利用者は「MCP が動かない」としか分からなくなる。
    pub async fn read_mcp_config(&self) -> CoreResult<crate::mcp::McpConfig> {
        let path = self.root.join(MCP_FILE);
        match tokio::fs::read_to_string(&path).await {
            Ok(text) if text.trim().is_empty() => Ok(crate::mcp::McpConfig::default()),
            Ok(text) => serde_json::from_str(&text).map_err(CoreError::from),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                Ok(crate::mcp::McpConfig::default())
            }
            Err(err) => Err(Self::io_err(&path, err)),
        }
    }

    /// MCP サーバー宣言を書く。
    pub async fn write_mcp_config(&self, config: &crate::mcp::McpConfig) -> CoreResult<()> {
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(|e| Self::io_err(&self.root, e))?;
        let json = serde_json::to_string_pretty(config)?;
        let path = self.root.join(MCP_FILE);
        tokio::fs::write(&path, json)
            .await
            .map_err(|e| Self::io_err(&path, e))
    }

    /// エージェントの設定ディレクトリごと削除する。存在しなければ何もしない。
    pub async fn remove_agent_dir(&self, id: &AgentId) -> CoreResult<()> {
        let dir = self.agent_dir(id)?;
        match tokio::fs::remove_dir_all(&dir).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(Self::io_err(&dir, err)),
        }
    }

    /// 設定ファイルを連結して、そのエージェントのシステムプロンプトを組み立てる。
    ///
    /// 連結順は `村の条例` → `Construct` → `Skill` → `Memory` で固定する。
    /// **順序を固定するのはプロンプトキャッシュのため** — 先頭が毎回揺れると
    /// キャッシュのプレフィックスが一致せず、読み取り割引が一切効かなくなる。
    ///
    /// 条例を最上段に置くのは、規則の序列（ベンダーの憲法 > 条例 > 個別設定）を
    /// プロンプトの物理順として表現するため。条例は全エージェント共通なので、
    /// モデル間の憲法差（振る舞いの既定値の違い）を吸収する正規化層にもなる。
    /// 編集すると全エージェントのキャッシュが一度無効になるが、それは
    /// 「場の規則が変わった」ことの正しい代償である。
    ///
    /// 戻り値の第 2 要素は「安定部分の文字数」で、
    /// [`crate::llm::ChatRequest::cacheable_prefix_len`] にそのまま渡せる。
    /// `Memory.md` は対話で書き換わる想定なので、境界はその手前に置く。
    pub async fn compose_system_prompt(
        &self,
        spec: &AgentSpec,
        grounded: bool,
        roster: Option<&str>,
        language: Language,
    ) -> CoreResult<(String, usize)> {
        let ordinance = self.read_ordinance().await?;
        let construct = self.read_config(&spec.id, ConfigFileKind::Construct).await?;
        let skill = self.read_config(&spec.id, ConfigFileKind::Skill).await?;
        let memory = self.read_config(&spec.id, ConfigFileKind::Memory).await?;

        let mut prompt = String::new();
        // **加算で二言語**（Spec 35 D1）。日本語の腕はバイト等価で据え置き —
        // golden（tests/prompt_golden.rs）が (prompt, stable_len) の両方を
        // リテラルで固定している。英語は翻訳ではなく英語で書く（D7）。
        if !ordinance.is_empty() {
            prompt.push_str(match language {
                Language::Ja => {
                    "# 村の条例（この場の全員に適用される規則。個別設定より優先される）\n"
                }
                Language::En => {
                    "# Village Ordinance (rules for everyone here; they override individual settings)\n"
                }
            });
            prompt.push_str(&ordinance);
            prompt.push_str("\n\n");
        }
        // 自分が誰で、どこまでが自分の役かを明示する。
        //
        // 以前はここが `# エージェント: {name}` の見出し 1 行だけだった。見出しは
        // 指示として弱く、実機では 1 体が 3 人分のセリフを自分で書く「一人三役」が
        // 起きた。この場には複数のエージェントが居て、それぞれが自分で喋る —
        // という前提はモデルにとって自明ではない。**役の境界は、書かれていなければ
        // 存在しない。**
        prompt.push_str(&match language {
            Language::Ja => format!(
                "# あなたについて\n\
                 あなたはこの場に参加している **{}** です。\n\
                 - **{} 自身の発言だけを書いてください。** 他のエージェントの発言を\
                 代筆・代弁してはいけません。彼らはそれぞれ自分で発言します。\n\
                 - 発言に自分の名前を書く必要はありません。誰の発言かは自動的に伝わります。\n\
                 - 相手の発言を装って会話を先に進めないでください。\n\n",
                spec.name, spec.name
            ),
            Language::En => format!(
                "# About you\n\
                 You are **{}**, one of the participants in this space.\n\
                 - **Write only {}'s own words.** Never write or paraphrase another \
                 agent's lines — each of them speaks for themselves.\n\
                 - No need to sign your messages; who is speaking is conveyed \
                 automatically.\n\
                 - Never move the conversation forward by staging someone else's \
                 reply.\n\n",
                spec.name, spec.name
            ),
        });

        // 作業フォルダの実パスを開示する。ツールは相対パスしか返さないため、
        // これが無いとモデルは説明文に書く絶対パスを**推測で創作**する
        // （実機で、実在しない `D:\work\Concordia` を作業場所として語った。
        // 判断材料の欠落は、禁止ではなく情報で埋める — 知っていれば創作する
        // 理由が消える）。
        if let Some(work_dir) = &spec.work_dir {
            prompt.push_str(&match language {
                Language::Ja => format!(
                    "## 作業フォルダ\n\
                     あなたのファイル系ツール（grep / fd / diff / sd / yq）が\
                     読み書きできるのは `{work_dir}` の中だけです。ツールが返す\
                     相対パスは、このフォルダ直下からのパスです。ファイルの場所を\
                     説明するときは、このパスを基準にしてください\
                     （それ以外の場所を推測で語らないこと）。\n\n"
                ),
                Language::En => format!(
                    "## Working folder\n\
                     Your file tools (grep / fd / diff / sd / yq) can read and \
                     write only inside `{work_dir}`. Relative paths returned by \
                     the tools are rooted at this folder. When you describe where \
                     a file is, use this path as the base (do not guess about \
                     locations outside it).\n\n"
                ),
            });
        }
        // 接地の作法。**「出典を出すな」ではなく「何が手元に無いか」を伝える。**
        //
        // Google 検索による接地は、答えの中身は運んでくるが**参照元 URL を
        // こちらへ渡さない**（渡ってくるのは検索語だけ）。この事実を伝えないまま
        // 出典を求められると、モデルは引用の形をした文字列を作る — 実機で、
        // ドメインのルート URL に記事の見出しを添えた偽の引用が返り、さらに
        // 2 回目には**実在するものと 404 が混在する**、より紛らわしい形になった
        // （2026-07-29。半分が本物であることが、残り半分に信憑性を貸す）。
        //
        // これは作業フォルダの実パス開示と同じ形の処方で、上の節と同じ理由で
        // ここに置いている。人格ではなくワイヤ経路の性質なので、SKILL.md では
        // なく実装側から入れる（接地を有効にした全員に等しく効く）。
        if grounded {
            prompt.push_str(match language {
                Language::Ja => {
                    "## グラウンディング（Google 検索）について\n\
                     あなたは Google 検索で裏を取ってから答えられます。ただし\
                     **参照したページの URL は、あなたの手元には渡ってきません。**\n\
                     - **URL を書かないでください。** 出典を求められたら\
                     「URL は取得できない」と正直に答えてください。\n\
                     - 代わりに、**実際に検索した語**と、**発表元の名前**\
                     （気象庁・内閣府・◯◯新聞 など）は答えられます。それを示してください。\n\
                     - もっともらしい URL を組み立ててはいけません。\
                     実在しない URL は、出典が無いことより有害です\
                     （受け取った相手が確認済みだと誤解します）。\n\n"
                }
                Language::En => {
                    "## Grounding (Google Search)\n\
                     You can check your answer against Google Search before \
                     replying, but **the URLs of the pages you consulted are \
                     never handed back to you.**\n\
                     - **Do not write URLs.** If asked for sources, say honestly \
                     that URLs are not available to you.\n\
                     - What you can report instead are **the exact search terms \
                     you used** and **the names of the publishers** (a weather \
                     bureau, a ministry, a newspaper, and so on). Offer those.\n\
                     - Never assemble a plausible-looking URL. A URL that does \
                     not exist is worse than having no source at all (the reader \
                     will assume it has been verified).\n\n"
                }
            });
        }

        if !construct.is_empty() {
            prompt.push_str("## Construct\n");
            prompt.push_str(&construct);
            prompt.push_str("\n\n");
        }
        if !skill.is_empty() {
            prompt.push_str("## Skill\n");
            prompt.push_str(&skill);
            prompt.push_str("\n\n");
        }

        // キャッシュの安定境界は**安定素材（条例〜Skill）の末尾**。
        //
        // 旧定義は「Memory の直前」だったが、それが「Skill の末尾」と同じ点を
        // 指せていたのは間に何も無かった間だけ（Spec 06 rev4 指摘 1）。
        // 顔ぶれを挟む今、境界は**先に**確定させる — 後で数えると顔ぶれが
        // 安定部分に入り、状態が変わるたびに全エージェントのキャッシュが割れる。
        let stable_len = prompt.chars().count();

        // 今の顔ぶれ（Spec 06 P1.5）。可変部分に置くので stable_len は据え置き。
        // 順序・形式は呼び出し側（orchestrator）が組む — 状態は World の持ち物で、
        // ConfigStore はファイルしか知らない。
        if let Some(roster) = roster {
            prompt.push_str(match language {
                Language::Ja => "## 今の顔ぶれ\n",
                Language::En => "## Current roster\n",
            });
            prompt.push_str(roster);
            prompt.push_str("\n\n");
        }

        if !memory.is_empty() {
            prompt.push_str("## Memory\n");
            prompt.push_str(&memory);
            prompt.push('\n');
        }

        Ok((prompt, stable_len))
    }

    /// `world.json` が実在するか。
    ///
    /// [`Self::load_world`] は不在を空の世界として返すため、呼んだ後からは
    /// 「新規の村」と「空の村」を区別できない。新規にだけ既定値を書く判断
    /// （Spec 11 の tokenBudget）は、load の前にこれで見る。
    pub fn world_exists(&self) -> bool {
        self.root.join(WORLD_FILE).exists()
    }

    /// 登録簿を読み込む。ファイルが無ければ空の状態を返す。
    pub async fn load_world(&self) -> CoreResult<PersistedWorld> {
        let path = self.root.join(WORLD_FILE);
        match tokio::fs::read_to_string(&path).await {
            Ok(text) => serde_json::from_str(&text).map_err(CoreError::from),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                Ok(PersistedWorld::default())
            }
            Err(err) => Err(Self::io_err(&path, err)),
        }
    }

    /// 登録簿を保存する。
    ///
    /// 一時ファイルへ書いてから rename する。書き込み途中で落ちても
    /// 既存の `world.json` が壊れた JSON に置き換わらない（全設定を失わない）。
    pub async fn save_world(&self, world: &PersistedWorld) -> CoreResult<()> {
        self.write_atomic(WORLD_FILE, &serde_json::to_string_pretty(world)?)
            .await
    }

    /// 予定を読み込む（Spec 07）。
    ///
    /// **1 件ずつ検証し、壊れた 1 件だけを落として残りを開く。** `mcp.json` とは
    /// 逆の判断で、あちらは「壊れた JSON を空として扱うと全ツールが黙って消える」
    /// ため全体をエラーにしている。予定は 1 件ずつ独立していて、他の予定を
    /// 人質にする理由が無い。
    ///
    /// # Errors
    /// - ファイル自体が JSON 配列として読めない場合。**この時は呼び出し側が
    ///   書き戻しを止める**（読めなかったものを上書きすると、直せば戻ったはずの
    ///   予定を消す）
    pub async fn load_schedules(&self) -> CoreResult<LoadedSchedules> {
        let path = self.root.join(SCHEDULES_FILE);
        let text = match tokio::fs::read_to_string(&path).await {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(LoadedSchedules::default());
            }
            Err(err) => return Err(Self::io_err(&path, err)),
        };

        let rows: Vec<serde_json::Value> = serde_json::from_str(&text)?;

        let mut loaded = LoadedSchedules::default();
        for row in rows {
            match serde_json::from_value::<ScheduledTask>(row.clone()) {
                Ok(task) => match task.validate() {
                    Ok(()) => loaded.tasks.push(task),
                    Err(err) => loaded.dropped.push(format!("{} を落としました: {err}", task.id)),
                },
                Err(err) => loaded
                    .dropped
                    .push(format!("読めない予定を 1 件落としました: {err}（{row}）")),
            }
        }
        Ok(loaded)
    }

    /// 予定を保存する。`save_world` と同じく一時ファイル + rename。
    ///
    /// 電源断で壊れると**全予定が消える**ので、原子性は世界と同格に扱う。
    pub async fn save_schedules(&self, tasks: &[ScheduledTask]) -> CoreResult<()> {
        self.write_atomic(SCHEDULES_FILE, &serde_json::to_string_pretty(tasks)?)
            .await
    }

    /// 一時ファイルへ書いてから rename する共通経路。
    async fn write_atomic(&self, file_name: &str, json: &str) -> CoreResult<()> {
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(|e| Self::io_err(&self.root, e))?;

        let final_path = self.root.join(file_name);
        let temp_path = self.root.join(format!("{file_name}.tmp"));

        tokio::fs::write(&temp_path, json)
            .await
            .map_err(|e| Self::io_err(&temp_path, e))?;
        tokio::fs::rename(&temp_path, &final_path)
            .await
            .map_err(|e| Self::io_err(&final_path, e))
    }
}

/// [`ConfigStore::load_schedules`] の結果。
#[derive(Debug, Default)]
pub struct LoadedSchedules {
    /// 読めて検証も通った予定。
    pub tasks: Vec<ScheduledTask>,
    /// 落とした 1 件ごとの理由。利用者へ出すのではなく、起動ログへ残すため。
    pub dropped: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト用の一時ディレクトリ。`tempfile` を足さずに済ませるための最小実装。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "fuseforks-test-{tag}-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// 最小の WebP コンテナ（マジック番号だけを満たすバイト列）。
    fn webp(payload: &[u8]) -> Vec<u8> {
        let mut bytes = b"RIFF\0\0\0\0WEBP".to_vec();
        bytes.extend_from_slice(payload);
        bytes
    }

    /// 利用者のアイコンは `{workspace}/user/icon.webp` を往復する（Spec 19）。
    ///
    /// フォルダが無くても書ける・消したら `None`・削除は冪等。
    #[tokio::test]
    async fn a_user_icon_roundtrips_through_its_own_folder() {
        let dir = TempDir::new("user-icon");
        let store = ConfigStore::new(&dir.0);

        assert_eq!(store.read_user_icon().await.unwrap(), None, "未設定は None");
        // 未設定でも削除は成功する（冪等）。
        store.delete_user_icon().await.unwrap();

        let bytes = webp(b"chikara");
        // `user/` は存在しないが、書き込みが作る。
        store.write_user_icon(&bytes).await.unwrap();
        assert_eq!(store.read_user_icon().await.unwrap(), Some(bytes));
        assert!(
            dir.0.join("user").join("icon.webp").exists(),
            "置き場は {{workspace}}/user/icon.webp"
        );

        store.delete_user_icon().await.unwrap();
        assert_eq!(store.read_user_icon().await.unwrap(), None);
    }

    /// **検証は 2 種類のアイコンで 1 実装を共有する**（`icon_contract`）。
    ///
    /// 切り出した述語を戻して片方だけに書き直すと、このテストが落ちる。
    /// 2 箇所に書くと、片方の上限を変えたときにもう片方が古い値のまま通し続ける。
    #[tokio::test]
    async fn both_icon_kinds_share_one_validator() {
        let dir = TempDir::new("icon-validator");
        let store = ConfigStore::new(&dir.0);
        let id = AgentId::from("agent_01");

        // WebP でないバイト列。
        let not_webp = b"\x89PNG\r\n\x1a\n and some more bytes".to_vec();
        for err in [
            store.write_icon(&id, &not_webp).await.unwrap_err(),
            store.write_user_icon(&not_webp).await.unwrap_err(),
        ] {
            assert_eq!(err.code(), "INVALID_ICON", "WebP でないものは両方とも拒否");
        }

        // 上限超過。
        let oversized = webp(&vec![0u8; ICON_MAX_BYTES]);
        for err in [
            store.write_icon(&id, &oversized).await.unwrap_err(),
            store.write_user_icon(&oversized).await.unwrap_err(),
        ] {
            assert_eq!(err.code(), "INVALID_ICON", "上限超過は両方とも拒否");
        }

        // 拒否した側にファイルは残らない。
        assert_eq!(store.read_user_icon().await.unwrap(), None);
        assert_eq!(store.read_icon(&id).await.unwrap(), None);
    }

    /// **人の手編集を機械が潰さない**（`update_command_policy` は毎回読み直す）。
    ///
    /// この保証は `merge_pending_into` のテストが担っていた。Spec 20 で
    /// あの関数を撤去したので、**保証をここへ移し替える** — 移さないと
    /// 「誰も覆っていない不変条件」が残る（failures.md #62 と同型）。
    #[tokio::test]
    async fn a_policy_update_never_clobbers_the_human_edits() {
        let dir = TempDir::new("policy-update");
        let store = ConfigStore::new(&dir.0);
        let id = AgentId::from("agent_01");

        // 人が allow / deny / timeoutSecs を書いた状態。
        store
            .write_config(
                &id,
                ConfigFileKind::Run,
                r#"{"allow":["git log *"],"deny":["rm *"],"timeoutSecs":300}"#,
            )
            .await
            .unwrap();

        // 機械が pending を 1 件足す。
        store
            .update_command_policy(&id, |policy| {
                let _ = policy.note_pending("cargo", &["test".to_owned()], 42);
            })
            .await
            .unwrap();

        let after = store.read_command_policy(&id).await.unwrap();
        assert_eq!(after.allow, vec!["git log *"], "人の allow が残る");
        assert_eq!(after.deny, vec!["rm *"]);
        assert_eq!(after.timeout_secs, 300, "人の timeoutSecs が残る");
        assert_eq!(after.pending.len(), 1);
    }

    /// 壊れた JSON は `Err`。**既定へ落とさない**（落として書くと人の編集が消える）。
    #[tokio::test]
    async fn a_broken_policy_reads_as_an_error_not_as_the_default() {
        let dir = TempDir::new("policy-broken");
        let store = ConfigStore::new(&dir.0);
        let id = AgentId::from("agent_01");
        let path = dir.0.join("agents").join("agent_01");
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("run.json"), "{ oops").unwrap();

        assert!(store.read_command_policy(&id).await.is_err());
        assert!(
            store.update_command_policy(&id, |_| ()).await.is_err(),
            "壊れていたら書き込みへ進まない"
        );
        assert_eq!(
            std::fs::read_to_string(path.join("run.json")).unwrap(),
            "{ oops",
            "1 バイトも変えない"
        );
    }

    /// 未作成は既定として読める（fail closed = allow が空）。
    #[tokio::test]
    async fn a_missing_policy_reads_as_the_fail_closed_default() {
        let dir = TempDir::new("policy-missing");
        let store = ConfigStore::new(&dir.0);
        let policy = store
            .read_command_policy(&AgentId::from("agent_01"))
            .await
            .unwrap();
        assert!(policy.allow.is_empty());
        assert!(!policy.allows_anything(), "何も実行できない");
    }
    #[tokio::test]
    async fn missing_config_reads_as_empty_string() {
        let dir = TempDir::new("missing");
        let store = ConfigStore::new(&dir.0);

        let text = store
            .read_config(&"agent_01".into(), ConfigFileKind::Skill)
            .await
            .unwrap();
        assert_eq!(text, "");
    }

    #[tokio::test]
    async fn write_then_read_roundtrips() {
        let dir = TempDir::new("roundtrip");
        let store = ConfigStore::new(&dir.0);
        let id = AgentId::from("agent_01");

        store
            .write_config(&id, ConfigFileKind::Skill, "# 能力\n計画を立てる")
            .await
            .unwrap();

        let text = store.read_config(&id, ConfigFileKind::Skill).await.unwrap();
        assert_eq!(text, "# 能力\n計画を立てる");
        assert!(dir.0.join("agents/agent_01/SKILL.md").exists());
    }

    #[tokio::test]
    async fn path_traversal_is_rejected_before_any_io() {
        let dir = TempDir::new("traversal");
        let store = ConfigStore::new(&dir.0);

        let err = store
            .write_config(&"../../evil".into(), ConfigFileKind::Skill, "x")
            .await
            .unwrap_err();

        assert_eq!(err.code(), "UNSAFE_IDENTIFIER");
    }

    #[tokio::test]
    async fn system_prompt_boundary_excludes_the_mutable_memory_section() {
        let dir = TempDir::new("prompt");
        let store = ConfigStore::new(&dir.0);
        let id = AgentId::from("agent_01");

        store
            .write_config(&id, ConfigFileKind::Construct, "制約A")
            .await
            .unwrap();
        store
            .write_config(&id, ConfigFileKind::Skill, "能力B")
            .await
            .unwrap();
        store
            .write_config(&id, ConfigFileKind::Memory, "記憶C")
            .await
            .unwrap();

        let spec = AgentSpec::new(id.clone(), "Planner", "tpl");
        let (prompt, stable_len) = store.compose_system_prompt(&spec, false, None, Language::Ja).await.unwrap();

        let stable: String = prompt.chars().take(stable_len).collect();
        assert!(stable.contains("制約A") && stable.contains("能力B"));
        assert!(!stable.contains("記憶C"), "可変部分は境界の外側");
    }

    /// 顔ぶれが安定境界の外（可変部分）に置かれること（Spec 06 P1.5）。
    ///
    /// 旧定義「Memory の直前」は Skill と Memory が隣接していた間だけ
    /// 「Skill の末尾」と同じ点を指せていた。顔ぶれを挟んだ今、境界が
    /// 顔ぶれの後ろで確定すると、状態が変わるたびに全エージェントの
    /// キャッシュが割れる。
    #[tokio::test]
    async fn the_roster_lives_outside_the_stable_prefix() {
        let dir = TempDir::new("prompt-roster");
        let store = ConfigStore::new(&dir.0);
        let id = AgentId::from("agent_01");
        store
            .write_config(&id, ConfigFileKind::Skill, "能力B")
            .await
            .unwrap();
        let spec = AgentSpec::new(id, "Planner", "tpl");

        let roster = "agent_2（ジェミー）: 稼働中 / agent_3（ロボットくん1号）: 停止中";
        let (with, stable_with) = store
            .compose_system_prompt(&spec, false, Some(roster), Language::Ja)
            .await
            .unwrap();
        let (without, stable_without) =
            store.compose_system_prompt(&spec, false, None, Language::Ja).await.unwrap();

        // 境界の値は顔ぶれの有無で変わらない（変わればキャッシュキーが揺れる）。
        assert_eq!(stable_with, stable_without, "顔ぶれは境界を動かさない");

        let stable: String = with.chars().take(stable_with).collect();
        assert!(!stable.contains("今の顔ぶれ"), "顔ぶれは安定部分に入らない");
        assert!(
            with.contains("## 今の顔ぶれ\nagent_2（ジェミー）: 稼働中"),
            "可変部分には入っていること: {with}"
        );
        assert!(!without.contains("今の顔ぶれ"), "無ければ節ごと出さない");
    }

    /// 接地を有効にしたエージェントには、URL が手元に来ないことを伝えること。
    ///
    /// 伝えないと、出典を求められたモデルは引用の形をした文字列を作る。
    /// 実機では実在する URL と 404 が混ざった形で返り、生きている側が
    /// 死んでいる側に信憑性を貸した（2026-07-29）。
    #[tokio::test]
    async fn a_grounded_agent_is_told_it_cannot_cite_urls() {
        let dir = TempDir::new("prompt-grounded");
        let store = ConfigStore::new(&dir.0);
        let spec = AgentSpec::new(AgentId::from("agent_01"), "ジェミー", "tpl");

        let (grounded, stable_len) = store.compose_system_prompt(&spec, true, None, Language::Ja).await.unwrap();
        assert!(grounded.contains("URL は、あなたの手元には渡ってきません"));
        assert!(grounded.contains("検索した語"), "代わりに何を言えるかも伝える");

        // 会話ごとに揺れない情報なので、キャッシュの安定部分に入っていること。
        let stable: String = grounded.chars().take(stable_len).collect();
        assert!(stable.contains("グラウンディング（Google 検索）について"));

        // 接地していないエージェントには出さない。無関係な制約を負わせない。
        let (plain, _) = store.compose_system_prompt(&spec, false, None, Language::Ja).await.unwrap();
        assert!(!plain.contains("グラウンディング（Google 検索）について"));
    }

    /// エージェント別 mcp.json は保存時にパース検証されること（失敗二分類 (1)）。
    #[tokio::test]
    async fn agent_mcp_config_writes_are_validated() {
        let dir = TempDir::new("agent-mcp");
        let store = ConfigStore::new(&dir.0);
        let id = AgentId::from("agent_01");

        // 壊れた JSON は書かずに拒否。
        let err = store
            .write_config(&id, ConfigFileKind::Mcp, "{ broken")
            .await
            .unwrap_err();
        assert_eq!(err.code(), "SERDE_FAILED");
        assert!(!dir.0.join("agents/agent_01/mcp.json").exists(), "ディスクに書かない");

        // 正しい宣言は書けて、読み戻せる。
        let valid = r#"{ "mcpServers": { "memo": { "command": "memo-server", "args": [] } } }"#;
        store.write_config(&id, ConfigFileKind::Mcp, valid).await.unwrap();
        let config = store.read_agent_mcp_config(&id).await.unwrap();
        assert!(config.servers.contains_key("memo"));

        // 空文字は「未設定」として許す。
        store.write_config(&id, ConfigFileKind::Mcp, "").await.unwrap();
        assert!(store.read_agent_mcp_config(&id).await.unwrap().servers.is_empty());
    }

    /// 外部編集で壊れた mcp.json は読み込みエラーになること（失敗二分類 (1')）。
    #[tokio::test]
    async fn a_hand_broken_agent_mcp_config_reads_as_an_error_not_as_empty() {
        let dir = TempDir::new("agent-mcp-broken");
        let store = ConfigStore::new(&dir.0);
        let id = AgentId::from("agent_01");

        // 未作成は空の集合（エラーではない）。
        assert!(store.read_agent_mcp_config(&id).await.unwrap().servers.is_empty());

        // 保存経路を迂回してディスクを直接壊す（外部編集の再現）。
        std::fs::create_dir_all(dir.0.join("agents/agent_01")).unwrap();
        std::fs::write(dir.0.join("agents/agent_01/mcp.json"), "{ broken").unwrap();

        let err = store.read_agent_mcp_config(&id).await.unwrap_err();
        assert_eq!(err.code(), "SERDE_FAILED", "空扱いにせずエラー");
    }

    #[tokio::test]
    async fn world_save_is_atomic_and_leaves_no_temp_file() {
        let dir = TempDir::new("world");
        let store = ConfigStore::new(&dir.0);

        let mut persisted = PersistedWorld::default();
        persisted
            .model_templates
            .push(crate::model::ModelTemplate::new("tpl", "既定", "gpt-4o"));
        store.save_world(&persisted).await.unwrap();

        let loaded = store.load_world().await.unwrap();
        assert_eq!(loaded.model_templates.len(), 1);
        assert!(!dir.0.join("world.json.tmp").exists());
    }

    #[tokio::test]
    async fn loading_a_missing_world_yields_an_empty_one() {
        let dir = TempDir::new("empty-world");
        let store = ConfigStore::new(&dir.0);

        let loaded = store.load_world().await.unwrap();
        assert!(loaded.agents.is_empty() && loaded.model_templates.is_empty());
    }

    /// 作業フォルダが設定されていれば、その実パスがプロンプトに入ること。
    ///
    /// ツールは相対パスしか返さないため、実パスを渡さないとモデルは
    /// 説明に使う絶対パスを推測で創作する（実在しないパスを語った実例あり）。
    #[tokio::test]
    async fn the_system_prompt_discloses_the_work_dir_when_set() {
        let dir = TempDir::new("workdir-prompt");
        let store = ConfigStore::new(&dir.0);

        let mut spec = AgentSpec::new("agent_1", "コーダー", "tpl");
        let (prompt, _) = store.compose_system_prompt(&spec, false, None, Language::Ja).await.unwrap();
        assert!(!prompt.contains("作業フォルダ"), "未設定なら節ごと出さない");

        spec.work_dir = Some("D:\\Projects\\my-app".into());
        let (prompt, stable_len) = store.compose_system_prompt(&spec, false, None, Language::Ja).await.unwrap();
        assert!(prompt.contains("D:\\Projects\\my-app"), "実パスが入ること: {prompt}");
        let stable: String = prompt.chars().take(stable_len).collect();
        assert!(
            stable.contains("D:\\Projects\\my-app"),
            "作業フォルダは安定プレフィックス側（設定変更まで不変）"
        );
    }

    /// システムプロンプトが「自分は誰か」と「自分の発言だけを書く」を明示すること。
    ///
    /// 見出し（`# エージェント: ジェミー`）だけでは指示として弱く、実機では
    /// 1 体が 3 人分のセリフを自分で書く「一人三役」が起きた。役の境界は
    /// 書かれていなければ存在しない。
    #[tokio::test]
    async fn the_system_prompt_declares_identity_and_forbids_speaking_for_others() {
        let dir = TempDir::new("identity");
        let store = ConfigStore::new(&dir.0);
        let spec = AgentSpec::new("agent_1", "ジェミー", "tpl");

        let (prompt, _) = store.compose_system_prompt(&spec, false, None, Language::Ja).await.unwrap();

        assert!(prompt.contains("ジェミー"), "自分の名前が入ること");
        assert!(
            prompt.contains("代筆") || prompt.contains("代弁"),
            "他人の発言を書かない規則が入ること: {prompt}"
        );
    }
}
