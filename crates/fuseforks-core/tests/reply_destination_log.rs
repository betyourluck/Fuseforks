//! 答えの宛先と転送がログに残る（計器）。
//!
//! **起点は診断できなかったこと**（2026-08-11）— 利用者から
//! 「委譲したのにユーザーへ返る」と報告されたが、**ログのどの行にも宛先が
//! 記録されておらず**、`transfer_to_*` に至っては `tool:` 行を出す前に
//! ループを抜けるので **grep が構造的に 0 件しか返さなかった**。
//! 「使われていない」と「見えていない」が同じ 0 に畳まれていた（#90 の再演）。
//!
//! ここで留めるのは 2 点で、**対で見る** — 転送した回に `handoff:` が出て、
//! その相手の答えが `reply: … to=user` になること。
//! 片方だけでは「常に出る実装」と区別が付かない。
//!
//! **診断の出口はプロセスで 1 つ**（`OnceLock`）なので、**ログを読むテストは
//! このファイルに 1 つだけ**にする（`tests/diag.rs` と同じ制約）。
//! 2 本目（提示の検査）はログを読まないので同居できる。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fuseforks_core::llm::{ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, ToolCall, Usage};
use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-replylog-{tag}-{}",
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

/// 転送ツールが提示されていれば 1 度だけ転送し、以後は本文を返す。
struct HandoffOnceBackend;

#[async_trait::async_trait]
impl LlmBackend for HandoffOnceBackend {
    fn name(&self) -> &str {
        "handoff-once"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let transfer = req.tools.iter().find(|t| t.name.starts_with("transfer_to_"));
        // 既に転送された側（提示された相手が居ない）なら本文を返す。
        let tool_calls = match transfer {
            Some(tool) => vec![ToolCall {
                id: "call_1".into(),
                name: tool.name.clone(),
                args: serde_json::json!({ "message": "任せた" }),
                extra: None,
            }],
            None => Vec::new(),
        };
        Ok(ChatResponse {
            text: Some("答えです".into()),
            finish: if tool_calls.is_empty() {
                Finish::Stop
            } else {
                Finish::ToolUse
            },
            tool_calls,
            usage: Usage {
                prompt: 1,
                completion: 1,
                cache_read: 0,
                reasoning: 0,
            },
            grounding: Default::default(),
            reasoning_summary: Vec::new(),
        })
    }
}

#[tokio::test]
async fn a_handoff_is_logged_and_the_reply_goes_to_the_user() {
    let dir = TempDir::new("handoff");
    let log_path = dir.0.join("fuseforks.log");
    fuseforks_core::open_log(&log_path).expect("ログを開けること");

    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(Arc::new(
            HandoffOnceBackend,
        ))),
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

    let coordinator = AgentId::from("agent_01");
    let worker = AgentId::from("agent_02");
    orchestrator
        .create_agent(AgentSpec::new(worker.clone(), "ワーカー", "tpl"))
        .await
        .unwrap();
    let mut spec = AgentSpec::new(coordinator.clone(), "進行役", "tpl");
    spec.connected_agents = vec![worker.clone()];
    orchestrator.create_agent(spec).await.unwrap();
    orchestrator.start_agent(&worker).await.unwrap();
    orchestrator.start_agent(&coordinator).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&coordinator, "調べて")
        .await
        .unwrap();
    while tokio::time::timeout(Duration::from_millis(400), rx.recv())
        .await
        .is_ok()
    {}

    let body = std::fs::read_to_string(&log_path).expect("ログが読めること");

    assert!(
        body.contains("handoff: agent=agent_01"),
        "転送がログに残ること（これが無いと診断できない）:
{body}"
    );
    // **本題**: 転送で来た依頼の答えはユーザーへ行く。
    assert!(
        body.contains("reply: agent=agent_02") && body.contains("to=user"),
        "転送された側の答えは reply_to を持たないので user へ返る:
{body}"
    );
}

/// 提示されたツール名を覚えるだけのバックエンド。
struct ToolProbe {
    seen: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
}

#[async_trait::async_trait]
impl LlmBackend for ToolProbe {
    fn name(&self) -> &str {
        "tool-probe"
    }
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.seen
            .lock()
            .unwrap()
            .push(req.tools.iter().map(|t| t.name.clone()).collect());
        Ok(ChatResponse {
            text: Some("了解".into()),
            tool_calls: Vec::new(),
            finish: Finish::Stop,
            usage: Usage {
                prompt: 1,
                completion: 1,
                cache_read: 0,
                reasoning: 0,
            },
            grounding: Default::default(),
            reasoning_summary: Vec::new(),
        })
    }
}

/// `allow_handoff = false` は**転送だけ**を落とす。
///
/// **委譲（`ask_*`）と並列委譲（`plan`）は残る** — ここが本題で、
/// 1 つのフラグで 3 つとも消すと「進行役が誰にも頼めない」に化ける。
/// 正の対照（既定の個体には 3 つとも出る）と並べて見る。
#[tokio::test]
async fn disabling_handoff_removes_only_transfer_tools() {
    let dir = TempDir::new("gate");
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(Arc::new(ToolProbe {
            seen: Arc::clone(&seen),
        }))),
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

    // 接続先 2 体（`plan` は 2 体以上でだけ生える）。
    for id in ["agent_02", "agent_03"] {
        orchestrator
            .create_agent(AgentSpec::new(AgentId::from(id), id, "tpl"))
            .await
            .unwrap();
        orchestrator.start_agent(&AgentId::from(id)).await.unwrap();
    }

    let permissive = AgentId::from("agent_01");
    let mut spec = AgentSpec::new(permissive.clone(), "既定", "tpl");
    spec.connected_agents = vec![AgentId::from("agent_02"), AgentId::from("agent_03")];
    orchestrator.create_agent(spec).await.unwrap();
    orchestrator.start_agent(&permissive).await.unwrap();

    let restricted = AgentId::from("agent_04");
    let mut spec = AgentSpec::new(restricted.clone(), "進行役", "tpl");
    spec.connected_agents = vec![AgentId::from("agent_02"), AgentId::from("agent_03")];
    spec.allow_handoff = false;
    orchestrator.create_agent(spec).await.unwrap();
    orchestrator.start_agent(&restricted).await.unwrap();

    let mut rx = orchestrator.subscribe();
    for target in [&permissive, &restricted] {
        orchestrator.send_user_message(target, "やあ").await.unwrap();
        while tokio::time::timeout(Duration::from_millis(400), rx.recv())
            .await
            .is_ok()
        {}
    }

    let rounds = seen.lock().unwrap().clone();
    let has = |names: &Vec<String>, prefix: &str| names.iter().any(|n| n.starts_with(prefix));

    // 正の対照: 既定の個体には 3 つとも出る。
    let default_round = rounds
        .iter()
        .find(|names| has(names, "transfer_to_"))
        .expect("既定の個体には転送が出ること（出ないなら検査が空振り）");
    assert!(has(default_round, "ask_"));
    assert!(default_round.iter().any(|n| n == "plan"));

    // 本題: 転送だけが消え、委譲と plan は残る。
    let restricted_round = rounds
        .iter()
        .find(|names| !has(names, "transfer_to_") && has(names, "ask_"))
        .expect("転送を落とした個体の周があること");
    assert!(
        restricted_round.iter().any(|n| n == "plan"),
        "plan は残る（消すと進行役が手分けできなくなる）: {restricted_round:?}"
    );
}

/// ツールを呼んだだけの応答（本文は空）を返すバックエンド。
///
/// **転送を切った個体がここで転送してはいけない。** 実機ではこの形で
/// 空の本文が最初の接続先へ配送された（2026-08-11。判定側の未検査）。
struct AsksThenEmptyBackend;

#[async_trait::async_trait]
impl LlmBackend for AsksThenEmptyBackend {
    fn name(&self) -> &str {
        "asks-then-empty"
    }
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let ask = req.tools.iter().find(|t| t.name.starts_with("ask_"));
        let tool_calls = match ask {
            // 既に結果を受け取っていれば終わる。
            Some(tool) if !req.messages.iter().any(|m| m.tool_call_id.is_some()) => {
                vec![ToolCall {
                    id: "call_1".into(),
                    name: tool.name.clone(),
                    args: serde_json::json!({ "message": "調べて" }),
                    extra: None,
                }]
            }
            _ => Vec::new(),
        };
        Ok(ChatResponse {
            // **本文は空。** 旧経路へ落ちると、この空文字が転送される。
            text: Some(String::new()),
            finish: if tool_calls.is_empty() {
                Finish::Stop
            } else {
                Finish::ToolUse
            },
            tool_calls,
            usage: Usage {
                prompt: 1,
                completion: 1,
                cache_read: 0,
                reasoning: 0,
            },
            grounding: Default::default(),
            reasoning_summary: Vec::new(),
        })
    }
}

/// **転送を切った個体は、どんな応答でも転送で抜けない。**
///
/// 提示の検査（上のテスト）は**判定側を 1 ミリも守らない** — 提示していない
/// 道具でも、判定が別の経路で転送を作れば配送は起きる。実機で起きたのがこれで、
/// `decide` の引数に「ツールを使えるか」と「転送を出すか」を畳んで渡していた
/// ため、転送を切った個体がツール非対応モデル用の旧経路（終了マーカーが
/// 無ければ最初の相手へ渡す）へ落ちていた。
#[tokio::test]
async fn a_restricted_agent_never_hands_off_even_with_empty_text() {
    let dir = TempDir::new("nofallback");
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(Arc::new(AsksThenEmptyBackend))),
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

    let worker = AgentId::from("agent_02");
    orchestrator
        .create_agent(AgentSpec::new(worker.clone(), "ワーカー", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&worker).await.unwrap();

    let coordinator = AgentId::from("agent_01");
    let mut spec = AgentSpec::new(coordinator.clone(), "進行役", "tpl");
    spec.connected_agents = vec![worker.clone()];
    spec.allow_handoff = false;
    orchestrator.create_agent(spec).await.unwrap();
    orchestrator.start_agent(&coordinator).await.unwrap();

    let mut rx = orchestrator.subscribe();
    let mut sent: Vec<(String, String)> = Vec::new();
    orchestrator
        .send_user_message(&coordinator, "調べて")
        .await
        .unwrap();
    while let Ok(Ok(event)) = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
        if let fuseforks_core::event::CoreEvent::MessageSent { message } = event {
            sent.push((format!("{:?}", message.from), format!("{:?}", message.to)));
        }
    }

    // **判定は「答えがどちらへ戻ったか」**。委譲の依頼も転送も、進行役から
    // ワーカーへの配送として 1 通記録される（画面の「ザリ → イクス」がそれ）ので、
    // **配送の有無では区別できない**。区別が付くのは戻りの向きだけ。
    //
    // 正の対照: ワーカーの答えが**進行役へ**戻っている。
    assert!(
        sent.iter()
            .any(|(from, to)| from.contains("agent_02") && to.contains("agent_01")),
        "委譲の答えは依頼主へ戻る（戻っていないなら以下は何も検査しない）: {sent:?}"
    );
    // 本題: ワーカーの答えが利用者へ流れていない。
    // 旧経路へ落ちると `reply_to` を持たない配送になり、ここが `User` になる。
    assert!(
        !sent
            .iter()
            .any(|(from, to)| from.contains("agent_02") && to == "User"),
        "転送を切った個体の依頼で、答えが利用者へ逸れてはいけない: {sent:?}"
    );
}
