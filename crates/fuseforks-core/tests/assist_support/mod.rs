//! AI 下書き補助の結合テストの土台（`assist_draft.rs` と `assist_log.rs` が共有する）。
#![allow(dead_code)]

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fuseforks_core::assist::SUBMIT_DRAFT;
use fuseforks_core::llm::{
    BackendFactory, BackendResolution, ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, ToolCall, Usage,
};
use fuseforks_core::model::{AgentId, AgentSpec, JudgeSpec, ModelTemplate, UnknownFields};
use fuseforks_core::{ConfigStore, InMemorySecretStore, Orchestrator, OrchestratorConfig};

pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-assist-{tag}-{}",
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

pub fn usage() -> Usage {
    Usage { prompt: 1_000, completion: 100, cache_read: 200, cache_write: 0, cache_write_1h: 0, reasoning: 10 }
}

fn response(text: Option<&str>, calls: Vec<ToolCall>) -> ChatResponse {
    ChatResponse {
        text: text.map(str::to_owned),
        tool_calls: calls,
        finish: Finish::Stop,
        usage: usage(),
        grounding: Default::default(),
        reasoning_summary: Vec::new(),
    }
}

/// 質問だけを返す応答。
pub fn question(text: &str) -> Result<ChatResponse, LlmError> {
    Ok(response(Some(text), Vec::new()))
}

/// `submit_draft` を呼ぶ応答。`extra` は思考署名の代わり（逐語往復を見るため）。
pub fn draft(id: &str, content: &str) -> Result<ChatResponse, LlmError> {
    Ok(response(
        None,
        vec![ToolCall {
            id: id.into(),
            name: SUBMIT_DRAFT.into(),
            args: serde_json::json!({ "content": content, "notes": "仮定: 朝 7 時" }),
            extra: Some(serde_json::json!({ "signature": format!("sig-{id}") })),
        }],
    ))
}

/// 本文も呼び出しも無い応答。
pub fn empty() -> Result<ChatResponse, LlmError> {
    Ok(response(None, Vec::new()))
}

/// 決められた順に応答を返し、受けた要求をすべて記録する。
pub struct Script {
    pub replies: Mutex<VecDeque<Result<ChatResponse, LlmError>>>,
    pub requests: Arc<Mutex<Vec<ChatRequest>>>,
}

#[async_trait::async_trait]
impl LlmBackend for Script {
    fn name(&self) -> &str {
        "assist-script"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.requests.lock().unwrap().push(req);
        self.replies.lock().unwrap().pop_front().unwrap_or_else(|| Err(LlmError::EmptyResponse))
    }
}

/// 渡されたテンプレートを記録し、台本のバックエンドを返す（固有スキルの外し漏れを見るため）。
pub struct Factory {
    pub backend: Arc<Script>,
    pub templates: Arc<Mutex<Vec<ModelTemplate>>>,
}

impl BackendFactory for Factory {
    fn create(&self, template: &ModelTemplate) -> Result<BackendResolution, LlmError> {
        self.templates.lock().unwrap().push(template.clone());
        Ok(BackendResolution::healthy(Arc::clone(&self.backend) as Arc<dyn LlmBackend>))
    }
}

/// 判断役の正しい規則（`agent_02` へ渡す）。
pub const GOOD_RULES: &str = r#"
[questions.kind]
type = "choice"
ask = "依頼の種類"
options = { research = "調査", other = "その他" }

[[rules]]
when = "kind == research"
to = ["agent_02"]

[otherwise]
do = "return"
"#;

/// 検査に落ちる規則（`otherwise` が無い）。
pub const BAD_RULES: &str = r#"
[questions.kind]
type = "choice"
ask = "依頼の種類"
options = { research = "調査", other = "その他" }

[[rules]]
when = "kind == research"
to = ["agent_02"]
"#;

pub struct Village {
    pub _dir: TempDir,
    pub orchestrator: Orchestrator,
    pub requests: Arc<Mutex<Vec<ChatRequest>>>,
    pub templates: Arc<Mutex<Vec<ModelTemplate>>>,
}

pub const SERVANT: &str = "agent_01";
pub const JUDGE: &str = "router";

/// サーヴァント 2 体（`agent_01` ザリ → `agent_02` ジェミーに接続）と判断役 1 つ。
/// 生成役のテンプレートは `gen`（固有スキルを全部 ON にしてある）。
pub async fn village(tag: &str, replies: Vec<Result<ChatResponse, LlmError>>) -> Village {
    village_in(TempDir::new(tag), replies, |_| {}).await
}

pub async fn village_in(
    dir: TempDir,
    replies: Vec<Result<ChatResponse, LlmError>>,
    tweak: impl FnOnce(&mut ModelTemplate),
) -> Village {
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
    orchestrator.set_language(fuseforks_core::world::Language::Ja).await.unwrap();
    orchestrator.upsert_template(ModelTemplate::new("tpl", "既定", "mock-model")).await.unwrap();
    let mut generator = ModelTemplate::new("gen", "生成役", "gen-model");
    generator.google_search = true;
    generator.openai_web_search = true;
    generator.openai_reasoning_pro = true;
    generator.input_per_mtok = Some(1.0);
    generator.output_per_mtok = Some(2.0);
    tweak(&mut generator);
    orchestrator.upsert_template(generator).await.unwrap();
    orchestrator.create_agent(AgentSpec::new(AgentId::from("agent_02"), "ジェミー", "tpl")).await.unwrap();
    let mut zari = AgentSpec::new(AgentId::from(SERVANT), "ザリ", "tpl");
    zari.connected_agents = vec!["agent_02".into()];
    orchestrator.create_agent(zari).await.unwrap();
    orchestrator
        .create_judge(JudgeSpec { id: JUDGE.into(), name: "振り分け役".into(), order: 0, enabled: true, unknown: UnknownFields::default() })
        .await
        .unwrap();
    Village { _dir: dir, orchestrator, requests, templates }
}
