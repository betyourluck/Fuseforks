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

/// `AgentSpec::enabled_tools` による提示制御の**対象**となる同梱ツールの一覧。
/// MCP 由来・転送・委譲ツールは対象外（enabled_tools_invariant）。
///
/// **`rag` はここに入れない**（Spec 18 D13。利用者裁定 2026-08-05 で
/// rev3 の D7「既定集合に入れる」を覆した）。`rag` の提示は
/// **宣言（`rag_sources`）だけ**が決める — 宣言を書けるのは人だけなので、
/// 宣言そのものがオプトインであり、チェックボックスは同じ意図に対する
/// 2 つ目のスイッチだった。実機で即座に踏んだ: 既存の村は全個体が明示配列
/// （Spec 02 の頃にツール選択を触った履歴）で、明示配列に新ツールは自動で
/// 増えないため、**フォルダを宣言しても誰にも `rag` が出なかった**。
/// この集合から外れたことで `is_bundled_tool_presented` を素通りし、
/// `RagTool::spec_for`（宣言が空または全滅なら提示しない）だけが効く。
pub const BUNDLED_TOOL_NAMES: [&str; 8] =
    ["diff", "fd", "file", "grep", "remember", "run", "sd", "yq"];

/// `enabled_tools: None`（既定に従う）で提示する集合。
///
/// **`BUNDLED_TOOL_NAMES = DEFAULT_ENABLED_TOOLS ∪ {run}`。** `run` だけが
/// 既定集合の外に居る（Spec 15、破壊的変更）。
///
/// `run` を他の 7 本と同じ扱いにすると、**アプリを更新した瞬間に全個体が
/// コマンド実行能力を得る**。`batch_start_invariant` が「開いただけで課金が
/// 始まる作りにしない」と言うのと同じ形で、**更新しただけで実行能力が増える
/// 作りにしない**。
///
/// 移行は不要 — 既存の `world.json` は書き換えず、`None` の解釈が変わるだけで
/// **既存個体は `run` を得ない**のが移行の目的。
pub const DEFAULT_ENABLED_TOOLS: [&str; 7] =
    ["diff", "fd", "file", "grep", "remember", "sd", "yq"];

/// 作業フォルダが無いと動かない同梱ツール。
///
/// 未設定のエージェントには enabled_tools に関わらず**提示しない** —
/// 呼んでも「未設定です」と答えるだけのツールに、毎ターンスキーマ分の
/// トークンを払わない（使えないツールを見せない）。
pub const WORK_DIR_TOOL_NAMES: [&str; 6] = ["diff", "fd", "file", "grep", "sd", "yq"];
