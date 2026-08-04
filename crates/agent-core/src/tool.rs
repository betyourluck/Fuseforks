//! エージェントが実行できるツール。
//!
//! # 差し替え点としての位置づけ
//!
//! ここは **MCP サーバーを繋ぐときの受け口**でもある。MCP のツール 1 本を
//! [`AgentTool`] の実装 1 つに写せば、オーケストレーター側は何も変わらない。
//! 逆に言えば、この trait が LLM のワイヤ形にも MCP のワイヤ形にも
//! 依存しないことが、両方を同じ穴へ嵌めるための条件になる。
//!
//! # 実行の境界
//!
//! ツールは**失敗しても会話を止めない**。エラーは文字列としてモデルへ返り、
//! モデルが読んで次を決める。ツールの失敗でターンごと落とすと、
//! 「引数を間違えた」だけで会話が終わる。
//!
//! ただし返す文字列に内部情報を載せすぎないこと。ツール結果はそのまま
//! プロンプトへ入り、モデルの出力を経て外へ出うる。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::CoreResult;
use crate::llm::ToolSpec;
use crate::model::AgentId;

/// ツール実行時に渡される文脈。
///
/// **どのエージェントが呼んだか**を必ず伴う。エージェントごとに書き込み先が
/// 違う（`Memory.md` など）以上、呼び出し元を知らずに実行できる操作は少ない。
pub struct ToolContext {
    /// 呼び出したエージェント。
    pub agent_id: AgentId,
    /// 呼び出したエージェントの作業フォルダ（`AgentSpec::work_dir`）。
    ///
    /// ツール自身に world を引かせず、オーケストレーターが実行時に解決して渡す。
    /// ツールが登録簿の型を知ると、MCP ツールと同じ穴に嵌らなくなる。
    pub work_dir: Option<PathBuf>,
    /// このターンの協調的キャンセル（Spec 10）。**ほとんどのツールは見ない。**
    ///
    /// Spec 10 の不変条件 1 は「検査点は周回境界だけ」で、ツールの内側では
    /// 見ないのが原則。例外は**外部プロセスを起動するツール**（Spec 15 の `run`）で、
    /// 周回境界まで待つと最長 `timeoutSecs`（上限 1 時間）走り続ける。
    /// 「要求から 0.0 秒」で止まる Spec 10 の約束が、そこだけ破れる。
    ///
    /// **葉で 1 箇所だけ見るのは、周回境界の検査を増やすことではない** —
    /// ターンループの構造は変わらず、止められない待ちを 1 つ潰すだけ。
    pub cancel: Option<tokio_util::sync::CancellationToken>,
}

/// 実行可能なツール。
#[async_trait]
pub trait AgentTool: Send + Sync {
    /// モデルへ提示する名前。`[a-zA-Z0-9_-]{1,64}`。
    fn name(&self) -> &str;

    /// モデルへ提示する説明。**いつ呼ぶべきかを書く。** 何をするかだけでは呼ばれない。
    fn description(&self) -> String;

    /// 引数の JSON Schema。
    fn parameters(&self) -> Value;

    /// 実行する。戻り値はモデルへそのまま渡る文字列。
    ///
    /// # Errors
    /// 実行できなかった理由。呼び出し側が文字列へ落としてモデルへ返す。
    async fn call(&self, ctx: &ToolContext, args: &Value) -> CoreResult<String>;

    /// モデルへ提示する定義。
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_owned(),
            description: self.description(),
            parameters: self.parameters(),
        }
    }

    /// **その個体へ**提示する定義。既定は [`Self::spec`]（誰に対しても同じ）。
    ///
    /// 上書きするのは、提示する内容が個体で変わるツールだけ（Spec 15 の `run` は
    /// 「その個体から実行できる登録」だけを列挙する）。`description()` は
    /// [`ToolContext`] を受け取らないので、**個体別の提示はこの穴でしか書けない**。
    ///
    /// `None` を返したら**そのツールを提示しない**。既存の
    /// `WORK_DIR_TOOL_NAMES` による自動除外は名前の集合で書かれているが、
    /// 「登録が 1 件も実行可能でない」のような**中身を見ないと決まらない除外**は
    /// 名前の集合では書けない。
    fn spec_for(&self, _ctx: &ToolContext) -> Option<ToolSpec> {
        Some(self.spec())
    }
}

/// 名前で引ける登録簿。
#[derive(Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn AgentTool>>,
}

impl ToolRegistry {
    /// 空の登録簿を作る。
    pub fn new() -> Self {
        Self::default()
    }

    /// ツールを登録する。同名は置き換える。
    pub fn register(&mut self, tool: Arc<dyn AgentTool>) {
        self.tools.insert(tool.name().to_owned(), tool);
    }

    /// 名前で引く。
    pub fn get(&self, name: &str) -> Option<&Arc<dyn AgentTool>> {
        self.tools.get(name)
    }

    /// ツールを取り除く。無ければ何もしない。
    ///
    /// MCP サーバーの再接続で要る。古い接続のツールを消さずに新しいものを
    /// 登録すると、**繋がっていないサーバーのツールがモデルへ提示され続ける**
    /// （呼ぶと必ず失敗する幽霊が残る）。
    pub fn unregister(&mut self, name: &str) {
        self.tools.remove(name);
    }

    /// 登録数。
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// 空か。
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// 全ツールの定義。モデルへ提示する順は名前順で安定させる
    /// （順が揺れるとプロンプトキャッシュのプレフィックスが毎回変わる）。
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.values().map(|tool| tool.spec()).collect()
    }

    /// 登録済みの名前。
    pub fn names(&self) -> Vec<&str> {
        self.tools.keys().map(String::as_str).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dummy(&'static str);

    #[async_trait]
    impl AgentTool for Dummy {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self) -> String {
            "テスト用".into()
        }
        fn parameters(&self) -> Value {
            serde_json::json!({ "type": "object" })
        }
        async fn call(&self, _ctx: &ToolContext, _args: &Value) -> CoreResult<String> {
            Ok("ok".into())
        }
    }

    #[test]
    fn specs_are_ordered_by_name() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(Dummy("zebra")));
        registry.register(Arc::new(Dummy("alpha")));

        let names: Vec<String> = registry.specs().into_iter().map(|s| s.name).collect();
        assert_eq!(names, vec!["alpha", "zebra"], "提示順は安定していること");
    }

    #[test]
    fn registering_the_same_name_replaces() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(Dummy("same")));
        registry.register(Arc::new(Dummy("same")));
        assert_eq!(registry.len(), 1);
    }
}
