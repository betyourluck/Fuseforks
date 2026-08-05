//! 同梱ツール。
//!
//! ここに置くのは「Concordia 自身が提供する操作」だけに限る。
//! 外部の能力（ファイル操作・検索・API 呼び出し）は MCP サーバー経由で足す。
//! 何でもここへ足すと、シングルバイナリに世界中の依存が生えてくる。

pub mod edit;
pub mod file;
pub mod fs;
pub mod memory;
pub mod rag;
pub mod run;

pub use edit::{SdTool, YqTool};
pub use file::FileTool;
pub use fs::{DiffTool, FdTool, GrepTool};
pub use memory::RememberTool;
pub use rag::RagTool;
pub use run::RunTool;

/// 同梱ツールの名前一覧。`AgentSpec::enabled_tools` による提示制御の対象は
/// この集合だけで、MCP 由来・転送・委譲ツールは対象外（enabled_tools_invariant）。
pub const BUNDLED_TOOL_NAMES: [&str; 9] =
    ["diff", "fd", "file", "grep", "rag", "remember", "run", "sd", "yq"];

/// `enabled_tools: None`（既定に従う）で提示する集合。
///
/// **`BUNDLED_TOOL_NAMES = DEFAULT_ENABLED_TOOLS ∪ {run}`。** `run` だけが
/// 既定集合の外に居る（Spec 15、破壊的変更）。
///
/// `run` を他の 8 本と同じ扱いにすると、**アプリを更新した瞬間に全個体が
/// コマンド実行能力を得る**。`batch_start_invariant` が「開いただけで課金が
/// 始まる作りにしない」と言うのと同じ形で、**更新しただけで実行能力が増える
/// 作りにしない**。
///
/// 移行は不要 — 既存の `world.json` は書き換えず、`None` の解釈が変わるだけで
/// **既存個体は `run` を得ない**のが移行の目的。
///
/// **`rag` は既定集合に入る**（Spec 18）。`run` を外した理由（外部プロセス起動と
/// いう別種の危険）は `rag` に当たらない — 読むだけで、人が宣言したフォルダしか
/// 読めず、宣言が無い個体には 2 段ゲート（`spec_for`）が提示自体を落とす。
pub const DEFAULT_ENABLED_TOOLS: [&str; 8] =
    ["diff", "fd", "file", "grep", "rag", "remember", "sd", "yq"];

/// 作業フォルダが無いと動かない同梱ツール。
///
/// 未設定のエージェントには enabled_tools に関わらず**提示しない** —
/// 呼んでも「未設定です」と答えるだけのツールに、毎ターンスキーマ分の
/// トークンを払わない（使えないツールを見せない）。
pub const WORK_DIR_TOOL_NAMES: [&str; 6] = ["diff", "fd", "file", "grep", "sd", "yq"];
