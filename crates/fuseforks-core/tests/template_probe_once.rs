//! 接続テストは**再試行の輪を通らない**（Spec 68 D5・契約 4）を、実物の HTTP 経路で確かめる。
//!
//! ループバックのスタブが 429 + `Retry-After: 120` を返す。テンプレートの `max_retries` は 3 のままで、
//! 輪を通れば 120 秒を待つか（60 秒の天井を超えるので止める）`llm retry stop:` 行を出す。接続テストは
//! `chat_once` で 1 回だけ撃ち、待たず・再送せず・`llm retry` で始まる行を出さず、`retry_after` を
//! 120 秒のまま運ぶ。あわせて計器 `template probe:` の 1 行が出て、接続先の本文を書かないことを見る。
//!
//! 診断ログの宛先はプロセスで 1 つ（`OnceLock`）なので、このバイナリはテストを 1 本しか持たない。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fuseforks_core::llm::Provider;
use fuseforks_core::model::{CredentialSource, ModelTemplate, ModelTemplateId};
use fuseforks_core::template_probe::{ProbeFailure, TemplateProbe};
use fuseforks_core::{ConfigStore, HttpBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig};

const BODY: &str = r#"{"error":{"message":"slow down please"}}"#;

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn spawn_stub() -> (String, Arc<Mutex<usize>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(Mutex::new(0usize));
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
                let response = format!(
                    "HTTP/1.1 429 Too Many Requests\r\ncontent-type: application/json\r\nretry-after: 120\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{BODY}",
                    BODY.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    (format!("http://127.0.0.1:{port}/v1"), hits)
}

#[tokio::test]
async fn the_probe_shoots_once_without_the_retry_loop() {
    let dir = TempDir(std::env::temp_dir().join(format!("fuseforks-template-probe-once-{}", std::process::id())));
    std::fs::create_dir_all(&dir.0).unwrap();
    let log = dir.0.join("fuseforks.log");
    fuseforks_core::diag::open_log(&log).expect("ログを開けること");

    let (url, hits) = spawn_stub().await;
    let secrets = Arc::new(InMemorySecretStore::new());
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(dir.0.join("village")),
        Arc::new(HttpBackendFactory::strict(secrets.clone())),
        secrets,
        OrchestratorConfig { schedule_interval: Duration::from_secs(3600), ..OrchestratorConfig::default() },
    )
    .await
    .unwrap();
    let mut template = ModelTemplate::new("stub", "スタブ", "stub-model");
    template.base_url = url;
    template.provider = Some(Provider::OpenAiCompat);
    template.credential = CredentialSource::NotRequired;
    template.max_retries = 3;
    orchestrator.upsert_template(template).await.unwrap();

    let started = Instant::now();
    let probe = orchestrator.probe_template(&ModelTemplateId::from("stub")).await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(10), "待たずに返る: {:?}", started.elapsed());
    assert_eq!(*hits.lock().unwrap(), 1, "1 回だけ撃つ（再送しない）");

    let TemplateProbe::Failed { failure, status, retry_after_secs, error, .. } = probe else {
        panic!("failed のはず: {probe:?}");
    };
    assert_eq!(failure, ProbeFailure::RateLimit);
    assert_eq!(status, Some(429));
    assert_eq!(retry_after_secs, Some(120), "60 秒を超えても値のまま運ぶ");
    assert!(error.message.contains("slow down please"), "接続先の本文は画面へ運ぶ: {}", error.message);

    let text = std::fs::read_to_string(&log).unwrap();
    assert!(!text.contains("llm retry"), "輪の計器を出さない: {text}");
    let line = text
        .lines()
        .find(|l| l.contains("template probe:"))
        .unwrap_or_else(|| panic!("`template probe:` の行が出ること: {text}"));
    assert!(line.contains("template=stub"), "{line}");
    assert!(line.contains("wire=OpenAiCompat"), "{line}");
    assert!(line.contains("outcome=rate_limit"), "{line}");
    assert!(line.contains("status=429"), "{line}");
    assert!(line.contains("class=rate_limit"), "{line}");
    assert!(line.contains("prompt=-"), "使用量の無い結果は - : {line}");
    assert!(!line.contains("slow down") && !line.contains("127.0.0.1"), "本文と base URL を書かない: {line}");
}
