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

use fuseforks_core::llm::retry::RetryClass;
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
    spawn_stub_with("400 Bad Request", payload).await
}

async fn spawn_stub_with(status: &'static str, payload: String) -> String {
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
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n\
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

/// 分類は**切る前の**本文で読む（Spec 77 #2）。切り詰めは表示のためで、判定に使う本文を短くする
/// 意図は無い — 2,000 バイトを超える本文の後ろにある `code`（Gemini なら `RetryInfo` の待ち時間）が、
/// 切った後に JSON として読めず失われていた。
#[tokio::test]
async fn the_signal_is_read_before_the_body_is_cut() {
    let payload = format!(
        r#"{{"error":{{"message":"{}","type":"invalid_request_error","code":"context_length_exceeded"}}}}"#,
        "x".repeat(2_100)
    );
    assert!(payload.len() > MAX_ERROR_BODY, "前提: 上限より長い");
    assert!(
        payload.find("context_length_exceeded").unwrap() > MAX_ERROR_BODY,
        "前提: code は上限より後ろにある"
    );

    let base_url = spawn_stub(payload).await;
    let mut template = ModelTemplate::new("tpl_stub", "スタブ", "stub-model");
    template.base_url = base_url;
    template.credential = CredentialSource::NotRequired;
    template.provider = Some(Provider::OpenAiCompat);
    template.max_retries = 0;
    let config = LlmConfig::from_template(&template, &InMemorySecretStore::new()).unwrap();
    let backend = HttpLlmBackend::new(config).unwrap();

    let err = backend
        .chat(ChatRequest::plain("stub-model", vec![ChatMessage::user("ping")], 16))
        .await
        .expect_err("400 は失敗として返るはず");

    let LlmError::Api { class, code, body, .. } = err else {
        panic!("Api のはず: {err:?}");
    };
    assert_eq!(class, RetryClass::ContextLength, "上限より後ろの code で分類する");
    assert_eq!(code.as_deref(), Some("context_length_exceeded"));
    assert!(body.len() <= MAX_ERROR_BODY, "画面とログへ運ぶ本文は今までどおり切る");
}

/// Gemini の 429 は `RetryInfo` が `details[]` の**最後**に来るので、本文が長いと最初に失われるのが
/// 待ち時間だった（Spec 77 #2）。
#[tokio::test]
async fn a_long_gemini_429_keeps_its_retry_delay() {
    let payload = format!(
        r#"{{"error":{{"code":429,"message":"{}","status":"RESOURCE_EXHAUSTED","details":[{{"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"7s"}}]}}}}"#,
        "q".repeat(2_100)
    );
    let base_url = spawn_stub_with("429 Too Many Requests", payload).await;
    let mut template = ModelTemplate::new("tpl_stub", "スタブ", "gemini-stub");
    template.base_url = base_url;
    template.credential = CredentialSource::NotRequired;
    template.provider = Some(Provider::Gemini);
    template.max_retries = 0;
    let config = LlmConfig::from_template(&template, &InMemorySecretStore::new()).unwrap();
    let backend = HttpLlmBackend::new(config).unwrap();

    let err = backend
        .chat(ChatRequest::plain("gemini-stub", vec![ChatMessage::user("ping")], 16))
        .await
        .expect_err("429 は失敗として返るはず");

    let LlmError::Api { retry_after, .. } = err else {
        panic!("Api のはず: {err:?}");
    };
    assert_eq!(retry_after, Some(std::time::Duration::from_secs(7)));
}
