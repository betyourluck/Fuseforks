//! 応答本文は塊ごとに上限を見て読む（Spec 77 #7b / N5）。
//!
//! # 何が起きていたか
//!
//! LLM の応答（`response.text()`）・OAuth の探索（`response.text()`）・単価表（`response.bytes()`）は
//! 本文を**全部読んでから**大きさを見ていた（または見ていなかった）。`Content-Length` を付けずに
//! 桁外れの本文を送る相手には、その大きさのメモリを確保し、送り終えない相手には待ちの上限まで
//! 止まる。rmcp の `ReqwestOAuthHttpClient`（写し元）は 1 塊ずつ読み、溜める前に上限を見ている。
//!
//! # 測り方
//!
//! ループバックのスタブが `Content-Length` 無しで上限を超える本文を送り、**送り終えずに接続を
//! 持ち続ける**。全量を読む実装は待ちの上限まで返らず、塊ごとに上限を見る実装は上限を超えた
//! 時点で返る — 返るまでの時間が判定になる。

use std::time::Duration;

use fuseforks_core::llm::{ChatMessage, ChatRequest, LlmBackend, LlmError};
use fuseforks_core::model::{CredentialSource, ModelTemplate};
use fuseforks_core::{HttpLlmBackend, InMemorySecretStore, LlmConfig, Provider};

/// `Content-Length` 無しで `body_len` バイトを送り、送り終えたら接続を持ったまま眠るスタブ。
async fn spawn_stalling_stub(body_len: usize) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                // 要求の頭だけ読む（本文の有無は問わない — 応答は要求を待たずに返してよい）。
                let mut buf = [0u8; 8192];
                let mut raw = Vec::new();
                loop {
                    let Ok(n) = socket.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    if raw.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let head = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n";
                if socket.write_all(head.as_bytes()).await.is_err() {
                    return;
                }
                let chunk = vec![b'a'; 64 * 1024];
                let mut sent = 0;
                while sent < body_len {
                    let n = chunk.len().min(body_len - sent);
                    if socket.write_all(&chunk[..n]).await.is_err() {
                        return;
                    }
                    sent += n;
                }
                // 送り終えない（接続を持ち続ける）。
                tokio::time::sleep(Duration::from_secs(120)).await;
                drop(socket);
            });
        }
    });

    format!("http://127.0.0.1:{port}")
}

#[tokio::test]
async fn an_oversized_llm_response_is_refused_without_waiting_for_the_rest() {
    // LLM の応答の上限（`llm/client.rs` の `MAX_RESPONSE_BODY` = 32 MiB）を少し超える。
    let base_url = spawn_stalling_stub(32 * 1024 * 1024 + 64 * 1024).await;
    let mut template = ModelTemplate::new("tpl_stub", "スタブ", "stub-model");
    template.base_url = format!("{base_url}/v1");
    template.credential = CredentialSource::NotRequired;
    template.provider = Some(Provider::OpenAiCompat);
    template.max_retries = 0;
    let config = LlmConfig::from_template(&template, &InMemorySecretStore::new()).unwrap();
    let backend = HttpLlmBackend::new(config).unwrap();

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        backend.chat(ChatRequest::plain("stub-model", vec![ChatMessage::user("ping")], 16)),
    )
    .await
    .expect("上限を超えた時点で返る（全量を待たない）");
    let err = result.expect_err("上限を超えた応答は失敗");
    assert!(
        matches!(&err, LlmError::Api { status: 200, body, .. } if body.contains("上限")),
        "上限を超えたことを名指しする: {err:?}"
    );
}

#[tokio::test]
async fn oauth_discovery_stops_reading_at_the_cap() {
    // OAuth の本文の上限（1 MiB）を少し超える。探索は 2 つの候補を順に試すので、全量を読む
    // 実装は 1 候補あたり待ちの上限（10 秒）ずつ止まる。
    let base_url = spawn_stalling_stub(1024 * 1024 + 64 * 1024).await;
    let started = std::time::Instant::now();
    let scopes = tokio::time::timeout(
        Duration::from_secs(5),
        fuseforks_core::mcp_oauth::fetch_resource_scopes(&format!("{base_url}/mcp")),
    )
    .await
    .expect("上限を超えた時点で次の候補へ進む（待ちの上限まで止まらない）");
    assert_eq!(scopes, None, "読めないメタデータは名乗らなかったのと同じ");
    assert!(started.elapsed() < Duration::from_secs(5));
}
