//! `ask_external_outcome` — 扉と同じ 1 通を送り、答えと配送の結末を返す（Spec 64 D10）。
//!
//! `ask_external` はこれを呼んで結末を捨てる 1 行になった。扉側の挙動は
//! `external_ask.rs` が留めたまま（緑のまま）で、ここで見るのは**結末が型で届くこと**。

use std::path::PathBuf;
use std::sync::Arc;

use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::plan::PlanTaskState;
use fuseforks_core::{
    ConfigStore, CoreError, FixedBackendFactory, InMemorySecretStore, Orchestrator,
    OrchestratorConfig,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-ask-outcome-{tag}-{}",
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

async fn setup_with_reception(dir: &TempDir) -> (Orchestrator, AgentId) {
    setup_with_backend(dir, FixedBackendFactory::echo("[echo]")).await
}

async fn setup_with_backend(dir: &TempDir, factory: FixedBackendFactory) -> (Orchestrator, AgentId) {
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(factory),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig::default(),
    )
    .await
    .expect("bootstrap できること");
    orchestrator
        .set_language(fuseforks_core::world::Language::Ja)
        .await
        .unwrap();
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();
    let id = AgentId::from("agent_desk");
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "窓口", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();
    orchestrator.set_reception(Some(&id)).await.unwrap();
    (orchestrator, id)
}

/// 答えが返れば `Answered`。本文は `ask_external` と同じ。
#[tokio::test]
async fn an_answered_request_reports_answered() {
    let dir = TempDir::new("answered");
    let (orchestrator, _id) = setup_with_reception(&dir).await;

    let (answer, state) = orchestrator
        .ask_external_outcome("fuseforks-cli", "村の様子を教えて")
        .await
        .expect("窓口が居るので通ること");

    assert_eq!(state, PlanTaskState::Answered);
    assert!(answer.starts_with("[echo] "), "実際: {answer}");
    assert!(
        answer.ends_with("【送り手: fuseforks-cli（外部クライアント）】\n村の様子を教えて"),
        "封筒は ask_external と同じ: {answer}"
    );
}

/// 毎周ツール呼び出しを返し続けるバックエンド（`external_ask.rs` と同じ道具）。
///
/// 提示されていないツール名を呼ぶので実行はされないが、`tool_result` として返るので
/// 周は進む。予算の検査点は周回境界にしか無いので、**1 周で答える echo では天井に
/// 当たらない**（初回の予約の見積もりは `min(床, 天井)` に丸められて 1 周目は通る）。
struct LoopingBackend;

#[async_trait::async_trait]
impl fuseforks_core::llm::LlmBackend for LoopingBackend {
    fn name(&self) -> &str {
        "looping"
    }

    async fn chat(
        &self,
        _req: fuseforks_core::llm::ChatRequest,
    ) -> Result<fuseforks_core::llm::ChatResponse, fuseforks_core::llm::LlmError> {
        Ok(fuseforks_core::llm::ChatResponse {
            text: Some(String::new()),
            tool_calls: vec![fuseforks_core::llm::ToolCall {
                id: "call_1".into(),
                name: "not_presented".into(),
                args: serde_json::json!({}),
                extra: None,
            }],
            finish: fuseforks_core::llm::Finish::ToolUse,
            usage: fuseforks_core::llm::Usage {
                prompt: 1,
                completion: 1,
                cache_read: 0,
                cache_write: 0,
                cache_write_1h: 0,
                reasoning: 0,
            },
            grounding: fuseforks_core::llm::Grounding::default(),
            reasoning_summary: Vec::new(),
        })
    }
}

/// 天井で止まれば `BudgetExhausted`（`fuseforks-cli ask` の終了コード 9 の材料）。
///
/// **`Answered` 以外の結末が型で届くことの対照。** 上の 1 本だけだと、結末を
/// 常に `Answered` で返す実装でも緑になる。天井 5 に対し 1 周目の払いは実効 5
/// （未キャッシュ 1 + 出力 1 × 4）でちょうど尽き、2 周目の予約で止まる — 決定的。
/// 本文（定型文）は扉と同じく文字列で返る。
#[tokio::test]
async fn a_request_stopped_by_the_ceiling_reports_budget_exhausted() {
    let dir = TempDir::new("budget");
    let (orchestrator, _id) =
        setup_with_backend(&dir, FixedBackendFactory::new(Arc::new(LoopingBackend))).await;
    orchestrator.set_token_budget(Some(5)).await.unwrap();

    let (answer, state) = orchestrator
        .ask_external_outcome("fuseforks-cli", "調べて")
        .await
        .expect("扉自体は開く（結末は型で返る）");

    assert_eq!(state, PlanTaskState::BudgetExhausted, "本文: {answer}");
    assert!(!answer.is_empty(), "6〜9 でも定型文は返る");
}

/// 扉と同じ拒否（窓口が止まっている）は、結末ではなくエラーで返る — 配送に入る前の失敗。
#[tokio::test]
async fn a_stopped_reception_is_an_error_not_an_outcome() {
    let dir = TempDir::new("stopped");
    let (orchestrator, id) = setup_with_reception(&dir).await;
    orchestrator.stop_agent(&id).await.unwrap();

    let err = orchestrator
        .ask_external_outcome("fuseforks-cli", "やあ")
        .await
        .expect_err("止まっていれば待たずに断る");
    assert!(matches!(err, CoreError::NotRunning { .. }), "{err}");
}
