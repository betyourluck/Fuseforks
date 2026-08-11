//! 思考の要約が**表示へは届き、履歴へは入らない**（Spec 33 D2-a）。
//!
//! 契約の凍結 1 は「席を作らなければ積めない」という**型の主張**だが、
//! 型だけを見ても**経路が正しく繋がっているか**は分からない。ここで留めるのは
//! 経路の 2 点で、**両方を 1 本で見る**:
//!
//! - **正の対照**: 要約が `AgentMessage` に載る（載らなければ配線が死んでいる）
//! - **本題**: 次のターンのプロンプトに要約が入っていない
//!
//! 正の対照をもう 1 段重ねてある — 次のターンのプロンプトには**前の答えの本文が
//! 入っている**。これが無いと「履歴そのものが空」の実装でも緑になり、
//! 「要約が入っていない」は何も証明しない。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fuseforks_core::event::CoreEvent;
use fuseforks_core::llm::{ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, Usage};
use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};
use tokio::sync::broadcast::Receiver;

/// 思考の要約（英語で返る。実測どおりの形にしてある）。
const SUMMARY: &str = "I'm working through a logic puzzle where A accuses B.";
/// モデルが返す本文。**履歴に載る側**の対照。
const ANSWER: &str = "犯人は B です";

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-summary-{tag}-{}",
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

/// 毎回、要約つきの応答を返しながら、**受け取ったプロンプト全体**を覚える。
///
/// 可変文脈は最終 user 発話へ畳まれる（#45）ので、`Role::System` だけを見る
/// probe では足りない。全メッセージを連結して覚える。
#[derive(Default)]
struct SummarizingProbe {
    prompts: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl LlmBackend for SummarizingProbe {
    fn name(&self) -> &str {
        "summarizing-probe"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let joined: String = req
            .messages
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n---\n");
        self.prompts.lock().unwrap().push(joined);

        Ok(ChatResponse {
            text: Some(ANSWER.into()),
            tool_calls: Vec::new(),
            finish: Finish::Stop,
            usage: Usage {
                prompt: 1,
                completion: 1,
                cache_read: 0,
                reasoning: 1,
            },
            grounding: Default::default(),
            reasoning_summary: vec![SUMMARY.into()],
        })
    }
}

/// 静かになるまでイベントを**集めて**返す。
///
/// `quiet` は `stats_interval`（1 秒）より短くする — 長くすると窓が原理的に
/// 閉じず、「失敗」ではなく「永久に返らない」として現れる（`failures.md` #86）。
async fn drain_until_quiet(rx: &mut Receiver<CoreEvent>) -> Vec<CoreEvent> {
    let mut seen = Vec::new();
    while let Ok(Ok(event)) = tokio::time::timeout(Duration::from_millis(400), rx.recv()).await {
        seen.push(event);
    }
    seen
}

async fn setup(dir: &TempDir, backend: Arc<SummarizingProbe>) -> (Orchestrator, AgentId) {
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(backend)),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig {
            schedule_interval: Duration::from_secs(3600),
            ..OrchestratorConfig::default()
        },
    )
    .await
    .expect("bootstrap できること");
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();

    let agent = AgentId::from("agent_01");
    orchestrator
        .create_agent(AgentSpec::new(agent.clone(), "考える人", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&agent).await.unwrap();
    (orchestrator, agent)
}

#[tokio::test]
async fn the_summary_reaches_the_message_but_never_the_next_prompt() {
    let dir = TempDir::new("path");
    let backend = Arc::new(SummarizingProbe::default());
    let (orchestrator, agent) = setup(&dir, Arc::clone(&backend)).await;
    let mut rx = orchestrator.subscribe();
    drain_until_quiet(&mut rx).await;

    // 1 ターン目。
    orchestrator
        .send_user_message(&agent, "犯人は誰？")
        .await
        .unwrap();
    let events = drain_until_quiet(&mut rx).await;

    // 正の対照: 要約が発話に載っている（載らなければ配線が死んでいる）。
    let with_summary: Vec<&fuseforks_core::AgentMessage> = events
        .iter()
        .filter_map(|e| match e {
            CoreEvent::MessageSent { message } if !message.reasoning_summary.is_empty() => {
                Some(message)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        with_summary.len(),
        1,
        "要約は 1 ターンぶんの事実なので、発話 1 通にだけ載る"
    );
    assert_eq!(with_summary[0].reasoning_summary, vec![SUMMARY.to_owned()]);

    // 2 ターン目。
    orchestrator
        .send_user_message(&agent, "もう一度確認して")
        .await
        .unwrap();
    drain_until_quiet(&mut rx).await;

    let prompts = backend.prompts.lock().unwrap();
    assert!(
        prompts.len() >= 2,
        "2 ターン目のプロンプトが取れていること（取れないと以下は何も検査しない）"
    );
    let second = prompts.last().unwrap();

    // 正の対照 2 段目: **前の答えは履歴に載っている**。
    // これが無いと「履歴そのものが空」の実装でも下の assert が通ってしまう。
    assert!(
        second.contains(ANSWER),
        "履歴は運ばれている（この対照が無いと本題が何も証明しない）"
    );
    // 本題: 要約は履歴に入らない。
    assert!(
        !second.contains(SUMMARY),
        "思考の要約はプロンプトへ戻らない（Spec 33 D2-a）"
    );
}
