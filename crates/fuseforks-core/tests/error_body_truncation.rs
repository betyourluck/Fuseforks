//! 接続先のエラー本文を `MAX_ERROR_BODY`（2,000 バイト）で切るとき、文字の途中で切らない（2026-10-19）。
//!
//! # 何が起きていたか
//!
//! `HttpLlmBackend::attempt` は失敗した応答の本文を `String::truncate(MAX_ERROR_BODY)` で切っていた。
//! `String::truncate` は切る位置が UTF-8 の文字の途中だと panic する。接続先のエラー本文が 2,000 バイトを
//! 超え、2,000 バイト目が多バイト文字（日本語など）の途中に来ると、**エラーを返すはずの呼び出しが panic した**。
//! Spec 68 rev2 の起票の途中で見つかった（Notes 5）。
//!
//! # なぜスタブを立てるか
//!
//! 切っているのは HTTP の応答を読む場所で、純関数の単体では「実際の応答の本文が届いて切られる」に届かない。
//! `tests/retry_hint_log.rs` と同じ最小の HTTP/1.1 スタブをループバックに立て、400 と長い日本語の本文を返す。

use fuseforks_core::llm::{ChatMessage, ChatRequest, LlmBackend, LlmError};
use fuseforks_core::model::{CredentialSource, ModelTemplate};
use fuseforks_core::{HttpLlmBackend, InMemorySecretStore, LlmConfig, Provider};

/// コア側の上限（`llm/client.rs` の `MAX_ERROR_BODY`）。非公開の定数なので値を写す。
const MAX_ERROR_BODY: usize = 2_000;

const PREFIX: &str = r#"{"error":{"message":""#;

/// 2,000 バイト目が「あ」（3 バイト）の途中に来る本文。
fn long_japanese_body() -> String {
    format!("{PREFIX}{}\"}}}}", "あ".repeat(1_000))
}

async fn spawn_stub(payload: String) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let payload = payload.clone();
            tokio::spawn(async move {
                let mut raw = Vec::new();
                let mut buf = [0u8; 8192];
                loop {
                    let Ok(n) = socket.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&raw);
                    let Some(head_end) = text.find("\r\n\r\n") else {
                        continue;
                    };
                    let len: usize = text[..head_end]
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse().ok())?
                        })
                        .unwrap_or(0);
                    if raw.len() >= head_end + 4 + len {
                        break;
                    }
                }
                let response = format!(
                    "HTTP/1.1 400 Bad Request\r\ncontent-type: application/json\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });

    format!("http://127.0.0.1:{port}/v1")
}

#[tokio::test]
async fn a_long_multibyte_error_body_is_cut_on_a_char_boundary() {
    let payload = long_japanese_body();
    // 前提: 本文は上限より長く、上限の位置は文字の途中（ここが崩れるとテストが何も確かめなくなる）。
    assert!(payload.len() > MAX_ERROR_BODY);
    assert!(!payload.is_char_boundary(MAX_ERROR_BODY));

    let base_url = spawn_stub(payload.clone()).await;
    let mut template = ModelTemplate::new("tpl_stub", "スタブ", "stub-model");
    template.base_url = base_url;
    template.credential = CredentialSource::NotRequired;
    template.provider = Some(Provider::OpenAiCompat);
    template.max_retries = 1;
    let config = LlmConfig::from_template(&template, &InMemorySecretStore::new()).unwrap();
    let backend = HttpLlmBackend::new(config).unwrap();

    let err = backend
        .chat(ChatRequest::plain("stub-model", vec![ChatMessage::user("ping")], 16))
        .await
        .expect_err("400 は失敗として返るはず");

    let LlmError::Api { status, body, .. } = err else {
        panic!("Api のはず: {err:?}");
    };
    assert_eq!(status, 400);
    // 上限を超えず、かつ上限の直前の文字の境界まで残す（切りすぎない）。
    assert!(body.len() <= MAX_ERROR_BODY, "上限を超えた: {} バイト", body.len());
    assert!(body.len() > MAX_ERROR_BODY - 3, "切りすぎ: {} バイト", body.len());
    assert!(payload.starts_with(&body), "本文の先頭がそのまま残っていない");
}
