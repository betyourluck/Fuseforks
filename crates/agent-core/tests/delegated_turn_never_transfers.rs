//! **委譲（`ask` / `plan`）で呼ばれたターンでは転送を提示しない。**
//!
//! 起点は実機（2026-08-11、`fuseforks.log` 04:54〜04:59）— ワーカーが
//! `ask` に答えず**依頼主自身へ転送した**（`handoff: agent=agent_3 to=agent`）。
//! その結果 **1 つの依頼が 2 本に分裂した**:
//!
//! 1. `ask_agent_3` の戻り口には「答えはこちらへ戻りません」の定型文
//!    （`body_chars=71`）が返り、依頼主は「スルーされた」と報告した
//! 2. 中身は**別の因果**として依頼主の受信箱へ積まれ、飛行中のターンが
//!    終わるのを 2 分 57 秒待ってから届き、**余分に 2 ターン**走った
//!    （28,084 + 29,453 トークン）
//!
//! **モデルの意図は正しく、選んだ道具だけが違った** — 転送先は依頼主自身で、
//! 「依頼主へ答えを返す」つもりだった。説明文では防げない（#84）ので、
//! **委譲で呼ばれたターンからは選択肢ごと外す**。
//!
//! ここで留めるのは 2 点。**提示側（1 本目）と結果側（2 本目）は別経路**で、
//! 片方だけでは守れない（Spec 27 P1 と同じ形）。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_core::llm::{ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, ToolCall, Usage};
use agent_core::model::{AgentId, AgentSpec, ModelTemplate};
use agent_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-delegated-{tag}-{}",
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

fn usage() -> Usage {
    Usage {
        prompt: 1,
        completion: 1,
        cache_read: 0,
        reasoning: 0,
    }
}

fn stop(text: &str) -> ChatResponse {
    ChatResponse {
        text: Some(text.into()),
        tool_calls: Vec::new(),
        finish: Finish::Stop,
        usage: usage(),
        grounding: Default::default(),
        reasoning_summary: Vec::new(),
    }
}

fn call(name: &str, message: &str) -> ChatResponse {
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

/// 進行役だけが 1 度 `ask_agent_02` を呼ぶ。提示ツール名を全周記録する。
///
/// **呼ぶ相手を固有名で決め打つ**のは、`ask_` の総当たりにすると
/// ワーカーも依頼主へ訊き返して往復が止まらなくなるため。
struct AskOnceProbe {
    seen: Arc<Mutex<Vec<Vec<String>>>>,
}

#[async_trait::async_trait]
impl LlmBackend for AskOnceProbe {
    fn name(&self) -> &str {
        "ask-once-probe"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let names: Vec<String> = req.tools.iter().map(|t| t.name.clone()).collect();
        self.seen.lock().unwrap().push(names.clone());
        let already_answered = req.messages.iter().any(|m| m.tool_call_id.is_some());
        if names.iter().any(|n| n == "ask_agent_02") && !already_answered {
            return Ok(call("ask_agent_02", "調べて"));
        }
        Ok(stop("答えです"))
    }
}

/// **委譲で呼ばれた周には転送が 1 つも出ず、委譲と `plan` は残る。**
///
/// 対照は**同じ村の別の周**で取る — 進行役は同じ既定の設定
/// （`allow_handoff` は真のまま）なのに転送を提示されている。
/// **効いているのは設定ではなく呼ばれ方**だと、この 2 周の差だけで読める。
#[tokio::test]
async fn a_delegated_turn_is_not_offered_transfer_tools() {
    let dir = TempDir::new("offer");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(Arc::new(AskOnceProbe {
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

    // 第三者。ワーカーの接続先を 2 体にして `plan` を生やすために要る。
    let third = AgentId::from("agent_03");
    orchestrator
        .create_agent(AgentSpec::new(third.clone(), "第三者", "tpl"))
        .await
        .unwrap();

    // 進行役とワーカーは相互に繋ぐので、片方を後から更新する
    // （`create_agent` は接続先の実在を検査する）。
    let coordinator = AgentId::from("agent_01");
    orchestrator
        .create_agent(AgentSpec::new(coordinator.clone(), "進行役", "tpl"))
        .await
        .unwrap();

    // ワーカー。**転送は許したまま**（既定）。落とすのは呼ばれ方の側。
    let worker = AgentId::from("agent_02");
    let mut spec = AgentSpec::new(worker.clone(), "ワーカー", "tpl");
    spec.connected_agents = vec![coordinator.clone(), third.clone()];
    orchestrator.create_agent(spec).await.unwrap();

    let mut spec = AgentSpec::new(coordinator.clone(), "進行役", "tpl");
    spec.connected_agents = vec![worker.clone(), third.clone()];
    orchestrator.update_agent(spec).await.unwrap();

    for id in [&third, &worker, &coordinator] {
        orchestrator.start_agent(id).await.unwrap();
    }

    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&coordinator, "調べて")
        .await
        .unwrap();
    while tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .is_ok()
    {}

    let rounds = seen.lock().unwrap().clone();
    let has = |names: &Vec<String>, prefix: &str| names.iter().any(|n| n.starts_with(prefix));

    // 正の対照。利用者から直接呼ばれた進行役には転送が出ている。
    // **これが無ければ以下の「出ない」は何も証明しない**（全周で出ない実装でも緑）。
    let direct = rounds
        .iter()
        .find(|names| names.iter().any(|n| n == "ask_agent_02"))
        .expect("進行役の周があること");
    assert!(
        has(direct, "transfer_to_"),
        "利用者から直接呼ばれた周には転送が出る: {direct:?}"
    );

    // 本題。委譲で呼ばれたワーカーの周。
    let delegated = rounds
        .iter()
        .find(|names| names.iter().any(|n| n == "ask_agent_01"))
        .expect("委譲で呼ばれたワーカーの周があること");
    assert!(
        !has(delegated, "transfer_to_"),
        "委譲で呼ばれた周に転送が出てはいけない: {delegated:?}"
    );
    // **委譲の連鎖は残る。** 1 つのフラグで 3 つとも消すと、
    // ワーカーが誰にも訊けなくなる（`allow_handoff` と同じ分け方）。
    assert!(
        has(delegated, "ask_"),
        "委譲は残る（ワーカーはさらに別の相手へ訊ける）: {delegated:?}"
    );
    assert!(
        delegated.iter().any(|n| n == "plan"),
        "並列委譲も残る: {delegated:?}"
    );
}

/// 進行役は `ask_agent_02` を 1 度呼ぶ。ワーカーは**転送が出ていれば転送する**。
///
/// 実機のワーカー（gemini-3.6-flash）の振る舞いをそのまま模したもの。
/// 修正前はここで分裂が起きた。
struct TransfersIfOffered;

#[async_trait::async_trait]
impl LlmBackend for TransfersIfOffered {
    fn name(&self) -> &str {
        "transfers-if-offered"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let names: Vec<String> = req.tools.iter().map(|t| t.name.clone()).collect();
        let already_answered = req.messages.iter().any(|m| m.tool_call_id.is_some());
        if names.iter().any(|n| n == "ask_agent_02") {
            if already_answered {
                return Ok(stop("まとめました"));
            }
            return Ok(call("ask_agent_02", "調べて"));
        }
        // ワーカー側。転送が出ていれば**依頼主へ**渡す（実機と同じ形）。
        if let Some(name) = names.iter().find(|n| n.starts_with("transfer_to_")) {
            return Ok(call(name, "調べた結果です"));
        }
        Ok(stop("調べた結果です"))
    }
}

/// **1 つの依頼が 2 本に分裂しない。**
///
/// 提示の検査（1 本目）は結果を 1 ミリも守らない — 提示していない道具でも、
/// 判定が別の経路で転送を作れば配送は起きる（実機で実際に起きた）。
/// ここは**利用者へ届いた通数**で見る。分裂すると、進行役は
/// 「答えが返らなかった」の報告と、遅れて届いた中身への答えの **2 通**を返す。
#[tokio::test]
async fn a_delegated_answer_does_not_split_into_two_conversations() {
    let dir = TempDir::new("split");
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(Arc::new(TransfersIfOffered))),
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
    orchestrator
        .create_agent(AgentSpec::new(coordinator.clone(), "進行役", "tpl"))
        .await
        .unwrap();

    // ワーカーの接続先は依頼主だけ。**実機の形**（ジェミー → ザリへ転送）。
    let worker = AgentId::from("agent_02");
    let mut spec = AgentSpec::new(worker.clone(), "ワーカー", "tpl");
    spec.connected_agents = vec![coordinator.clone()];
    orchestrator.create_agent(spec).await.unwrap();

    let mut spec = AgentSpec::new(coordinator.clone(), "進行役", "tpl");
    spec.connected_agents = vec![worker.clone()];
    orchestrator.update_agent(spec).await.unwrap();

    orchestrator.start_agent(&worker).await.unwrap();
    orchestrator.start_agent(&coordinator).await.unwrap();

    let mut rx = orchestrator.subscribe();
    let mut sent: Vec<(String, String)> = Vec::new();
    orchestrator
        .send_user_message(&coordinator, "調べて")
        .await
        .unwrap();
    while let Ok(Ok(event)) = tokio::time::timeout(Duration::from_millis(600), rx.recv()).await {
        if let agent_core::event::CoreEvent::MessageSent { message } = event {
            sent.push((format!("{:?}", message.from), format!("{:?}", message.to)));
        }
    }

    // 正の対照。ワーカーの答えが進行役へ戻っている。
    assert!(
        sent.iter()
            .any(|(from, to)| from.contains("agent_02") && to.contains("agent_01")),
        "委譲の答えは依頼主へ戻る（戻っていないなら以下は何も検査しない）: {sent:?}"
    );
    // 本題。利用者へ返るのは 1 通だけ。
    let to_user = sent
        .iter()
        .filter(|(from, to)| from.contains("agent_01") && to == "User")
        .count();
    assert_eq!(
        to_user, 1,
        "1 つの依頼への答えは 1 通。2 通なら分裂している: {sent:?}"
    );
}
