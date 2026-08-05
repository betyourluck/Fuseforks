//! `Memory.md` へ書き込む・読み出すツール。
//!
//! 長期記憶の**最小の形**。ファイルは既にあり、システムプロンプトへ入る経路も
//! 通っているので、書き込む手段を渡すだけで「自己更新する記憶」になる。
//!
//! ここで検索や忘却は持たない。量が増えて手に負えなくなってから、
//! 構造化された記憶（別 DB の Memoria）へ移す。**先に構造を作ると、
//! 何を憶えるべきかが分からないまま器だけができる。**

use async_trait::async_trait;
use serde_json::Value;

use crate::config_store::ConfigStore;
use crate::error::CoreResult;
use crate::model::ConfigFileKind;
use crate::tool::{AgentTool, ToolContext};

/// 1 回の書き込みで受け付ける最大文字数。
///
/// 上限が無いと、モデルが会話全体を貼り付けて `Memory.md` を肥大させる。
/// 記憶はプロンプトの先頭に毎回入るので、太るとそのぶん毎ターン課金される。
const MAX_NOTE_CHARS: usize = 500;

/// `Memory.md` の末尾へ 1 行追記する。
pub struct RememberTool {
    store: ConfigStore,
}

impl RememberTool {
    /// 設定ファイルの置き場を指定して作る。
    pub fn new(store: ConfigStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl AgentTool for RememberTool {
    fn name(&self) -> &str {
        "remember"
    }

    fn description(&self) -> String {
        format!(
            "後の会話でも憶えておくべきことを長期記憶へ書き留める。\
             会話が終わっても保持され、次回以降のあなたのプロンプトに含まれる。\
             書くのは**後で判断に使う事実**（相手の好み、決まった方針、繰り返し出る前提）だけにすること。\
             その場限りの話題や、いま答えれば済むことは書かない。{MAX_NOTE_CHARS} 文字まで。"
        )
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "note": {
                    "type": "string",
                    "description": "書き留める内容。一文で、後から読んで意味が分かる形にすること"
                }
            },
            "required": ["note"],
            "additionalProperties": false
        })
    }

    async fn call(&self, ctx: &ToolContext, args: &Value) -> CoreResult<String> {
        let Some(note) = args.get("note").and_then(Value::as_str) else {
            return Ok("引数 `note` が必要です。".into());
        };

        let note = note.trim();
        if note.is_empty() {
            return Ok("空の内容は書き留めません。".into());
        }
        if note.chars().count() > MAX_NOTE_CHARS {
            return Ok(format!(
                "長すぎます（{MAX_NOTE_CHARS} 文字まで）。要点だけに絞ってください。"
            ));
        }

        let existing = self
            .store
            .read_config(&ctx.agent_id, ConfigFileKind::Memory)
            .await?;

        // 同じことを何度も書かせない。モデルは前回書いたことを忘れて重ねて書く。
        if existing.lines().any(|line| line.trim_start_matches("- ") == note) {
            return Ok("同じ内容が既にあります。書き足しませんでした。".into());
        }

        let mut updated = existing;
        if !updated.is_empty() && !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push_str("- ");
        updated.push_str(note);
        updated.push('\n');

        self.store
            .write_config(&ctx.agent_id, ConfigFileKind::Memory, &updated)
            .await?;

        Ok("書き留めました。".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AgentId;

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "concordia-tool-{tag}-{}",
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

    fn ctx() -> ToolContext {
        ToolContext {
            agent_id: AgentId::from("agent_01"),
            work_dir: None,
            cancel: None,
            rag_roots: Vec::new(),
        }
    }

    #[tokio::test]
    async fn notes_are_appended_as_a_list() {
        let dir = TempDir::new("append");
        let store = ConfigStore::new(&dir.0);
        let tool = RememberTool::new(store.clone());

        tool.call(&ctx(), &serde_json::json!({ "note": "相手は簡潔な返答を好む" }))
            .await
            .unwrap();
        tool.call(&ctx(), &serde_json::json!({ "note": "週次で進捗を報告する" }))
            .await
            .unwrap();

        let saved = store
            .read_config(&AgentId::from("agent_01"), ConfigFileKind::Memory)
            .await
            .unwrap();
        assert_eq!(saved, "- 相手は簡潔な返答を好む\n- 週次で進捗を報告する\n");
    }

    #[tokio::test]
    async fn duplicate_notes_are_not_appended() {
        let dir = TempDir::new("dup");
        let store = ConfigStore::new(&dir.0);
        let tool = RememberTool::new(store.clone());

        let note = serde_json::json!({ "note": "同じこと" });
        tool.call(&ctx(), &note).await.unwrap();
        let reply = tool.call(&ctx(), &note).await.unwrap();

        assert!(reply.contains("既にあります"));
        let saved = store
            .read_config(&AgentId::from("agent_01"), ConfigFileKind::Memory)
            .await
            .unwrap();
        assert_eq!(saved.lines().count(), 1);
    }

    /// 引数不正でも Err にしない。モデルが読んで直せる文字列を返す。
    #[tokio::test]
    async fn bad_arguments_come_back_as_a_readable_message() {
        let dir = TempDir::new("bad");
        let tool = RememberTool::new(ConfigStore::new(&dir.0));

        let reply = tool.call(&ctx(), &serde_json::json!({})).await.unwrap();
        assert!(reply.contains("note"));

        let long = "あ".repeat(MAX_NOTE_CHARS + 1);
        let reply = tool
            .call(&ctx(), &serde_json::json!({ "note": long }))
            .await
            .unwrap();
        assert!(reply.contains("長すぎます"));
    }
}
