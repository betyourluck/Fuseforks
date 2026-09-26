//! 判断役の結合テストの土台（`judge_agents.rs` と `judge_log.rs` が共有する）。
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fuseforks_core::event::CoreEvent;
use fuseforks_core::judge::{Answer, Judge, JudgeError, JudgeReport, Question};
use fuseforks_core::llm::{ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, Role, ToolCall, Usage};
use fuseforks_core::model::{AgentId, AgentSpec, Endpoint, JudgeSpec, ModelTemplate, UnknownFields};
use fuseforks_core::{ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig};

pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-judge-{tag}-{}",
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

pub fn usage() -> Usage {
    Usage { prompt: 1, completion: 1, cache_read: 0, cache_write: 0, cache_write_1h: 0, reasoning: 0 }
}

pub fn stop(text: &str) -> ChatResponse {
    ChatResponse {
        text: Some(text.into()),
        tool_calls: Vec::new(),
        finish: Finish::Stop,
        usage: usage(),
        grounding: Default::default(),
        reasoning_summary: Vec::new(),
    }
}

pub fn call(name: &str, message: &str) -> ChatResponse {
    ChatResponse {
        text: Some(String::new()),
        tool_calls: vec![ToolCall {
            id: "call_1".into(),
            name: name.into(),
            args: serde_json::json!({ "message": message }),
            extra: None,
        }],
        finish: Finish::ToolUse,
        usage: usage(),
        grounding: Default::default(),
        reasoning_summary: Vec::new(),
    }
}

/// 進行役: `judge_router` が見えていれば 1 度だけ呼び、ツール結果を記録して終える。
/// それ以外（ワーカー）: 受け取った本文を記録して「ワーカーの答え」を返す。
pub struct Script {
    /// 周ごとに提示されたツール名。
    pub seen: Arc<Mutex<Vec<Vec<String>>>>,
    /// 進行役が受け取ったツール結果。
    pub tool_results: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl LlmBackend for Script {
    fn name(&self) -> &str {
        "judge-script"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let names: Vec<String> = req.tools.iter().map(|t| t.name.clone()).collect();
        self.seen.lock().unwrap().push(names.clone());
        if names.iter().any(|n| n == "judge_router") {
            if let Some(result) = req.messages.iter().rev().find(|m| m.role == Role::Tool) {
                self.tool_results.lock().unwrap().push(result.content.clone());
                return Ok(stop("まとめました"));
            }
            return Ok(call("judge_router", "LangGraph と AionUi の委譲を比べて"));
        }
        Ok(stop("ワーカーの答え"))
    }
}

/// 判断モデルの偽物。`answer` が `None` なら失敗を返す。
pub struct FakeJudge {
    pub answer: Option<(&'static str, f64)>,
    pub calls: Arc<Mutex<u32>>,
}

#[async_trait::async_trait]
impl Judge for FakeJudge {
    async fn judge(
        &self,
        _message: &str,
        _questions: &BTreeMap<String, Question>,
    ) -> Result<JudgeReport, JudgeError> {
        *self.calls.lock().unwrap() += 1;
        let Some((choice, p)) = self.answer else {
            return Err(JudgeError::Failed("偽の失敗".into()));
        };
        let mut probabilities = BTreeMap::from([
            ("research".to_owned(), 0.0),
            ("implement".to_owned(), 0.0),
            ("other".to_owned(), 0.0),
        ]);
        probabilities.insert(choice.to_owned(), p);
        Ok(JudgeReport {
            model: "fake".into(),
            answers: BTreeMap::from([("kind".to_owned(), Answer::Choice { choice: choice.to_owned(), probabilities })]),
            input_tokens: 10,
            output_tokens: 1,
        })
    }
}

/// 判断役の規則。research → 1 体 / implement → 2 体 / other → 呼び出し元自身。
pub const RULES: &str = r#"
[questions.kind]
type = "choice"
ask = "種類"
options = { research = "調査", implement = "実装", other = "その他" }

[[rules]]
when = "kind == research"
to = ["agent_02"]

[[rules]]
when = "kind == implement"
to = ["agent_02", "agent_03"]

[[rules]]
when = "kind == other"
to = ["agent_01"]

[otherwise]
to = ["agent_02"]
"#;

pub struct Village {
    pub _dir: TempDir,
    pub orchestrator: Orchestrator,
    pub seen: Arc<Mutex<Vec<Vec<String>>>>,
    pub tool_results: Arc<Mutex<Vec<String>>>,
    pub judge_calls: Arc<Mutex<u32>>,
}

pub const COORDINATOR: &str = "agent_01";

/// 進行役（agent_01）→ 判断役（router）/ ワーカー 2 体（agent_02・agent_03）。
/// `servant_link` が真なら進行役はワーカー 1 体にも直接繋ぐ。
pub async fn village(tag: &str, answer: Option<(&'static str, f64)>, with_model: bool, servant_link: bool) -> Village {
    village_in(TempDir::new(tag), answer, with_model, servant_link).await
}

/// `dir` を先に作っておきたいとき（ログを開いてから村を建てる）。
pub async fn village_in(dir: TempDir, answer: Option<(&'static str, f64)>, with_model: bool, servant_link: bool) -> Village {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let tool_results = Arc::new(Mutex::new(Vec::new()));
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(Arc::new(Script {
            seen: Arc::clone(&seen),
            tool_results: Arc::clone(&tool_results),
        }))),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig { schedule_interval: Duration::from_secs(3600), ..OrchestratorConfig::default() },
    )
    .await
    .expect("bootstrap");
    orchestrator.set_language(fuseforks_core::world::Language::Ja).await.unwrap();
    orchestrator.upsert_template(ModelTemplate::new("tpl", "既定", "mock-model")).await.unwrap();
    for (id, name) in [("agent_02", "イクス"), ("agent_03", "ザリ")] {
        orchestrator.create_agent(AgentSpec::new(AgentId::from(id), name, "tpl")).await.unwrap();
    }
    orchestrator
        .create_judge(JudgeSpec { id: "router".into(), name: "振り分け役".into(), order: 0, unknown: UnknownFields::default() })
        .await
        .unwrap();
    orchestrator.create_agent(AgentSpec::new(AgentId::from(COORDINATOR), "ルナ", "tpl")).await.unwrap();
    orchestrator.save_judge_file(&"router".into(), RULES).await.expect("規則は通る");
    let mut spec = AgentSpec::new(AgentId::from(COORDINATOR), "ルナ", "tpl");
    spec.connected_agents = if servant_link {
        vec!["agent_02".into(), "router".into()]
    } else {
        vec!["router".into()]
    };
    orchestrator.update_agent(spec).await.unwrap();

    let judge_calls = Arc::new(Mutex::new(0));
    if with_model {
        orchestrator
            .set_judge(Some(Arc::new(FakeJudge { answer, calls: Arc::clone(&judge_calls) })))
            .await;
    }
    for id in ["agent_02", "agent_03", COORDINATOR] {
        orchestrator.start_agent(&id.into()).await.unwrap();
    }
    Village { _dir: dir, orchestrator, seen, tool_results, judge_calls }
}

/// 進行役へ 1 通送り、静かになるまで待つ。**窓は統計の周期（1 秒）より短く**（#86）。
pub async fn run(v: &Village) -> Vec<CoreEvent> {
    let mut rx = v.orchestrator.subscribe();
    v.orchestrator.send_user_message(&COORDINATOR.into(), "振り分けて").await.unwrap();
    let mut events = Vec::new();
    while let Ok(Ok(e)) = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
        events.push(e);
    }
    events
}

pub async fn deliveries_to(v: &Village, id: &str) -> Vec<String> {
    v.orchestrator
        .message_log(None)
        .await
        .into_iter()
        .filter(|m| {
            m.from == Endpoint::Agent { id: COORDINATOR.into() } && m.to == Endpoint::Agent { id: id.into() }
        })
        .map(|m| m.content)
        .collect()
}

