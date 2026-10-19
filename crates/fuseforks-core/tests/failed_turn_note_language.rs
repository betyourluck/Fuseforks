//! 失敗したターンの注記を、村の言語で書く（2026-10-19）。
//!
//! # 何が起きていたか
//!
//! 失敗したターンは、依頼と「失敗した」という注記の往復として履歴に積む（`record_failed_turn`。
//! 往復の対を崩さないため）。この注記は**日本語固定**で、英語の村でもモデルは日本語の文を読んでいた。
//! さらに、接続先が 60 秒を超える待ちを求めて再試行をやめたとき、LLM の層が日本語固定の
//! 「プロバイダは N 秒後の再試行を求めています」を**エラーの本文へ**書き足しており、それが注記の中へ
//! そのまま入っていた。秒数は `LlmError::Api.retry_after` の構造で運び、文にするのは村の言語を
//! 知っている注記の側（ここ）と画面の側（`formatError`）へ移した。
//!
//! 負の対照として日本語の村も同じ形で確かめる — 日本語の村の注記は、秒数の文が本文から
//! 注記へ移っただけで、従来どおり日本語で残る。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fuseforks_core::event::CoreEvent;
use fuseforks_core::llm::retry::{HintSource, RetryClass};
use fuseforks_core::llm::{ChatMessage, ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, Usage};
use fuseforks_core::model::{AgentId, AgentSpec, AgentStatus, ModelTemplate};
use fuseforks_core::world::Language;
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};
use tokio::sync::broadcast::Receiver;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-failed-note-{tag}-{}",
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

/// 接続先が 3600 秒後の再試行を求めて止まった形（`chat_with_backoff` が返すのと同じ）。
fn hint_too_long() -> LlmError {
    LlmError::Api {
        status: 429,
        body: r#"{"error":{"message":"slow down"}}"#.to_owned(),
        class: RetryClass::RateLimit,
        code: None,
        retry_after: Some(Duration::from_secs(3600)),
        hint_src: HintSource::Header,
    }
}

/// この接続先が画像を受け付けなかった形（JPEG フォールバックが返すのと同じ。2026-10-20）。
fn images_rejected() -> LlmError {
    LlmError::ImagesRejected {
        body: r#"{"error":{"message":"unsupported image format"}}"#.to_owned(),
    }
}

/// 1 回目だけ `first` で失敗し、以後は受け取ったメッセージを記録して答える。
struct FailFirstThenRecording {
    first: fn() -> LlmError,
    calls: Mutex<usize>,
    seen: Mutex<Vec<Vec<ChatMessage>>>,
}

#[async_trait::async_trait]
impl LlmBackend for FailFirstThenRecording {
    fn name(&self) -> &str {
        "fail-first"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        {
            let mut calls = self.calls.lock().unwrap();
            *calls += 1;
            if *calls == 1 {
                return Err((self.first)());
            }
        }
        self.seen.lock().unwrap().push(req.messages.clone());
        Ok(ChatResponse {
            text: Some("ok".into()),
            tool_calls: Vec::new(),
            finish: Finish::Stop,
            usage: Usage { prompt: 1, completion: 1, cache_read: 0, cache_write: 0, cache_write_1h: 0, reasoning: 0 },
            grounding: Default::default(),
            reasoning_summary: Vec::new(),
        })
    }
}

/// 静穏窓は統計の周期（1 秒）より**短く**取る（#86 — 長いと永久に返らない）。
async fn drain_until_quiet(rx: &mut Receiver<CoreEvent>, quiet: Duration) {
    while let Ok(Ok(_)) = tokio::time::timeout(quiet, rx.recv()).await {}
}

/// 1 ターン目を `first` で失敗させ、2 ターン目のプロンプトの本文を全部つないで返す。
async fn prompt_after_a_failed_turn(language: Language, tag: &str, first: fn() -> LlmError) -> String {
    let dir = TempDir::new(tag);
    let backend = Arc::new(FailFirstThenRecording {
        first,
        calls: Mutex::new(0),
        seen: Mutex::new(Vec::new()),
    });
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(backend.clone())),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig {
            schedule_interval: Duration::from_secs(3600),
            ..OrchestratorConfig::default()
        },
    )
    .await
    .expect("bootstrap できること");
    orchestrator.set_language(language).await.unwrap();
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "default", "mock-model"))
        .await
        .unwrap();
    let id = AgentId::from("agent_01");
    orchestrator.create_agent(AgentSpec::new(id.clone(), "Bob", "tpl")).await.unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "translate the README").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    // 非一過性の失敗（画像の拒否 = 400）は個体を止める（従来どおり）。注記は止まる前に積まれているので、
    // 起動し直して 2 ターン目を送る（起動は履歴を消さない — Spec 12 P2）。
    if orchestrator.snapshot(&id).await.unwrap().status != AgentStatus::Running {
        orchestrator.start_agent(&id).await.unwrap();
    }
    orchestrator.send_user_message(&id, "please continue").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let seen = backend.seen.lock().unwrap();
    let latest = seen.last().expect("2 ターン目のリクエストが記録されること");
    latest.iter().map(|m| m.content.clone()).collect::<Vec<_>>().join("\n")
}

#[tokio::test]
async fn an_english_village_gets_an_english_failure_note_with_the_seconds() {
    let joined = prompt_after_a_failed_turn(Language::En, "en", hint_too_long).await;
    assert!(joined.contains("translate the README"), "失敗したターンの依頼は残る: {joined}");
    assert!(
        !joined.contains("このターンは失敗し") && !joined.contains("秒後の再試行"),
        "英語の村の注記に日本語の文を入れない: {joined}"
    );
    assert!(joined.contains("This turn failed"), "注記は英語で書く: {joined}");
    assert!(
        joined.contains("The provider asks to retry after 3600 seconds."),
        "接続先が求めた秒数を注記に書く: {joined}"
    );
}

#[tokio::test]
async fn a_japanese_village_keeps_the_japanese_note_and_the_seconds() {
    let joined = prompt_after_a_failed_turn(Language::Ja, "ja", hint_too_long).await;
    assert!(joined.contains("このターンは失敗し"), "日本語の村は従来どおり日本語: {joined}");
    assert!(
        joined.contains("プロバイダは 3600 秒後の再試行を求めています。"),
        "秒数の文は本文から注記へ移っただけで、日本語の村でも残る: {joined}"
    );
}

/// 画像を受け付けなかった失敗（JPEG フォールバックの結末。2026-10-20）も、案内は村の言語で書く。
#[tokio::test]
async fn an_english_village_gets_the_image_rejection_in_english() {
    let joined = prompt_after_a_failed_turn(Language::En, "img-en", images_rejected).await;
    assert!(
        !joined.contains("画像を受け付けません") && !joined.contains("送り直してください"),
        "英語の村の注記に日本語の案内を入れない: {joined}"
    );
    assert!(
        joined.contains("This endpoint does not accept images (both WebP and JPEG were rejected)."),
        "原因を英語で書く: {joined}"
    );
    assert!(joined.contains("unsupported image format"), "接続先の原文は残す: {joined}");
}

#[tokio::test]
async fn a_japanese_village_keeps_the_image_rejection_in_japanese() {
    let joined = prompt_after_a_failed_turn(Language::Ja, "img-ja", images_rejected).await;
    assert!(
        joined.contains("この接続先は画像を受け付けません（WebP と JPEG の両方が拒否されました）。"),
        "日本語の村では従来と同じ案内: {joined}"
    );
}
