//! 実物の MCP サーバーに対する end-to-end 検証。
//!
//! **既定では走らない。** 外部コマンド（`npx`）とネットワーク（初回の
//! パッケージ取得）に依存するため、CI や普段の `cargo test` を外部要因で
//! 赤くしない。手元で経路を確かめたいときに明示して走らせる:
//!
//! ```text
//! cargo test -p agent-core --test mcp_live -- --ignored --nocapture
//! ```
//!
//! ここで確かめたいのは自分のコードの結線であって SDK の正しさではない:
//! 宣言 → 子プロセス起動 → handshake → `tools/list` → [`AgentTool`] への写像 →
//! `tools/call` → 文字列化、という**通し**が繋がっていること。
//! 単体テスト（`src/mcp.rs`）が押さえているのは純関数だけで、この経路は通らない。

use std::collections::BTreeMap;
use std::path::PathBuf;

use agent_core::mcp::{McpConfig, McpManager, McpServerConfig};
use agent_core::tool::ToolContext;
use agent_core::AgentId;

/// テスト用の一時ディレクトリ。
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "concordia-mcp-{tag}-{}",
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

/// 公式のリファレンス実装（filesystem サーバー）へ実際に繋ぐ。
///
/// `npx` は Windows では `npx.cmd` を起動する必要がある（`Command` は
/// `.cmd` を PATH 解決しない）。**これは実運用でも踏む罠**なので、
/// ここで形を確かめておく価値がある。
#[tokio::test]
#[ignore = "外部コマンド (npx) とネットワークに依存する"]
async fn connects_to_a_real_server_and_calls_a_tool() {
    let dir = TempDir::new("filesystem");
    std::fs::write(dir.0.join("hello.txt"), "こんにちは、MCP。").unwrap();

    let command = if cfg!(windows) { "npx.cmd" } else { "npx" };
    let mut servers = BTreeMap::new();
    servers.insert(
        "fs".to_owned(),
        McpServerConfig {
            command: command.to_owned(),
            args: vec![
                "-y".to_owned(),
                "@modelcontextprotocol/server-filesystem".to_owned(),
                dir.0.display().to_string(),
            ],
            env: BTreeMap::new(),
            enabled: true,
        },
    );

    let manager = McpManager::connect_all(&McpConfig { servers }).await;
    let status = manager.statuses().first().expect("1 台ぶんの状態").clone();
    assert!(
        status.connected,
        "接続できること。理由: {:?}",
        status.error
    );
    assert!(!status.tools.is_empty(), "ツールが 1 本以上見えること");

    // 名前空間の接頭辞が付いていること（サーバー跨ぎの衝突を防ぐ形）。
    assert!(
        status.tools.iter().all(|name| name.starts_with("fs__")),
        "実際: {:?}",
        status.tools
    );

    // 実際に 1 本呼ぶ。ここまで繋がって初めて経路が通ったと言える。
    let read_tool = manager
        .tools()
        .iter()
        .find(|tool| tool.name().contains("read") && tool.name().contains("file"))
        .expect("ファイル読み取りツールがあること")
        .clone();

    let ctx = ToolContext {
        agent_id: AgentId::from("agent_test"),
    };
    let args = serde_json::json!({ "path": dir.0.join("hello.txt").display().to_string() });
    let output = read_tool.call(&ctx, &args).await.expect("呼び出せること");

    assert!(
        output.contains("こんにちは、MCP。"),
        "ツールの結果が本文を含むこと。実際: {output}"
    );

    // 畳めること（子プロセスを残さない）。
    manager.shutdown().await;
}
