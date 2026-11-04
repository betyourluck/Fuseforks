//! 简体中文の村はモデルへ英語で話す（Spec 74 D1・P1）。
//!
//! 画面の言語（`DisplayLanguage`）とコアの声（`Language`）を型で分けた。この結合テストは、
//! 世界に `ZhHans` を保存した村が実際に送る要求（システムプロンプト・履歴・ツールの提示）が、
//! **英語の村とバイト等価**であることを確かめる。比べるのは `ChatRequest` 丸ごと。
//!
//! 負の対照として日本語の村も同じ形で走らせ、要求が英語の村と**違う**ことを確かめる —
//! 言語で何も変わらない実装でも「中文 = 英語」は緑になってしまうので、比較が言語を
//! 見分けられることを先に示す。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fuseforks_core::event::CoreEvent;
use fuseforks_core::llm::{ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, Usage};
use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::world::DisplayLanguage;
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};
use tokio::sync::broadcast::Receiver;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-display-voice-{tag}-{}",
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

/// 受け取った要求をそのまま記録して答える。
struct Recording {
    seen: Mutex<Vec<ChatRequest>>,
}

#[async_trait::async_trait]
impl LlmBackend for Recording {
    fn name(&self) -> &str {
        "recording"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.seen.lock().unwrap().push(req);
        Ok(ChatResponse {
            text: Some("ok".into()),
            tool_calls: Vec::new(),
            finish: Finish::Stop,
            usage: Usage { prompt: 1, completion: 1, cache_read: 0, cache_write: 0, cache_write_1h: 0, reasoning: 0 },
            grounding: Default::default(),
            reasoning_summary: Vec::new(),
            replay: None,
        })
    }
}

/// 静穏窓は統計の周期（1 秒）より**短く**取る（#86 — 長いと永久に返らない）。
async fn drain_until_quiet(rx: &mut Receiver<CoreEvent>, quiet: Duration) {
    while let Ok(Ok(_)) = tokio::time::timeout(quiet, rx.recv()).await {}
}

/// `language` の村で 1 ターン走らせ、モデルへ送った最初の要求を返す。
async fn first_request(language: DisplayLanguage, tag: &str) -> ChatRequest {
    let dir = TempDir::new(tag);
    let backend = Arc::new(Recording { seen: Mutex::new(Vec::new()) });
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
    assert_eq!(orchestrator.display_language().await, language, "表示の言語は保存した値のまま");
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "default", "mock-model"))
        .await
        .unwrap();
    let id = AgentId::from("agent_01");
    orchestrator.create_agent(AgentSpec::new(id.clone(), "Bob", "tpl")).await.unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "summarize the README").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let seen = backend.seen.lock().unwrap();
    seen.first().expect("要求が 1 本は記録されること").clone()
}

#[tokio::test]
async fn a_simplified_chinese_village_sends_the_english_request_byte_for_byte() {
    let zh = first_request(DisplayLanguage::ZhHans, "zh").await;
    let en = first_request(DisplayLanguage::En, "en").await;
    let ja = first_request(DisplayLanguage::Ja, "ja").await;

    // 負の対照: 比較が言語を見分けられること（日本語と英語の要求は違う）。
    assert_ne!(ja, en, "日本語の村と英語の村の要求は違うはず — 違わなければこの比較は何も見ていない");

    assert_eq!(zh, en, "简体中文の村の要求（システムプロンプト・履歴・ツール）は英語の村と同じ");
}
