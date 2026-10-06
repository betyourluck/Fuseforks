//! Spec 47 P2 の結合テスト — リモート MCP の接続失敗が **headers の値を
//! 運ばない**こと（D7）。
//!
//! ループバックに「受け取った Authorization を応答本文へエコーする 401
//! サーバー」を立てる。**これが D7 の脅威の実物** — rmcp のエラーは応答本文を
//! 逐語で運ぶ（P0 実測）ので、分類を挟まず `McpServerStatus.error` へ写すと
//! トークンが画面とログへ出る。スタブの水準は `attachment_fallback.rs` と同じ
//! （tokio の net でループバックの最小 HTTP）。

use std::sync::{Arc, Mutex};

use fuseforks_core::mcp::{McpConfig, McpManager};
use fuseforks_core::{InMemorySecretStore, SecretStore};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// 受けた要求の Authorization を記録して 401 を返す（Spec 65 D3 の結合）。
/// 返すのは 401 だけ — 確かめたいのは「何が送られたか」と「送られたか」で、接続の成否ではない。
async fn spawn_recording_401_server() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("ループバックへ bind できること");
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&seen);
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let record = Arc::clone(&record);
            tokio::spawn(async move {
                let mut buffer = vec![0_u8; 8192];
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..read]).into_owned();
                let auth = request
                    .lines()
                    .find(|line| line.to_ascii_lowercase().starts_with("authorization:"))
                    .unwrap_or("authorization: (none)")
                    .to_owned();
                record.lock().unwrap().push(auth);
                let response = "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\
                                Connection: close\r\n\r\n";
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    (port, seen)
}

fn config_with_ref(port: u16) -> McpConfig {
    let raw = format!(
        r#"{{ "mcpServers": {{ "ref": {{
            "type": "http",
            "url": "http://127.0.0.1:{port}/mcp",
            "headers": {{ "Authorization": "Bearer ${{secret:REF_TOKEN}}" }}
        }} }} }}"#
    );
    serde_json::from_str(&raw).expect("受理されること")
}

/// 引けた参照は値へ置き換わって相手へ届く（`mcp:REF_TOKEN` の置き場から）。
#[tokio::test]
async fn a_resolved_secret_ref_is_sent_as_the_secret() {
    let (port, seen) = spawn_recording_401_server().await;
    let store = InMemorySecretStore::new();
    store.set("mcp:REF_TOKEN", "resolved-value-42").unwrap();

    let manager = McpManager::connect_all(&config_with_ref(port), &store).await;
    let status = manager.statuses().first().expect("1 台ぶんの状態").clone();
    assert!(!status.connected, "相手は 401 を返す");
    let error = status.error.expect("理由が残ること");
    assert!(!error.contains("resolved-value-42"), "値が漏れている: {error}");

    let seen = seen.lock().unwrap().clone();
    assert!(!seen.is_empty(), "相手へ届いていること");
    assert!(
        seen.iter().all(|line| line.ends_with("Bearer resolved-value-42")),
        "置き換えた値が送られていること: {seen:?}"
    );
    assert!(
        seen.iter().all(|line| !line.contains("${secret:")),
        "プレースホルダが送られている: {seen:?}"
    );
    manager.shutdown().await;
}

/// 引けない参照があれば相手へ 1 度も接続しない。理由は名前と変数名だけ。
#[tokio::test]
async fn an_unresolved_secret_ref_never_contacts_the_server() {
    let (port, seen) = spawn_recording_401_server().await;

    let manager =
        McpManager::connect_all(&config_with_ref(port), &InMemorySecretStore::new()).await;
    let status = manager.statuses().first().expect("1 台ぶんの状態").clone();
    assert!(!status.connected);
    let error = status.error.expect("理由が残ること");
    assert!(error.contains("REF_TOKEN"), "{error}");
    assert!(error.contains("FUSEFORKS_SECRET_MCP_REF_TOKEN"), "{error}");
    assert!(error.contains("接続していません"), "{error}");

    // 届いていれば spawn した受け手が記録する。少し待ってから 0 件を確かめる。
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(seen.lock().unwrap().is_empty(), "相手へ接続している: {:?}", seen.lock().unwrap());
    manager.shutdown().await;
}

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

    let manager = McpManager::connect_all(&config, &InMemorySecretStore::new()).await;
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
