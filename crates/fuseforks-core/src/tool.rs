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
    /// 見出し索引を張るフォルダの宣言（Spec 18、`AgentSpec::rag_sources`）。
    /// **見るのは `rag` だけ。**
    ///
    /// `work_dir` と同じ理由でここに乗る — `rag_sources` は `World` の中に住み、
    /// ツール自身に world を引かせない以上、オーケストレーターが実行時に解決して
    /// 渡すしかない（`run` が自分で `run.json` を引けるのは、あれが
    /// `ConfigStore` のファイルだから）。宣言そのものを運び、実在検査は
    /// ツール側が呼び出しごとに掛け直す（無効化であって削除ではない —
    /// パスを直せばその場で復活する）。
    pub rag_roots: Vec<PathBuf>,
    /// モデルへ届く文言の言語（Spec 35。村の `language`）。
    ///
    /// **提示時にも実行時にも渡る** — `spec_for(&self, ctx)` が提示時に ctx を
    /// 受けるので、個体別の提示文（`run` の「実行できる登録」の説明）を
    /// この値で書き分けられる。`work_dir` と同じく、ツール自身に world を
    /// 引かせず、オーケストレーターが解決して渡す。
    pub language: crate::world::Language,
}

/// 実行可能なツール。
#[async_trait]
pub trait AgentTool: Send + Sync {
    /// モデルへ提示する名前。`[a-zA-Z0-9_-]{1,64}`。
    fn name(&self) -> &str;

    /// モデルへ提示する説明。**いつ呼ぶべきかを書く。** 何をするかだけでは呼ばれない。
    ///
    /// `language` は村の言語（Spec 35。**モデルへ届く文言だけ**が対象で、
    /// 名前・schema の構造は言語に依存しない）。[`crate::mcp::McpTool`] は
    /// 受け取っても無視する — 名付けたのは接続先で、訳語を当てると
    /// 何が走ったかについて嘘になる。
    fn description(&self, language: crate::world::Language) -> String;

    /// 引数の JSON Schema。文言（`description` 値）だけが `language` で変わる。
    fn parameters(&self, language: crate::world::Language) -> Value;

    /// 実行する。戻り値はモデルへそのまま渡る文字列。
    ///
    /// # Errors
    /// 実行できなかった理由。呼び出し側が文字列へ落としてモデルへ返す。
    async fn call(&self, ctx: &ToolContext, args: &Value) -> CoreResult<String>;

    /// モデルへ提示する定義。
    fn spec(&self, language: crate::world::Language) -> ToolSpec {
        ToolSpec {
            name: self.name().to_owned(),
            description: self.description(language),
            parameters: self.parameters(language),
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
    async fn spec_for(&self, ctx: &ToolContext) -> Option<ToolSpec> {
        Some(self.spec(ctx.language))
    }

    /// 理由欄（Spec 27）を提示するか。**既定は真。**
    ///
    /// **偽を返すのは [`crate::mcp::McpTool`] だけ** — 他人が宣言したスキーマへ
    /// こちらの欄を生やして転送すると、`additionalProperties: false` の
    /// サーバーが拒否する。
    ///
    /// **新しい自前のツールは既定で対象**になる。外す判断が要るのは
    /// 「引数が既に別の形で画面へ出るツール」を足したときだけ
    /// （`ask` / `handoff` / `plan` は `AgentTool` を実装していないので
    /// そもそもここを通らない）。
    fn wants_reason(&self) -> bool {
        true
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
    pub fn specs(&self, language: crate::world::Language) -> Vec<ToolSpec> {
        self.tools.values().map(|tool| tool.spec(language)).collect()
    }

    /// 登録済みの名前。
    pub fn names(&self) -> Vec<&str> {
        self.tools.keys().map(String::as_str).collect()
    }

    /// **その個体へ**提示する定義。[`AgentTool::spec_for`] が `None` を返した
    /// ツールは落ちる。順は [`Self::specs`] と同じく名前順で安定。
    ///
    /// **理由欄（Spec 27）の注入はここ 1 箇所。** [`AgentTool::spec()`] の既定に
    /// 置かない理由が 2 つある:
    ///
    /// - [`crate::mcp::McpTool`] も [`AgentTool`] なので、既定に置くと
    ///   **外部のスキーマにこちらの欄が生える**
    /// - **[`AgentTool::spec_for`] を上書きしているツールは既定の `spec()` を
    ///   通らない**（Spec 15 の `run`）。既定に置くと `run` にだけ乗らない
    ///
    /// **ここは全ツールが通る唯一の漏斗**で、説明文の定数も 1 箇所に閉じる。
    pub async fn specs_for(&self, ctx: &ToolContext) -> Vec<ToolSpec> {
        let mut specs = Vec::with_capacity(self.tools.len());
        for tool in self.tools.values() {
            if let Some(mut spec) = tool.spec_for(ctx).await {
                if tool.wants_reason() {
                    crate::tool_reason::inject(&mut spec.parameters, ctx.language);
                }
                specs.push(spec);
            }
        }
        specs
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
        fn description(&self, _language: crate::world::Language) -> String {
            "テスト用".into()
        }
        fn parameters(&self, _language: crate::world::Language) -> Value {
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

        let names: Vec<String> = registry.specs(crate::world::Language::Ja).into_iter().map(|s| s.name).collect();
        assert_eq!(names, vec!["alpha", "zebra"], "提示順は安定していること");
    }

    #[test]
    fn registering_the_same_name_replaces() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(Dummy("same")));
        registry.register(Arc::new(Dummy("same")));
        assert_eq!(registry.len(), 1);
    }

    /// 理由欄を持たないツール（`McpTool` の代役）。
    struct Quiet(&'static str);

    #[async_trait]
    impl AgentTool for Quiet {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self, _language: crate::world::Language) -> String {
            "外部ツールの代役".into()
        }
        fn parameters(&self, _language: crate::world::Language) -> Value {
            serde_json::json!({ "type": "object", "properties": {} })
        }
        async fn call(&self, _ctx: &ToolContext, _args: &Value) -> CoreResult<String> {
            Ok("ok".into())
        }
        fn wants_reason(&self) -> bool {
            false
        }
    }

    fn ctx() -> ToolContext {
        ToolContext {
            agent_id: AgentId::new("agent_1"),
            work_dir: None,
            cancel: None,
            rag_roots: Vec::new(),
            language: crate::world::Language::Ja,
        }
    }

    #[tokio::test]
    async fn specs_for_injects_the_reason_field() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(Dummy("loud")));

        let specs = registry.specs_for(&ctx()).await;
        assert_eq!(specs.len(), 1);
        assert_eq!(
            specs[0].parameters["properties"][crate::tool_reason::REASON_KEY]["type"],
            "string",
            "既定のツールには理由欄が生える"
        );
    }

    #[tokio::test]
    async fn specs_for_leaves_opted_out_tools_untouched() {
        // **外部のスキーマへこちらの欄を生やすと、そのまま tools/call の引数として
        // 転送される。** `additionalProperties: false` のサーバーは呼び出しごと拒否する。
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(Quiet("external")));

        let specs = registry.specs_for(&ctx()).await;
        assert_eq!(specs.len(), 1);
        assert!(
            specs[0].parameters["properties"]
                .get(crate::tool_reason::REASON_KEY)
                .is_none(),
            "オプトアウトしたツールには 1 バイトも足さない"
        );
    }

    #[tokio::test]
    async fn opting_out_does_not_leak_to_the_neighbours() {
        // 1 本だけ外れることを、外れない相方と**対で**見る。
        // 片方だけを見ると「全部に足す実装」も「全部に足さない実装」も緑になる。
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(Dummy("loud")));
        registry.register(Arc::new(Quiet("external")));

        let specs = registry.specs_for(&ctx()).await;
        let has_reason = |name: &str| {
            specs
                .iter()
                .find(|s| s.name == name)
                .expect("登録したツールは提示される")
                .parameters["properties"]
                .get(crate::tool_reason::REASON_KEY)
                .is_some()
        };
        assert!(has_reason("loud"));
        assert!(!has_reason("external"));
    }

    /// **理由欄をオプトアウトしているのが `mcp.rs` だけであることを固定する。**
    ///
    /// 本数では数えない（`failures.md` #62 — 数の記述は増えた名前で grep しても
    /// 引っかからない）。**名前で突き合わせる**のは
    /// `defaultEnabledTools.test.ts` が Rust と Vue の 2 つの表でやっているのと同じ形。
    ///
    /// このテストが赤くなるのは、**新しいツールが理由欄を黙って外したとき**。
    /// 外すこと自体は正しい場合もあるが、**契約の対象一覧を直さずに外せないようにする**。
    #[test]
    fn only_mcp_opts_out_of_the_reason_field() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut opted_out = Vec::new();

        let mut stack = vec![src];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("src は読める") {
                let path = entry.expect("エントリは読める").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|ext| ext != "rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).expect("ソースは読める");
                // テスト内の代役（`Quiet`）は数えない。
                if path.file_name().is_some_and(|n| n == "tool.rs") {
                    continue;
                }
                if text.contains("fn wants_reason") {
                    opted_out.push(
                        path.file_name()
                            .expect("ファイル名がある")
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
            }
        }
        opted_out.sort();
        assert_eq!(
            opted_out,
            vec!["mcp.rs".to_owned()],
            "理由欄を上書きしてよいのは McpTool だけ（Spec 27 D5 / tool_reason_contract）"
        );
    }
}
