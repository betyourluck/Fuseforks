//! 再試行の分類・待ち時間・計器 `llm retry:`（Spec 52 P1）を、HTTP 経路ごと踏む。
//!
//! # なぜスタブを立てるか
//!
//! `Retry-After` はヘッダで、`insufficient_quota` と `RetryInfo` は本文なので、
//! 純関数（`retry::classify` / `plan_wait` / adapter の `error_signal`）の単体では
//! 「実際の応答から拾って、実際に待つか・止めるか」に届かない。
//! `tests/attachment_fallback.rs` と同じ最小の HTTP/1.1 スタブをループバックに立てる。
//! **実機で踏めない経路が 2 つある**（課金切れ = 残高のある鍵しか無い / 天井超えの明示値）
//! ので、このテストがその代替（検収 5）。
//!
//! # なぜ 1 本のテストか
//!
//! 診断ログの宛先はプロセスで 1 つ（`OnceLock`）なので、`tests/grep_include_log.rs` と
//! 同じくこのバイナリはテストを 1 本しか持たない。7 つの場面を順に踏み、最後に
//! ログを 1 回読む。**場面の順序がログの行の順序**。
//!
//! 待ちの実測は `Instant` で取る。明示値は 1 秒（テストが 2 分待たないため）。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use fuseforks_core::llm::retry::{HintSource, RetryClass};
use fuseforks_core::llm::{ChatMessage, ChatRequest, LlmBackend, LlmError};
use fuseforks_core::model::{CredentialSource, ModelTemplate};
use fuseforks_core::{HttpLlmBackend, InMemorySecretStore, LlmConfig, Provider};

/// スタブが返す応答の決め方。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Policy {
    /// 常に 429 + `Retry-After: 1`。
    RateLimitedWithHeader,
    /// 常に 429、ヘッダ無し。
    RateLimitedNoHeader,
    /// 常に 200。
    Ok,
    /// 常に 429 + OpenAI 系の課金切れ本文。**変わる挙動 1** — 再試行しない。
    Billing,
    /// 常に 429 + `Retry-After: 3600`。天井超え — 再試行せず本文へ秒数。
    HintTooLong,
    /// 常に 429 + Gemini の `RetryInfo` 本文（`retryDelay: "1s"`）。**変わる挙動 2** —
    /// 本文の明示値が下限になる。
    GeminiRetryInfo,
    /// 常に 529（Anthropic の過負荷の形。本文は素の JSON）。
    Overloaded,
    /// 常に 429 + `Retry-After: 30`。打ち切りで待ちが切れることを見る（D4）。
    RateLimitedThirty,
    /// 600 ms 黙ってから 200。**HTTP 往復は打ち切りで切れない**ことを見る（D4 の境界）。
    SlowOk,
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
                if policy == Policy::SlowOk {
                    tokio::time::sleep(Duration::from_millis(600)).await;
                }

                let slow_down = r#"{"error":{"message":"slow down"}}"#;
                let (status, extra, payload) = match policy {
                    Policy::RateLimitedWithHeader => {
                        ("429 Too Many Requests", "retry-after: 1\r\n", slow_down)
                    }
                    Policy::RateLimitedNoHeader => ("429 Too Many Requests", "", slow_down),
                    Policy::Ok => (
                        "200 OK",
                        "",
                        r#"{"choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#,
                    ),
                    Policy::Billing => (
                        "429 Too Many Requests",
                        "",
                        r#"{"error":{"message":"You exceeded your current quota","type":"insufficient_quota","code":"insufficient_quota"}}"#,
                    ),
                    Policy::HintTooLong => {
                        ("429 Too Many Requests", "retry-after: 3600\r\n", slow_down)
                    }
                    Policy::GeminiRetryInfo => (
                        "429 Too Many Requests",
                        "",
                        r#"{"error":{"code":429,"message":"quota","status":"RESOURCE_EXHAUSTED","details":[{"@type":"type.googleapis.com/google.rpc.QuotaFailure","violations":[]},{"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"1s"}]}}"#,
                    ),
                    Policy::Overloaded => (
                        "529 Overloaded",
                        "",
                        r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
                    ),
                    Policy::RateLimitedThirty => {
                        ("429 Too Many Requests", "retry-after: 30\r\n", slow_down)
                    }
                    Policy::SlowOk => (
                        "200 OK",
                        "",
                        r#"{"choices":[{"message":{"role":"assistant","content":"late"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#,
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

/// 通算 2 回試行のバックエンド（待ちは 0 回目の失敗の後の 1 回だけ）。
fn backend(base_url: &str, provider: Provider) -> HttpLlmBackend {
    let mut template = ModelTemplate::new("tpl_stub", "スタブ", "stub-model");
    template.base_url = base_url.to_owned();
    template.credential = CredentialSource::NotRequired;
    template.provider = Some(provider);
    template.max_retries = 2;
    let config = LlmConfig::from_template(&template, &InMemorySecretStore::new()).unwrap();
    HttpLlmBackend::new(config).unwrap()
}

fn request() -> ChatRequest {
    ChatRequest::plain("stub-model", vec![ChatMessage::user("ping")], 16)
}

/// 失敗を `Api` として開く。
fn api(err: LlmError) -> (u16, String, RetryClass, Option<Duration>, HintSource) {
    match err {
        LlmError::Api {
            status,
            body,
            class,
            retry_after,
            hint_src,
            ..
        } => (status, body, class, retry_after, hint_src),
        other => panic!("Api のはず: {other:?}"),
    }
}

/// ログの `wait=NNNms` を読む。
fn wait_ms(line: &str) -> u64 {
    line.split("wait=")
        .nth(1)
        .and_then(|s| s.strip_suffix("ms"))
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("wait= が読めない: {line}"))
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
async fn retries_wait_for_the_hint_and_stop_on_billing_or_a_too_long_hint() {
    let dir = TempDir::new();
    let log = dir.0.join("fuseforks.log");
    fuseforks_core::open_log(&log).expect("開けること");

    // 1. ヘッダつきの 429 — 2 回送って落ち、**1 秒以上待つ**（明示値が下限）。
    let (url, hits) = spawn_stub(Policy::RateLimitedWithHeader).await;
    let started = Instant::now();
    let err = backend(&url, Provider::OpenAiCompat).chat(request()).await.expect_err("429");
    let elapsed = started.elapsed();
    assert_eq!(*hits.lock().unwrap(), 2, "通算 2 回試行");
    assert!(elapsed >= Duration::from_secs(1), "明示値 1 秒は下限: {elapsed:?}");
    assert!(elapsed < Duration::from_millis(1500), "jitter は最大 10%: {elapsed:?}");
    let (status, _, class, retry_after, src) = api(err);
    assert_eq!((status, class), (429, RetryClass::RateLimit));
    assert_eq!(retry_after, Some(Duration::from_secs(1)));
    assert_eq!(src, HintSource::Header);

    // 2. ヘッダ無しの 429 — 2 回送るが、待ちは指数部の 200 ms だけ。
    let (url, hits) = spawn_stub(Policy::RateLimitedNoHeader).await;
    let started = Instant::now();
    let err = backend(&url, Provider::OpenAiCompat).chat(request()).await.expect_err("429");
    assert!(started.elapsed() < Duration::from_millis(900), "明示値が無ければ短い");
    assert_eq!(*hits.lock().unwrap(), 2);
    let (_, _, _, retry_after, src) = api(err);
    assert_eq!((retry_after, src), (None, HintSource::None));

    // 3. 成功 — 1 回で返り、計器は増えない（負の対照 = 検収 7）。
    let (url, hits) = spawn_stub(Policy::Ok).await;
    backend(&url, Provider::OpenAiCompat).chat(request()).await.expect("通ること");
    assert_eq!(*hits.lock().unwrap(), 1);

    // 4. 課金切れ — **429 でも 1 回で止まる**（変わる挙動 1・S2）。
    let (url, hits) = spawn_stub(Policy::Billing).await;
    let err = backend(&url, Provider::OpenAiCompat).chat(request()).await.expect_err("429");
    assert_eq!(*hits.lock().unwrap(), 1, "再試行しない");
    assert!(!err.is_transient(), "CoreError::is_retryable もこれを読む");
    let (status, _, class, ..) = api(err);
    assert_eq!((status, class), (429, RetryClass::Billing));

    // 5. 天井超えの明示値 — 1 回で止まり、本文の先頭に秒数（S6）。
    let (url, hits) = spawn_stub(Policy::HintTooLong).await;
    let err = backend(&url, Provider::OpenAiCompat).chat(request()).await.expect_err("429");
    assert_eq!(*hits.lock().unwrap(), 1, "天井を超える待ちはしない");
    let (status, body, class, retry_after, _) = api(err);
    assert_eq!((status, class), (429, RetryClass::RateLimit), "分類は元のまま");
    assert_eq!(retry_after, Some(Duration::from_secs(3600)));
    assert!(
        body.starts_with("プロバイダは 3600 秒後の再試行を求めています。"),
        "本文の先頭に秒数: {body}"
    );
    assert!(body.contains("slow down"), "元の本文は残す: {body}");

    // 6. Gemini の本文 `RetryInfo` — **ヘッダ無しでも 1 秒待つ**（変わる挙動 2・S1・検収 1）。
    let (url, hits) = spawn_stub(Policy::GeminiRetryInfo).await;
    let started = Instant::now();
    let err = backend(&url, Provider::Gemini).chat(request()).await.expect_err("429");
    let elapsed = started.elapsed();
    assert_eq!(*hits.lock().unwrap(), 2);
    assert!(elapsed >= Duration::from_secs(1), "本文の明示値が下限: {elapsed:?}");
    let (_, _, class, retry_after, src) = api(err);
    assert_eq!(class, RetryClass::RateLimit, "RESOURCE_EXHAUSTED は RateLimit");
    assert_eq!(retry_after, Some(Duration::from_secs(1)));
    assert_eq!(src, HintSource::Body);

    // 7. 529 — 今までどおり再送する（S3）。
    let (url, hits) = spawn_stub(Policy::Overloaded).await;
    let err = backend(&url, Provider::Anthropic).chat(request()).await.expect_err("529");
    assert_eq!(*hits.lock().unwrap(), 2);
    let (status, _, class, ..) = api(err);
    assert_eq!((status, class), (529, RetryClass::Overloaded));

    // 8. 30 秒の明示値の待ちの最中に打ち切り — **待ちが切れて即返る**（D4・S4・検収 4）。
    //    返るのは受けた 429 そのもの（新しい variant は無い）。送ったのは 1 回。
    let (url, hits) = spawn_stub(Policy::RateLimitedThirty).await;
    let token = CancellationToken::new();
    let canceller = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        canceller.cancel();
    });
    let started = Instant::now();
    let err = backend(&url, Provider::OpenAiCompat)
        .chat_cancellable(request(), Some(token))
        .await
        .expect_err("429");
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_secs(2), "30 秒待たずに返る: {elapsed:?}");
    assert_eq!(*hits.lock().unwrap(), 1, "再送していない");
    let (status, _, class, retry_after, _) = api(err);
    assert_eq!((status, class), (429, RetryClass::RateLimit));
    assert_eq!(retry_after, Some(Duration::from_secs(30)));

    // 9. HTTP 往復の最中に打ち切り — **切れない**（D4 の境界。払いの記録を失わないため）。
    //    600 ms 黙るスタブへ 100 ms で cancel しても、応答は届く。
    let (url, hits) = spawn_stub(Policy::SlowOk).await;
    let token = CancellationToken::new();
    let canceller = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        canceller.cancel();
    });
    let started = Instant::now();
    let response = backend(&url, Provider::OpenAiCompat)
        .chat_cancellable(request(), Some(token))
        .await
        .expect("往復は完走する");
    assert!(started.elapsed() >= Duration::from_millis(600), "HTTP は切らない");
    assert_eq!(response.text.as_deref(), Some("late"));
    assert_eq!(*hits.lock().unwrap(), 1);

    // ---- ログ。場面の順序 = 行の順序。
    let text = std::fs::read_to_string(&log).expect("読めること");
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| line.contains("llm retry"))
        .collect();
    assert_eq!(lines.len(), 7, "再送 5 本 + 停止 2 本:\n{text}");

    let expect_prefix = |line: &str, prefix: &str| {
        assert!(line.contains(prefix), "期待 `{prefix}`\n実物 `{line}`");
    };
    expect_prefix(
        lines[0],
        "llm retry: model=stub-model attempt=1/2 status=429 class=rate_limit code=- hint=1s src=header wait=",
    );
    let wait = wait_ms(lines[0]);
    assert!((1000..=1100).contains(&wait), "1 秒 + jitter ≤ 10%: {wait}");
    expect_prefix(
        lines[1],
        "llm retry: model=stub-model attempt=1/2 status=429 class=rate_limit code=- hint=- src=- wait=",
    );
    let wait = wait_ms(lines[1]);
    assert!((200..=220).contains(&wait), "200 ms + jitter ≤ 10%: {wait}");
    expect_prefix(
        lines[2],
        "llm retry stop: model=stub-model attempt=1/2 status=429 class=billing code=insufficient_quota hint=- src=-",
    );
    expect_prefix(
        lines[3],
        "llm retry stop: model=stub-model attempt=1/2 status=429 class=rate_limit code=- hint=3600s src=header reason=hint_too_long",
    );
    expect_prefix(
        lines[4],
        "llm retry: model=stub-model attempt=1/2 status=429 class=rate_limit code=RESOURCE_EXHAUSTED hint=1s src=body wait=",
    );
    expect_prefix(
        lines[5],
        "llm retry: model=stub-model attempt=1/2 status=529 class=overloaded code=overloaded_error hint=- src=- wait=",
    );
    // 打ち切られた待ちも、待ち始めた事実は 1 行残る（切れたことは turn.rs 側の
    // `interrupted` が書く — ここには固有の行を作らない）。
    expect_prefix(
        lines[6],
        "llm retry: model=stub-model attempt=1/2 status=429 class=rate_limit code=- hint=30s src=header wait=",
    );
    let wait = wait_ms(lines[6]);
    assert!((30_000..=33_000).contains(&wait), "30 秒 + jitter ≤ 10%: {wait}");
}
