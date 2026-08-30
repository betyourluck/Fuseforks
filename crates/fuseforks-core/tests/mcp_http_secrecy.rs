//! Spec 47 P2 の結合テスト — リモート MCP の接続失敗が **headers の値を
//! 運ばない**こと（D7）。
//!
//! ループバックに「受け取った Authorization を応答本文へエコーする 401
//! サーバー」を立てる。**これが D7 の脅威の実物** — rmcp のエラーは応答本文を
//! 逐語で運ぶ（P0 実測）ので、分類を挟まず `McpServerStatus.error` へ写すと
//! トークンが画面とログへ出る。スタブの水準は `attachment_fallback.rs` と同じ
//! （tokio の net でループバックの最小 HTTP）。

use fuseforks_core::mcp::{McpConfig, McpManager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// 受けたリクエストの Authorization ヘッダーを本文へエコーして 401 を返す。
async fn spawn_echoing_401_server() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("ループバックへ bind できること");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut buffer = vec![0_u8; 8192];
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
                let auth_line = request
                    .lines()
                    .find(|line| line.to_ascii_lowercase().starts_with("authorization:"))
                    .unwrap_or("authorization: (none)")
                    .to_owned();
                let body =
                    format!(r#"{{"error":"UNAUTHENTICATED","echo":"{}"}}"#, auth_line.trim());
                let response = format!(
                    "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    port
}

#[tokio::test]
async fn a_401_with_an_echoing_body_does_not_leak_the_token() {
    let port = spawn_echoing_401_server().await;

    // 入口はワイヤ形から（parse → 検証 → 接続、の全経路を通す）。
    // loopback の http が D4 を通ることの結合面でもある。
    let raw = format!(
        r#"{{ "mcpServers": {{ "echo": {{
            "type": "http",
            "url": "http://127.0.0.1:{port}/mcp",
            "headers": {{ "Authorization": "Bearer super-secret-token-123" }}
        }} }} }}"#
    );
    let config: McpConfig = serde_json::from_str(&raw).expect("受理されること");

    let manager = McpManager::connect_all(&config).await;
    let status = manager.statuses().first().expect("1 台ぶんの状態").clone();

    assert!(!status.connected, "401 で接続失敗になること");
    let error = status.error.expect("理由が残ること");
    // 分類はされている（沈黙にしない — D5）。
    assert!(error.contains("HTTP 401"), "実際: {error}");
    assert!(error.contains("再試行しません"), "実際: {error}");
    // **トークンは 1 文字も出ない**（D7 の本体）。サーバーは本文へエコー
    // している = 分類を外すと必ず漏れる入力になっている。
    assert!(
        !error.contains("super-secret-token-123"),
        "トークンが漏れている: {error}"
    );
    assert!(!error.contains("UNAUTHENTICATED"), "応答本文が漏れている: {error}");

    manager.shutdown().await;
}
