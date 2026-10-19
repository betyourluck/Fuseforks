//! モデルテンプレートの接続テスト（Spec 68 P1）を、オーケストレーター越しに確かめる。
//!
//! 純機構（判定・分類・ログ行）の単体は `src/template_probe.rs`。ここが見るのは配線 —
//! 送る中身（D3）/ 複製をキャッシュへ入れない（契約 8）/ 退避を成功にしない（契約 8）/ 村の言語 /
//! テンプレートが無いときだけ `Err`（契約 6）。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fuseforks_core::event::CoreEvent;
use fuseforks_core::llm::{
    BackendFactory, BackendResolution, ChatRequest, ChatResponse, Effort, Finish, LlmBackend, LlmError, Role,
    ToolChoice, Usage,
};
use fuseforks_core::model::{AgentId, AgentSpec, CredentialSource, ModelTemplate, ModelTemplateId};
use fuseforks_core::template_probe::{ProbeFailure, Reached, TemplateProbe};
use fuseforks_core::world::Language;
use fuseforks_core::{
    ConfigStore, CoreError, HttpBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};
use tokio::sync::broadcast::Receiver;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-template-probe-{tag}-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
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

fn ok(text: &str) -> Result<ChatResponse, LlmError> {
    Ok(ChatResponse {
        text: Some(text.to_owned()),
        tool_calls: Vec::new(),
        finish: Finish::Stop,
        usage: Usage { prompt: 14, completion: 2, cache_read: 0, cache_write: 0, cache_write_1h: 0, reasoning: 0 },
        grounding: Default::default(),
        reasoning_summary: Vec::new(),
    })
}

/// 決められた順に応答を返し、受けた要求を記録する。
struct Script {
    replies: Mutex<VecDeque<Result<ChatResponse, LlmError>>>,
    requests: Arc<Mutex<Vec<ChatRequest>>>,
}

#[async_trait::async_trait]
impl LlmBackend for Script {
    fn name(&self) -> &str {
        "probe-script"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.requests.lock().unwrap().push(req);
        self.replies.lock().unwrap().pop_front().unwrap_or_else(|| ok("OK"))
    }
}

/// 渡されたテンプレートを記録し、台本のバックエンドを返す。
struct Factory {
    backend: Arc<Script>,
    templates: Arc<Mutex<Vec<ModelTemplate>>>,
}

impl BackendFactory for Factory {
    fn create(&self, template: &ModelTemplate) -> Result<BackendResolution, LlmError> {
        self.templates.lock().unwrap().push(template.clone());
        Ok(BackendResolution::healthy(Arc::clone(&self.backend) as Arc<dyn LlmBackend>))
    }
}

struct Village {
    _dir: TempDir,
    orchestrator: Orchestrator,
    requests: Arc<Mutex<Vec<ChatRequest>>>,
    templates: Arc<Mutex<Vec<ModelTemplate>>>,
}

/// 固有スキルを全部 ON にし、思考段階・最大出力・温度を既定から変えたテンプレート `gen` を 1 つ持つ村。
async fn village(tag: &str, language: Language, replies: Vec<Result<ChatResponse, LlmError>>) -> Village {
    let dir = TempDir::new(tag);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let templates = Arc::new(Mutex::new(Vec::new()));
    let backend = Arc::new(Script { replies: Mutex::new(replies.into()), requests: Arc::clone(&requests) });
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(Factory { backend, templates: Arc::clone(&templates) }),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig { schedule_interval: Duration::from_secs(3600), ..OrchestratorConfig::default() },
    )
    .await
    .expect("bootstrap");
    orchestrator.set_language(language).await.unwrap();
    let mut generator = ModelTemplate::new("gen", "生成役", "gen-model");
    generator.google_search = true;
    generator.openai_web_search = true;
    generator.openai_reasoning_pro = true;
    generator.effort = Some(Effort::High);
    generator.max_output_tokens = 1234;
    generator.temperature = Some(0.3);
    orchestrator.upsert_template(generator).await.unwrap();
    Village { _dir: dir, orchestrator, requests, templates }
}

fn gen_id() -> ModelTemplateId {
    ModelTemplateId::from("gen")
}

async fn drain_until_quiet(rx: &mut Receiver<CoreEvent>, quiet: Duration) {
    while let Ok(Ok(_)) = tokio::time::timeout(quiet, rx.recv()).await {}
}

#[tokio::test]
async fn the_probe_sends_one_plain_message_with_the_templates_model_settings() {
    let v = village("d3", Language::Ja, vec![ok("OK")]).await;
    let probe = v.orchestrator.probe_template(&gen_id()).await.expect("確認を始められること");

    let TemplateProbe::Reached { reached: Reached::Replied { text, usage }, .. } = probe else {
        panic!("replied のはず: {probe:?}");
    };
    assert_eq!(text, "OK");
    assert_eq!((usage.prompt, usage.completion), (14, 2), "払いを画面へ運ぶ");

    let templates = v.templates.lock().unwrap();
    let built = templates.last().expect("ファクトリへ渡ること");
    assert!(
        !built.google_search && !built.openai_web_search && !built.openai_reasoning_pro,
        "固有スキルを外した複製で組む"
    );
    let requests = v.requests.lock().unwrap();
    assert_eq!(requests.len(), 1, "1 回だけ呼ぶ");
    let request = &requests[0];
    assert!(request.tools.is_empty(), "ツールを送らない（ダミーの関数も添えない）");
    assert!(matches!(request.tool_choice, ToolChoice::None));
    assert_eq!(request.effort, Some(Effort::High), "思考段階はテンプレートのまま");
    assert_eq!(request.max_tokens, 1234, "最大出力はテンプレートのまま");
    assert_eq!(request.temperature, Some(0.3), "温度はテンプレートのまま");
    assert_eq!(request.cacheable_prefix_len, 0);
    assert_eq!(request.messages.len(), 1, "システムプロンプトも履歴も送らない");
    assert_eq!(request.messages[0].role, Role::User);
    assert!(request.messages[0].content.contains("接続の確認"), "村の言語の定型文");
}

#[tokio::test]
async fn an_english_village_sends_the_english_prompt() {
    let v = village("en", Language::En, vec![ok("OK")]).await;
    v.orchestrator.probe_template(&gen_id()).await.unwrap();
    let requests = v.requests.lock().unwrap();
    assert!(requests[0].messages[0].content.starts_with("This is a connection check"));
}

#[tokio::test]
async fn the_stripped_copy_never_reaches_the_villages_backend_cache() {
    let v = village("cache", Language::Ja, vec![ok("OK"), ok("はい")]).await;
    let id = AgentId::from("agent_01");
    v.orchestrator.create_agent(AgentSpec::new(id.clone(), "ザリ", "gen")).await.unwrap();

    v.orchestrator.probe_template(&gen_id()).await.unwrap();

    v.orchestrator.start_agent(&id).await.unwrap();
    let mut rx = v.orchestrator.subscribe();
    v.orchestrator.send_user_message(&id, "やあ").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    // 払いは個体の累計に入らない（確認は個体の仕事ではない）。ロックを持ったまま await しない。
    let snapshot = v.orchestrator.snapshot(&id).await.unwrap();
    assert_eq!(snapshot.total_tokens, 16, "累計は個体のターンの 1 回分だけ（確認の払いは入らない）");
    assert_eq!(v.requests.lock().unwrap().len(), 2);

    let templates = v.templates.lock().unwrap().clone();
    assert_eq!(templates.len(), 2, "確認と個体でファクトリを別々に呼ぶ（複製をキャッシュへ入れない）");
    assert!(!templates[0].google_search, "確認はスキル無し");
    assert!(templates[1].google_search, "個体のバックエンドはスキルつきのまま");
}

#[tokio::test]
async fn an_llm_failure_comes_back_as_a_result_not_an_error() {
    let v = village(
        "fail",
        Language::Ja,
        vec![Err(LlmError::api(401, r#"{"error":{"message":"invalid x-api-key"}}"#))],
    )
    .await;
    let probe = v.orchestrator.probe_template(&gen_id()).await.expect("LLM の失敗は Err にしない");
    let TemplateProbe::Failed { failure, status, error, .. } = probe else { panic!("failed のはず") };
    assert_eq!(failure, ProbeFailure::Auth);
    assert_eq!(status, Some(401));
    assert!(error.message.contains("invalid x-api-key"), "接続先の本文を運ぶ");
}

#[tokio::test]
async fn a_missing_template_is_the_only_error() {
    let v = village("missing", Language::Ja, Vec::new()).await;
    let err = v.orchestrator.probe_template(&ModelTemplateId::from("nope")).await.unwrap_err();
    assert!(matches!(err, CoreError::ModelTemplateNotFound(_)), "{err:?}");
    assert!(v.requests.lock().unwrap().is_empty(), "外へ出ない");
}

/// GUI と同じ退避型のファクトリでは、鍵の無いテンプレートはエコー応答へ退避する。
/// **確認がそれを「成功」と読まないこと**（契約 8）。
#[tokio::test]
async fn a_degraded_backend_is_config_and_never_an_echo_success() {
    let dir = TempDir::new("degraded");
    let secrets = Arc::new(InMemorySecretStore::new());
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(HttpBackendFactory::echo_on_failure(secrets.clone())),
        secrets,
        OrchestratorConfig { schedule_interval: Duration::from_secs(3600), ..OrchestratorConfig::default() },
    )
    .await
    .unwrap();
    // 鍵が未設定のテンプレート（新規作成の既定。GUI では「保存」だけ押して鍵を登録していない形）。
    let mut template = ModelTemplate::new("nokey", "鍵なし", "m");
    template.credential = CredentialSource::Unset;
    orchestrator.upsert_template(template).await.unwrap();

    let probe = orchestrator.probe_template(&ModelTemplateId::from("nokey")).await.unwrap();
    let TemplateProbe::Failed { failure, error, .. } = probe else {
        panic!("エコー応答で成功と出さない: {probe:?}");
    };
    assert_eq!(failure, ProbeFailure::Config);
    assert_eq!(error.code, "LLM_CONFIG");
    assert_eq!(error.message.matches("LLM 設定エラー: ").count(), 1, "接頭辞を二重にしない: {}", error.message);
}
