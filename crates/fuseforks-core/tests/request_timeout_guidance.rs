//! 接続先がテンプレートの「タイムアウト(秒)」以内に応答しなかったとき、その秒数を `ErrorPayload.timeoutSecs` で
//! 運ぶ（2026-11-21）。
//!
//! # 何が起きていたか
//!
//! `gemini-4-hydrogen` の 1 回の呼び出しが 120 秒（テンプレートの既定）を 3 回続けて超え、ターンが
//! `LLM_HTTP` で止まった。画面に出たのは reqwest の原文「error sending request for url (…): operation timed
//! out」だけで、**どの設定を延ばせば通るのかが分からなかった**。待ち時間はテンプレートの欄で、利用者が
//! モデル登録で変えられる。
//!
//! # なぜ LLM の層で秒数を付けるか
//!
//! `LlmError::Http` は reqwest のエラーを包むだけで、何秒で切ったかを知らない。知っているのは
//! `HttpLlmBackend`（`LlmConfig::request_timeout`）だけなので、送信の失敗を返す地点で秒数を付ける。
//! **文にするのは画面**（`formatError` が表示の言語で添える。`retryAfterSecs` と同じ形）。
//!
//! # なぜスタブを立てるか
//!
//! 秒数が付くのは「実際に待ち切れなかった」ときだけで、純関数の単体では reqwest のタイムアウトのエラーを
//! 作れない。要求を受けたまま返事をしないスタブをループバックに立てる。

use std::time::{Duration, Instant};

use fuseforks_core::llm::{ChatMessage, ChatRequest, LlmBackend, LlmError};
use fuseforks_core::model::{CredentialSource, ModelTemplate};
use fuseforks_core::{CoreError, ErrorPayload, HttpLlmBackend, InMemorySecretStore, LlmConfig, Provider};

/// 接続を受けて要求を読み、**返事をしない**スタブ。テストが終わるまで接続を握ったままにする。
async fn spawn_silent_stub() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 8192];
                while let Ok(n) = socket.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                }
            });
        }
    });
    format!("http://127.0.0.1:{port}/v1")
}

fn backend(base_url: String, timeout_secs: u32) -> HttpLlmBackend {
    let mut template = ModelTemplate::new("tpl_stub", "スタブ", "stub-model");
    template.base_url = base_url;
    template.credential = CredentialSource::NotRequired;
    template.provider = Some(Provider::OpenAiCompat);
    template.request_timeout_secs = timeout_secs;
    // 1 回だけ撃つ（タイムアウトは一過性なので、2 以上だと待ちが試行の数だけ伸びる）。
    template.max_retries = 1;
    let config = LlmConfig::from_template(&template, &InMemorySecretStore::new()).unwrap();
    HttpLlmBackend::new(config).unwrap()
}

fn request() -> ChatRequest {
    ChatRequest::plain("stub-model", vec![ChatMessage::user("ping")], 16)
}

#[tokio::test]
async fn a_request_that_outlives_the_template_timeout_carries_the_seconds() {
    let backend = backend(spawn_silent_stub().await, 1);

    let started = Instant::now();
    let err = backend.chat(request()).await.expect_err("返事をしないスタブは待ち切れずに失敗する");
    assert!(started.elapsed() >= Duration::from_secs(1), "前提: 1 秒待ってから切れている");
    assert!(
        matches!(&err, LlmError::Http { source, .. } if source.is_timeout()),
        "前提: reqwest のタイムアウトとして返る: {err:?}"
    );

    let payload = ErrorPayload::from(&CoreError::Llm(err));
    let json = serde_json::to_value(&payload).unwrap();
    assert_eq!(json["timeoutSecs"], 1, "待ち切れなかった秒数を運ぶ: {json}");
    // 文は運ばない（コアは言語を知らない）。本文は reqwest の原文のまま。
    assert_eq!(json["code"], "LLM_HTTP");
    assert!(payload.message.contains("operation timed out"), "原文が残る: {}", payload.message);
}

/// 接続先の再試行を挟んだ失敗も、最後に返るのは同じ形（`chat_with_backoff` の最後の試行）。
#[tokio::test]
async fn the_seconds_survive_the_retry_loop() {
    let mut template = ModelTemplate::new("tpl_stub", "スタブ", "stub-model");
    template.base_url = spawn_silent_stub().await;
    template.credential = CredentialSource::NotRequired;
    template.provider = Some(Provider::OpenAiCompat);
    template.request_timeout_secs = 1;
    template.max_retries = 2;
    let config = LlmConfig::from_template(&template, &InMemorySecretStore::new()).unwrap();
    let backend = HttpLlmBackend::new(config).unwrap();

    let err = backend.chat(request()).await.expect_err("2 回とも待ち切れない");
    let json = serde_json::to_value(ErrorPayload::from(&CoreError::Llm(err))).unwrap();
    assert_eq!(json["timeoutSecs"], 1, "{json}");
}

/// 接続を断られた失敗はタイムアウトではない — 秒数を付けない（付けると「延ばせば通る」と誤った手を指す）。
#[tokio::test]
async fn a_refused_connection_carries_no_seconds() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let backend = backend(format!("http://127.0.0.1:{port}/v1"), 30);

    let err = backend.chat(request()).await.expect_err("閉じたポートへは繋がらない");
    assert!(
        matches!(&err, LlmError::Http { source, .. } if !source.is_timeout()),
        "前提: タイムアウトではない HTTP の失敗: {err:?}"
    );
    let json = serde_json::to_value(ErrorPayload::from(&CoreError::Llm(err))).unwrap();
    assert!(json["timeoutSecs"].is_null(), "{json}");
}
