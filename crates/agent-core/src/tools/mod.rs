//! 同梱ツール。
//!
//! ここに置くのは「Concordia 自身が提供する操作」だけに限る。
//! 外部の能力（ファイル操作・検索・API 呼び出し）は MCP サーバー経由で足す。
//! 何でもここへ足すと、シングルバイナリに世界中の依存が生えてくる。

pub mod edit;
pub mod fs;
pub mod memory;

pub use edit::{SdTool, YqTool};
pub use fs::{DiffTool, FdTool, GrepTool};
pub use memory::RememberTool;

/// 同梱ツールの名前一覧。`AgentSpec::enabled_tools` による提示制御の対象は
/// この集合だけで、MCP 由来・転送・委譲ツールは対象外（enabled_tools_invariant）。
pub const BUNDLED_TOOL_NAMES: [&str; 6] = ["diff", "fd", "grep", "remember", "sd", "yq"];

/// 作業フォルダが無いと動かない同梱ツール。
///
/// 未設定のエージェントには enabled_tools に関わらず**提示しない** —
/// 呼んでも「未設定です」と答えるだけのツールに、毎ターンスキーマ分の
/// トークンを払わない（使えないツールを見せない）。
pub const WORK_DIR_TOOL_NAMES: [&str; 5] = ["diff", "fd", "grep", "sd", "yq"];
