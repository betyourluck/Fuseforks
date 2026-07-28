//! 同梱ツール。
//!
//! ここに置くのは「Concordia 自身が提供する操作」だけに限る。
//! 外部の能力（ファイル操作・検索・API 呼び出し）は MCP サーバー経由で足す。
//! 何でもここへ足すと、シングルバイナリに世界中の依存が生えてくる。

pub mod memory;

pub use memory::RememberTool;
