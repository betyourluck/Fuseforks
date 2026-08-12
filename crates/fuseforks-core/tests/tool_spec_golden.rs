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

fn render(tools: &[Box<dyn AgentTool>], language: fuseforks_core::world::Language) -> String {
    let mut out = String::new();
    for tool in tools {
        let spec = tool.spec(language);
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
        render(&all_tools(), fuseforks_core::world::Language::Ja),
    )
    .unwrap();
}

#[test]
fn ja_tool_specs_are_byte_identical_to_the_pre_spec35_output() {
    let expected = include_str!("fixtures/tool_specs_ja.txt").replace("\r\n", "\n");
    let actual = render(&all_tools(), fuseforks_core::world::Language::Ja).replace("\r\n", "\n");
    assert_eq!(actual, expected, "同梱ツールの日本語の提示が変わった");
}

fn ja_chars(s: &str) -> usize {
    s.chars()
        .filter(|c| matches!(*c as u32, 0x3040..=0x309F | 0x30A0..=0x30FF | 0x4E00..=0x9FFF))
        .count()
}

/// 英語の提示に日本語が 1 文字も無い（検収 2 のツール版）。
#[test]
fn en_tool_specs_contain_no_japanese() {
    let text = render(&all_tools(), fuseforks_core::world::Language::En);
    assert_eq!(ja_chars(&text), 0, "英語の提示に日本語が残っている:\n{text}");
}

/// `run` の個体別提示（spec_for）も英語で組める。
///
/// spec() の既定説明ではなく **allow が空のときの説明文**が本命 —
/// あの文はモデルの次の手（利用者へ承認を頼む）を運ぶので、
/// 英語村で日本語のまま残ると一番読まれる場所で混ざる。
#[tokio::test]
async fn en_run_presentation_for_empty_allow_is_english() {
    use fuseforks_core::tool::ToolContext;
    let dir = std::env::temp_dir().join("ff-toolspec-run-en");
    let _ = std::fs::create_dir_all(&dir);
    let tool = RunTool::new(ConfigStore::new(&dir));
    let ctx = ToolContext {
        agent_id: fuseforks_core::AgentId::from("agent_99"),
        work_dir: None,
        cancel: None,
        rag_roots: Vec::new(),
        language: fuseforks_core::world::Language::En,
    };
    let spec = tool.spec_for(&ctx).await.expect("allow が空でも提示する");
    assert_eq!(ja_chars(&spec.description), 0, "{}", spec.description);
    assert!(spec.description.contains("No commands are currently allowed"), "{}", spec.description);
}
