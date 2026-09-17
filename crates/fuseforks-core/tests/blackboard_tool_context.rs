//! Spec 55 P1 — `ToolContext` の 2 欄（`agent_names` / `uses_blackboard`）が、
//! **提示時（`spec_for`）と実行時（`call`）の両方**へ届くことを見る結合テスト。
//!
//! 黒板ツールは `enabled_tools` の外に居て、提示の門を `spec_for(ctx)` が決める。
//! ctx を組むのは `turn.rs` の 2 箇所で、片方だけ写し忘れると
//! 「提示はされるのに実行で断られる」か、その逆になる（Spec 20 で踏んだ形）。
//! 型検査は 2 箇所とも欄を埋めることしか強制しないので、**値が個体の設定と村の
//! 顔ぶれから来ていること**はここで留める。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fuseforks_core::event::CoreEvent;
use fuseforks_core::llm::{
    ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, ToolCall, ToolSpec, Usage,
};
use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::{
    AgentTool, ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator,
    OrchestratorConfig, ToolContext,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-bbctx-{tag}-{}",
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

/// 受け取った ctx の 2 欄を、提示時と実行時で別々に書き留めるだけのツール。
#[derive(Default)]
struct Seen {
    presented: Vec<(Vec<(AgentId, String)>, bool)>,
    called: Vec<(Vec<(AgentId, String)>, bool)>,
}

struct CtxProbe(Arc<Mutex<Seen>>);

#[async_trait::async_trait]
impl AgentTool for CtxProbe {
    fn name(&self) -> &str {
        "ctx_probe"
    }

    fn description(&self, _language: fuseforks_core::world::Language) -> String {
        "ctx を書き留める".into()
    }

    fn parameters(&self, _language: fuseforks_core::world::Language) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }

    async fn spec_for(&self, ctx: &ToolContext) -> Option<ToolSpec> {
        self.0
            .lock()
            .unwrap()
            .presented
            .push((ctx.agent_names.clone(), ctx.uses_blackboard));
        Some(self.spec(ctx.language))
    }

    async fn call(&self, ctx: &ToolContext, _args: &serde_json::Value) -> fuseforks_core::CoreResult<String> {
        self.0
            .lock()
            .unwrap()
            .called
            .push((ctx.agent_names.clone(), ctx.uses_blackboard));
        Ok("ok".into())
    }
}

/// 1 回目だけ `ctx_probe` を呼び、2 回目は本文で終える。
#[derive(Default)]
struct CallsProbeOnce(Mutex<usize>);

#[async_trait::async_trait]
impl LlmBackend for CallsProbeOnce {
    fn name(&self) -> &str {
        "calls-probe-once"
    }

    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let mut calls = self.0.lock().unwrap();
        let first = *calls == 0;
        *calls += 1;
        Ok(ChatResponse {
            text: Some(if first { String::new() } else { "終わりました".into() }),
            tool_calls: if first {
                vec![ToolCall {
                    id: "call_1".into(),
                    name: "ctx_probe".into(),
                    args: serde_json::json!({}),
                    extra: None,
                }]
            } else {
                Vec::new()
            },
            finish: Finish::Stop,
            usage: Usage {
                prompt: 1,
                completion: 1,
                cache_read: 0,
                cache_write: 0,
                cache_write_1h: 0,
                reasoning: 0,
            },
            grounding: Default::default(),
            reasoning_summary: Vec::new(),
        })
    }
}

async fn run_one_turn(uses_blackboard: bool) -> Seen {
    let dir = TempDir::new(if uses_blackboard { "on" } else { "off" });
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(Arc::new(CallsProbeOnce::default()))),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig::default(),
    )
    .await
    .expect("bootstrap できること");
    orchestrator.set_language(fuseforks_core::world::Language::Ja).await.unwrap();
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();

    let seen = Arc::new(Mutex::new(Seen::default()));
    orchestrator.register_tool(Arc::new(CtxProbe(Arc::clone(&seen)))).await;

    // 2 体。接続は引かない — 名前の表は**顔ぶれ（接続先）ではなく村の全個体**から来る。
    let mut zari = AgentSpec::new("agent", "ザリ", "tpl");
    zari.uses_blackboard = uses_blackboard;
    orchestrator.create_agent(zari).await.unwrap();
    orchestrator
        .create_agent(AgentSpec::new("agent_2", "ジェミー", "tpl"))
        .await
        .unwrap();

    let mut rx = orchestrator.subscribe();
    let id = AgentId::from("agent");
    orchestrator.start_agent(&id).await.unwrap();
    orchestrator.send_user_message(&id, "probe を呼んで").await.unwrap();

    // 返信が出るまで待つ（ツールを 1 回呼んで本文で終える）。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let event = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("返信が出ること")
            .expect("受信できること");
        if let CoreEvent::MessageSent { message } = event
            && message.content.contains("終わりました")
        {
            break;
        }
    }

    let mut guard = seen.lock().unwrap();
    std::mem::take(&mut *guard)
}

fn names() -> Vec<(AgentId, String)> {
    vec![
        (AgentId::from("agent"), "ザリ".to_owned()),
        (AgentId::from("agent_2"), "ジェミー".to_owned()),
    ]
}

#[tokio::test]
async fn both_sites_carry_the_village_names_and_the_agents_setting() {
    let seen = run_one_turn(true).await;
    assert!(!seen.presented.is_empty(), "提示の段を通っていること");
    assert_eq!(seen.called.len(), 1, "実行は 1 回");
    for (got_names, flag) in seen.presented.iter().chain(seen.called.iter()) {
        assert_eq!(got_names, &names(), "接続していない個体も id 順で載る");
        assert!(*flag, "既定は真");
    }
}

#[tokio::test]
async fn an_opted_out_agent_reaches_both_sites_as_false() {
    let seen = run_one_turn(false).await;
    assert!(!seen.presented.is_empty());
    assert_eq!(seen.called.len(), 1);
    assert!(seen.presented.iter().all(|(_, flag)| !*flag), "提示時に偽が届く");
    assert!(seen.called.iter().all(|(_, flag)| !*flag), "実行時に偽が届く");
}
