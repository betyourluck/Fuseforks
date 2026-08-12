//! 同梱ツール 9 本の提示（description + parameters）の golden（Spec 35 検収 1）。
//!
//! **fixture ファイルとの比較で日本語のバイト等価を固定する。** システムプロンプトの
//! golden（prompt_golden.rs）がリテラル埋め込みなのに対し、こちらは 9 本ぶんで
//! 約 6KB あるので fixture ファイル（git で diff できる）にした。
//! 行末は正規化して比べる（git の改行変換と提示の等価性は無関係）。
//!
//! fixture は 2026-08-12（Spec 35 P2 着手前）の実出力を焼いたもの。

use fuseforks_core::config_store::ConfigStore;
use fuseforks_core::tool::AgentTool;
use fuseforks_core::tools::{
    DiffTool, FdTool, FileTool, GrepTool, RagTool, RememberTool, RunTool, SdTool, YqTool,
};

fn all_tools() -> Vec<Box<dyn AgentTool>> {
    let dir = std::env::temp_dir().join("ff-toolspec-golden");
    let _ = std::fs::create_dir_all(&dir);
    let store = ConfigStore::new(&dir);
    vec![
        Box::new(GrepTool),
        Box::new(FdTool),
        Box::new(DiffTool),
        Box::new(SdTool),
        Box::new(YqTool),
        Box::new(FileTool),
        Box::new(RagTool),
        Box::new(RememberTool::new(store.clone())),
        Box::new(RunTool::new(store)),
    ]
}

fn render(tools: &[Box<dyn AgentTool>]) -> String {
    let mut out = String::new();
    for tool in tools {
        let spec = tool.spec();
        out.push_str(&format!(
            "=== {} ===\n{}\n{}\n",
            spec.name,
            spec.description,
            serde_json::to_string_pretty(&spec.parameters).unwrap()
        ));
    }
    out
}

/// 捕獲用（1 回だけ使う）。
#[test]
#[ignore = "fixture の焼き付け用"]
fn dump_fixture() {
    std::fs::write(
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tool_specs_ja.txt"),
        render(&all_tools()),
    )
    .unwrap();
}

#[test]
fn ja_tool_specs_are_byte_identical_to_the_pre_spec35_output() {
    let expected = include_str!("fixtures/tool_specs_ja.txt").replace("\r\n", "\n");
    let actual = render(&all_tools()).replace("\r\n", "\n");
    assert_eq!(actual, expected, "同梱ツールの日本語の提示が変わった");
}
