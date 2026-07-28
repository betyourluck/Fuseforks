//! ワークスペース上の設定ファイル入出力。
//!
//! レイアウト:
//!
//! ```text
//! {workspace}/
//!   world.json              エージェント定義とモデルテンプレート
//!   Ordinance.md            村の条例（全エージェント共通の規則。プロンプト最上段に入る）
//!   agents/{agent_id}/
//!     SKILL.md              能力・振る舞いの定義
//!     Memory.md             長期記憶
//!     Construct.md          構成・制約の宣言
//!     icon.webp             アイコン（設定時のみ。UI が WebP へ変換して送る）
//! ```
//!
//! 書き込み先は **エージェント ID と [`ConfigFileKind`] の組み合わせでしか指定できない**。
//! GUI から任意のパス文字列を受け取らないことで、IPC 経由の任意ファイル書き込みを構造で封じる。
//! ID 側も [`AgentId::is_safe`] で英数字・`-`・`_` に限っており、`..` は入口で弾かれる。

use std::path::{Path, PathBuf};

use crate::error::{CoreError, CoreResult};
use crate::model::{AgentId, AgentSpec, ConfigFileKind};
use crate::world::PersistedWorld;

/// 登録簿の永続化ファイル名。
const WORLD_FILE: &str = "world.json";

/// MCP サーバー宣言のファイル名。
///
/// Claude Desktop の `claude_desktop_config.json` と同じ `mcpServers` 形式を採る
/// ので、利用者が既に持っている設定をそのまま貼れる。
const MCP_FILE: &str = "mcp.json";

/// エージェントアイコンのファイル名。**中身は WebP に固定する。**
///
/// 変換（png / jpg → WebP・リサイズ）は UI 層の責務で、コアは受け入れ検証だけを持つ。
/// 形式を 1 つに固定すると、表示側は MIME 判定なしで `image/webp` として扱える。
const ICON_FILE: &str = "icon.webp";

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
    pub async fn write_config(
        &self,
        id: &AgentId,
        kind: ConfigFileKind,
        content: &str,
    ) -> CoreResult<()> {
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
        let path = self.agent_dir(id)?.join(ICON_FILE);
        match tokio::fs::read(&path).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(Self::io_err(&path, err)),
        }
    }

    /// エージェントのアイコンを書く。
    ///
    /// # Errors
    /// WebP でない・サイズ上限超過の場合 [`CoreError::InvalidIcon`]。
    /// IPC から来る任意のバイト列をそのまま書くと、ワークスペースが
    /// 「画像のふりをした何か」の置き場になる。マジック番号とサイズで入口を絞る。
    pub async fn write_icon(&self, id: &AgentId, bytes: &[u8]) -> CoreResult<()> {
        if bytes.len() > ICON_MAX_BYTES {
            return Err(CoreError::InvalidIcon {
                reason: format!(
                    "サイズが上限を超えています（{} bytes > {} bytes）",
                    bytes.len(),
                    ICON_MAX_BYTES
                ),
            });
        }
        // WebP コンテナの magic: 先頭 "RIFF" + オフセット 8 から "WEBP"。
        let is_webp =
            bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP";
        if !is_webp {
            return Err(CoreError::InvalidIcon {
                reason: "WebP 形式ではありません（UI 側で変換してから送る契約）".to_owned(),
            });
        }

        let dir = self.agent_dir(id)?;
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| Self::io_err(&dir, e))?;
        let path = dir.join(ICON_FILE);
        tokio::fs::write(&path, bytes)
            .await
            .map_err(|e| Self::io_err(&path, e))
    }

    /// エージェントのアイコンを削除する。未設定でも成功として扱う（削除は冪等）。
    pub async fn delete_icon(&self, id: &AgentId) -> CoreResult<()> {
        let path = self.agent_dir(id)?.join(ICON_FILE);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(Self::io_err(&path, err)),
        }
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
    pub async fn compose_system_prompt(&self, spec: &AgentSpec) -> CoreResult<(String, usize)> {
        let ordinance = self.read_ordinance().await?;
        let construct = self.read_config(&spec.id, ConfigFileKind::Construct).await?;
        let skill = self.read_config(&spec.id, ConfigFileKind::Skill).await?;
        let memory = self.read_config(&spec.id, ConfigFileKind::Memory).await?;

        let mut prompt = String::new();
        if !ordinance.is_empty() {
            prompt.push_str(
                "# 村の条例（この場の全員に適用される規則。個別設定より優先される）\n",
            );
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
        prompt.push_str(&format!(
            "# あなたについて\n\
             あなたはこの場に参加している **{}** です。\n\
             - **{} 自身の発言だけを書いてください。** 他のエージェントの発言を\
             代筆・代弁してはいけません。彼らはそれぞれ自分で発言します。\n\
             - 発言に自分の名前を書く必要はありません。誰の発言かは自動的に伝わります。\n\
             - 相手の発言を装って会話を先に進めないでください。\n\n",
            spec.name, spec.name
        ));
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

        let stable_len = prompt.chars().count();

        if !memory.is_empty() {
            prompt.push_str("## Memory\n");
            prompt.push_str(&memory);
            prompt.push('\n');
        }

        Ok((prompt, stable_len))
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
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(|e| Self::io_err(&self.root, e))?;

        let json = serde_json::to_string_pretty(world)?;
        let final_path = self.root.join(WORLD_FILE);
        let temp_path = self.root.join(format!("{WORLD_FILE}.tmp"));

        tokio::fs::write(&temp_path, json)
            .await
            .map_err(|e| Self::io_err(&temp_path, e))?;
        tokio::fs::rename(&temp_path, &final_path)
            .await
            .map_err(|e| Self::io_err(&final_path, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト用の一時ディレクトリ。`tempfile` を足さずに済ませるための最小実装。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "concordia-test-{tag}-{}",
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
        let (prompt, stable_len) = store.compose_system_prompt(&spec).await.unwrap();

        let stable: String = prompt.chars().take(stable_len).collect();
        assert!(stable.contains("制約A") && stable.contains("能力B"));
        assert!(!stable.contains("記憶C"), "可変部分は境界の外側");
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

        let (prompt, _) = store.compose_system_prompt(&spec).await.unwrap();

        assert!(prompt.contains("ジェミー"), "自分の名前が入ること");
        assert!(
            prompt.contains("代筆") || prompt.contains("代弁"),
            "他人の発言を書かない規則が入ること: {prompt}"
        );
    }
}
