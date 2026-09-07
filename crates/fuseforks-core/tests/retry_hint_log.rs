//! 再試行の計器 `llm retry:`（Spec 52 P0）を、HTTP 経路ごと踏む。
//!
//! # なぜスタブを立てるか
//!
//! `Retry-After` はヘッダなので、純関数（`retry::parse_retry_after`）の単体では
//! 「実際の応答から拾えているか」に届かない。`tests/attachment_fallback.rs` と同じ
//! 最小の HTTP/1.1 スタブをループバックに立て、429 に `Retry-After` を付けて返す。
//!
//! # なぜ 1 本のテストか
//!
//! 診断ログの宛先はプロセスで 1 つ（`OnceLock`）なので、`tests/grep_include_log.rs` と
//! 同じくこのバイナリはテストを 1 本しか持たない。3 つの場面を順に踏み、最後に
//! ログを 1 回読む。
//!
//! P0 の縮退版なので `class=` / `code=` / `src=body` は出ない（それらは P1）。
//! ここで留めるのは **(a) ヘッダが `retry_after` と `src=header` に写ること**、
//! **(b) ヘッダが無ければ `hint=- src=-` になること**、**(c) 成功したターンで
//! この行が 1 本も出ないこと**（負の対照）の 3 つ。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fuseforks_core::llm::retry::HintSource;
use fuseforks_core::llm::{ChatMessage, ChatRequest, LlmBackend, LlmError};
use fuseforks_core::model::{CredentialSource, ModelTemplate};
use fuseforks_core::{HttpLlmBackend, InMemorySecretStore, LlmConfig, Provider};

/// スタブが返す応答の決め方。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Policy {
    /// 常に 429 + `Retry-After: 7`。
    RateLimitedWithHeader,
    /// 常に 429、ヘッダ無し。
    RateLimitedNoHeader,
    /// 常に 200。
    Ok,
}

type Hits = Arc<Mutex<usize>>;

async fn spawn_stub(policy: Policy) -> (String, Hits) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits: Hits = Arc::new(Mutex::new(0));
    let counter = Arc::clone(&hits);

    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let counter = Arc::clone(&counter);
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
                *counter.lock().unwrap() += 1;

                let (status, extra, payload) = match policy {
                    Policy::RateLimitedWithHeader => (
                        "429 Too Many Requests",
                        "retry-after: 7\r\n",
                        r#"{"error":{"message":"slow down"}}"#,
                    ),
                    Policy::RateLimitedNoHeader => (
                        "429 Too Many Requests",
                        "",
                        r#"{"error":{"message":"slow down"}}"#,
                    ),
                    Policy::Ok => (
                        "200 OK",
                        "",
                        r#"{"choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#,
                    ),
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n{extra}\
                     content-length: {}\r\nconnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });

    (format!("http://127.0.0.1:{port}/v1"), hits)
}

/// 再試行 2 回（通算 2 回試行）のバックエンド。待ちは 0 回目の後の 200 ms だけ。
fn backend(base_url: &str) -> HttpLlmBackend {
    let mut template = ModelTemplate::new("tpl_stub", "スタブ", "stub-model");
    template.base_url = base_url.to_owned();
    template.credential = CredentialSource::NotRequired;
    template.provider = Some(Provider::OpenAiCompat);
    template.max_retries = 2;
    let config = LlmConfig::from_template(&template, &InMemorySecretStore::new()).unwrap();
    HttpLlmBackend::new(config).unwrap()
}

fn request() -> ChatRequest {
    ChatRequest::plain("stub-model", vec![ChatMessage::user("ping")], 16)
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir()
            .join(format!("fuseforks-retry-hint-log-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn a_retry_leaves_one_line_carrying_the_header_hint() {
    let dir = TempDir::new();
    let log = dir.0.join("fuseforks.log");
    fuseforks_core::open_log(&log).expect("開けること");

    // (a) ヘッダつきの 429 — 2 回送って落ち、エラーがヘッダの値を運ぶ。
    let (url, hits) = spawn_stub(Policy::RateLimitedWithHeader).await;
    let err = backend(&url).chat(request()).await.expect_err("429 で落ちること");
    assert_eq!(*hits.lock().unwrap(), 2, "通算 2 回試行");
    match err {
        LlmError::Api {
            status,
            retry_after,
            hint_src,
            ..
        } => {
            assert_eq!(status, 429);
            assert_eq!(retry_after, Some(Duration::from_secs(7)), "ヘッダの 7 秒");
            assert_eq!(hint_src, HintSource::Header);
        }
        other => panic!("Api のはず: {other:?}"),
    }

    // (b) ヘッダ無しの 429 — 同じく 2 回送るが、hint は無い。
    let (url, hits) = spawn_stub(Policy::RateLimitedNoHeader).await;
    let err = backend(&url).chat(request()).await.expect_err("429 で落ちること");
    assert_eq!(*hits.lock().unwrap(), 2, "通算 2 回試行");
    match err {
        LlmError::Api {
            retry_after,
            hint_src,
            ..
        } => {
            assert_eq!(retry_after, None);
            assert_eq!(hint_src, HintSource::None);
        }
        other => panic!("Api のはず: {other:?}"),
    }

    // (c) 成功 — 1 回で返り、計器は増えない（負の対照）。
    let (url, hits) = spawn_stub(Policy::Ok).await;
    backend(&url).chat(request()).await.expect("通ること");
    assert_eq!(*hits.lock().unwrap(), 1);

    let body = std::fs::read_to_string(&log).expect("読めること");
    let lines: Vec<&str> = body
        .lines()
        .filter(|line| line.contains("llm retry:"))
        .collect();
    assert_eq!(lines.len(), 2, "再試行は 2 回だけ（成功では出ない）:\n{body}");
    assert!(
        lines[0].contains(
            "llm retry: model=stub-model attempt=1/2 status=429 hint=7s src=header wait=200ms"
        ),
        "ヘッダの値が hint= と src= に写る: {}",
        lines[0]
    );
    assert!(
        lines[1].contains(
            "llm retry: model=stub-model attempt=1/2 status=429 hint=- src=- wait=200ms"
        ),
        "ヘッダが無ければ hint=- src=-: {}",
        lines[1]
    );
}
