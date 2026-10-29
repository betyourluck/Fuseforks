//! Spec 72（コマンドの承認待ちでターンを止める）の配線層の結合テスト。
//!
//! 確かめるのは**周の終わりで閉じること**とその副作用: 次の周の LLM 呼び出しが無い /
//! 会話ログへ System 行が 1 本 / `Record::Turn` の `stop=approval_pending` /
//! 止まった印（`ApprovalStopped`）の出し入れ / 依頼主への `Reply.kind`。
//! 計器（`turn cut:` / `acceptance: … skipped`）はログを読むテストが別のファイルで持つ
//! （診断ログの宛先はプロセスで 1 つなので、ログを読むテストは 1 ファイル 1 本）。
//!
//! 観測は**機構が保証するもの**だけ — モデルが何本呼び直すかはここでは決めない。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use fuseforks_core::command::RunApproval;
use fuseforks_core::event::CoreEvent;
use fuseforks_core::llm::{
    ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, Role, ToolCall, Usage,
};
use fuseforks_core::model::{AgentId, AgentSpec, Endpoint, ModelTemplate};
use fuseforks_core::plan::PlanTaskState;
use fuseforks_core::stats::StatsScope;
use fuseforks_core::tool::{AgentTool, ToolContext};
use fuseforks_core::tools::run::RunTool;
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};
use tokio::sync::broadcast::Receiver;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-approval-stop-{tag}-{}",
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
        prompt: 10,
        completion: 2,
        cache_read: 0,
        cache_write: 0,
        cache_write_1h: 0,
        reasoning: 0,
    }
}

fn text_response(text: &str) -> ChatResponse {
    ChatResponse {
        text: Some(text.to_owned()),
        tool_calls: Vec::new(),
        finish: Finish::Stop,
        usage: usage(),
        grounding: Default::default(),
        reasoning_summary: Vec::new(),
        replay: None,
    }
}

fn calls_response(tool_calls: Vec<ToolCall>) -> ChatResponse {
    ChatResponse {
        text: Some(String::new()),
        tool_calls,
        finish: Finish::ToolUse,
        usage: usage(),
        grounding: Default::default(),
        reasoning_summary: Vec::new(),
        replay: None,
    }
}

/// 台本のバックエンド。**最初の 1 回だけ** `run` を `commands` の本数ぶん並べて呼び、
/// 以後は本文で終える。`ask_*` が提示されていてまだ結果を受け取っていなければ ask する
/// （進行役の役回り。結果を受け取ったら引用して終える）。
///
/// `gate` があれば、最初の呼び出しはそれが開くまで返らない（cancel の順序を作るため）。
struct ScriptedBackend {
    commands: Vec<(String, Vec<String>)>,
    tool_name: String,
    chats: AtomicUsize,
    gate: Option<Arc<tokio::sync::Notify>>,
    /// LLM 呼び出しに**入った**ことを知らせる（gate の対。cancel を「周の頭の検査の後・
    /// 周の終わりの検査の前」に立てるための順序作り）。
    entered: Option<Arc<tokio::sync::Notify>>,
}

impl ScriptedBackend {
    fn running(commands: &[(&str, &[&str])]) -> Arc<Self> {
        Arc::new(Self {
            commands: commands
                .iter()
                .map(|(c, a)| ((*c).to_owned(), a.iter().map(|s| (*s).to_owned()).collect()))
                .collect(),
            tool_name: "run".into(),
            chats: AtomicUsize::new(0),
            gate: None,
            entered: None,
        })
    }

    fn gated(
        commands: &[(&str, &[&str])],
        entered: Arc<tokio::sync::Notify>,
        gate: Arc<tokio::sync::Notify>,
    ) -> Arc<Self> {
        let mut backend = Arc::try_unwrap(Self::running(commands)).ok().expect("唯一の参照");
        backend.entered = Some(entered);
        backend.gate = Some(gate);
        Arc::new(backend)
    }

    /// `run` ではない名前のツールを同じ引数で呼ぶ版（型で運ぶことの対照）。
    fn calling(tool_name: &str, commands: &[(&str, &[&str])]) -> Arc<Self> {
        let mut backend = Arc::try_unwrap(Self::running(commands)).ok().expect("唯一の参照");
        backend.tool_name = tool_name.to_owned();
        Arc::new(backend)
    }

    fn chats(&self) -> usize {
        self.chats.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl LlmBackend for ScriptedBackend {
    fn name(&self) -> &str {
        "scripted"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let answered = req.messages.iter().any(|m| m.role == Role::Tool);
        let tool_result = req
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::Tool)
            .map(|m| m.content.clone());
        // 進行役（plan）: 接続先が 2 体以上なら全員へ 1 波で撒き、束ねを受け取ったら引用して終える。
        if req.tools.iter().any(|t| t.name == "plan") {
            if !answered {
                let tasks: Vec<serde_json::Value> = req
                    .tools
                    .iter()
                    .filter_map(|t| t.name.strip_prefix("ask_"))
                    .map(|to| serde_json::json!({ "to": to, "message": "調べてください" }))
                    .collect();
                return Ok(calls_response(vec![ToolCall {
                    id: "plan_1".into(),
                    name: "plan".into(),
                    args: serde_json::json!({ "tasks": tasks }),
                    extra: None,
                }]));
            }
            return Ok(text_response(&format!(
                "束ねました → {}",
                tool_result.unwrap_or_default()
            )));
        }
        // 進行役（ask）: ask が提示されていて結果をまだ受け取っていなければ ask する。
        if let Some(ask) = req.tools.iter().find(|t| t.name.starts_with("ask_")) {
            if !answered {
                return Ok(calls_response(vec![ToolCall {
                    id: "ask_1".into(),
                    name: ask.name.clone(),
                    args: serde_json::json!({ "message": "調べてください" }),
                    extra: None,
                }]));
            }
            return Ok(text_response(&format!(
                "受け取りました → {}",
                tool_result.unwrap_or_default()
            )));
        }
        self.chats.fetch_add(1, Ordering::SeqCst);
        // ワーカー: 道具が提示されていて、まだ結果を受け取っておらず、続行の定型文でもなければ呼ぶ。
        // **個体ごとに判定する**（共有のカウンタだと 2 体目のワーカーが本文で終わる）。
        let resumed = req
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .is_some_and(|m| m.content.contains("承認しました"));
        if !answered && !resumed {
            if let Some(entered) = &self.entered {
                entered.notify_one();
            }
            if let Some(gate) = &self.gate {
                gate.notified().await;
            }
            if req.tools.iter().any(|t| t.name == self.tool_name) {
                let calls = self
                    .commands
                    .iter()
                    .enumerate()
                    .map(|(i, (command, args))| ToolCall {
                        id: format!("call_{i}"),
                        name: self.tool_name.clone(),
                        args: serde_json::json!({ "command": command, "args": args }),
                        extra: None,
                    })
                    .collect();
                return Ok(calls_response(calls));
            }
        }
        Ok(text_response(&format!(
            "終わりました{}",
            tool_result.map(|t| format!(" → {t}")).unwrap_or_default()
        )))
    }
}

/// `run` の拒否文と同じ字面を返すだけの別のツール（D2 — 型で運ぶことの対照）。
struct Mimic;

#[async_trait::async_trait]
impl AgentTool for Mimic {
    fn name(&self) -> &str {
        "mimic"
    }
    fn description(&self, _language: fuseforks_core::world::Language) -> String {
        "拒否文の字面を返す".into()
    }
    fn parameters(&self, _language: fuseforks_core::world::Language) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    async fn call(
        &self,
        _ctx: &ToolContext,
        _args: &serde_json::Value,
    ) -> fuseforks_core::CoreResult<String> {
        Ok("`git` は許可されていないため実行しませんでした。**このターンはここで止まります。**".into())
    }
}

async fn setup(dir: &TempDir, backend: Arc<dyn LlmBackend>) -> Orchestrator {
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(backend)),
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
    orchestrator
        .register_tool(Arc::new(RunTool::new(ConfigStore::new(&dir.0))))
        .await;
    orchestrator
}

/// `run` を持つ個体を 1 体起こす。`run.json` は無い = `allow` も `deny` も空（全部 pending）。
async fn spawn_runner(orchestrator: &Orchestrator, dir: &TempDir, id: &str) -> AgentId {
    let agent = AgentId::from(id);
    let mut spec = AgentSpec::new(agent.clone(), format!("ロボット{id}"), "tpl");
    spec.enabled_tools = Some(vec!["run".into()]);
    spec.work_dir = Some(dir.0.to_string_lossy().into_owned());
    orchestrator.create_agent(spec).await.unwrap();
    orchestrator.start_agent(&agent).await.unwrap();
    agent
}

async fn drain_until_quiet(rx: &mut Receiver<CoreEvent>, quiet: Duration) -> Vec<CoreEvent> {
    let mut events = Vec::new();
    while let Ok(Ok(event)) = tokio::time::timeout(quiet, rx.recv()).await {
        events.push(event);
    }
    events
}

fn system_rows(events: &[CoreEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            CoreEvent::MessageSent { message }
                if matches!(message.from, Endpoint::System) && message.hop == 0 =>
            {
                Some(message.content.clone())
            }
            _ => None,
        })
        .collect()
}

fn agent_answers(events: &[CoreEvent], id: &AgentId) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            CoreEvent::MessageSent { message }
                if matches!(&message.from, Endpoint::Agent { id: from } if from == id)
                    && matches!(message.to, Endpoint::User) =>
            {
                Some(message.content.clone())
            }
            _ => None,
        })
        .collect()
}

fn approval_stopped(events: &[CoreEvent]) -> Vec<(AgentId, bool)> {
    events
        .iter()
        .filter_map(|e| match e {
            CoreEvent::ApprovalStopped { agent_id, stopped } => Some((agent_id.clone(), *stopped)),
            _ => None,
        })
        .collect()
}

/// 直近の会話の `stop` の内訳。
async fn stops(orchestrator: &Orchestrator) -> Vec<(String, u64)> {
    let report = orchestrator
        .session_stats(StatsScope::Session {
            session_id: orchestrator.current_session(),
        })
        .await
        .expect("統計が読めること");
    report.by_stop.into_iter().map(|row| (row.stop, row.count)).collect()
}

async fn stopped_flag(orchestrator: &Orchestrator, id: &AgentId) -> bool {
    orchestrator
        .command_policies()
        .await
        .into_iter()
        .find(|v| &v.agent_id == id)
        .map(|v| v.stopped_for_approval)
        .expect("投影に居ること")
}

/// (1) `required` で `Unknown` 1 本 → その周でターンが閉じる。次の周の LLM 呼び出しは無く、
/// 会話ログに System 行が 1 本、`stop=approval_pending`、印が立つ。個体の答えは 1 つも無い。
#[tokio::test]
async fn an_unapproved_command_closes_the_turn_at_the_end_of_the_round() {
    let dir = TempDir::new("one");
    let backend = ScriptedBackend::running(&[("git", &["status"])]);
    let orchestrator = setup(&dir, backend.clone()).await;
    let agent = spawn_runner(&orchestrator, &dir, "agent_01").await;

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&agent, "状態を見て").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(500)).await;

    assert_eq!(backend.chats(), 1, "次の周の LLM 呼び出しは起きない（半端な答えを書かせない）");
    assert!(agent_answers(&events, &agent).is_empty(), "個体の答えは無い: {events:?}");
    let rows = system_rows(&events);
    let stopped_rows: Vec<_> = rows.iter().filter(|r| r.contains("承認待ちのコマンドが 1 件あり")).collect();
    assert_eq!(stopped_rows.len(), 1, "会話ログへ System 行が 1 本: {rows:?}");
    assert!(
        stopped_rows[0].starts_with("agent_01（ロボットagent_01）"),
        "名指しは既存 2 出口と同じ `{{agent_id}}（{{display}}）`: {}",
        stopped_rows[0]
    );
    assert_eq!(
        approval_stopped(&events),
        vec![(agent.clone(), true)],
        "印は 1 回だけ立つ"
    );
    assert!(stopped_flag(&orchestrator, &agent).await, "投影にも写る");
    assert!(
        stops(&orchestrator).await.contains(&("approval_pending".to_owned(), 1)),
        "Record::Turn の stop は approval_pending: {:?}",
        stops(&orchestrator).await
    );
    let policy = orchestrator.command_policies().await;
    assert_eq!(policy[0].pending.len(), 1, "判断待ちは積まれている（Spec 15 のまま）");
}

/// (2) 同じ周に `allow` 1 本 + 別々の `Unknown` 2 本 → `allow` は実行され、判断待ちは 2 件、
/// System 行は 2 件と書く。**周の途中では切らない**（最初の 1 本で切ると残りが pending に乗らない）。
#[tokio::test]
async fn the_rest_of_the_round_still_runs_and_every_unknown_is_queued() {
    let dir = TempDir::new("round");
    let (allowed, pattern): (&[&str], &str) = if cfg!(windows) {
        (&["/C", "ver"], "cmd /C ver")
    } else {
        (&["-c", "pwd"], "sh -c pwd")
    };
    let shell = if cfg!(windows) { "cmd" } else { "sh" };
    let backend = ScriptedBackend::running(&[
        (shell, allowed),
        ("git", &["status"]),
        ("fd", &["."]),
    ]);
    let orchestrator = setup(&dir, backend.clone()).await;
    let agent = spawn_runner(&orchestrator, &dir, "agent_01").await;
    // 1 本だけ許可（Spec 20 の承認と同じファイルを人が書いた形）。
    let run_json = dir.0.join("agents").join("agent_01").join("run.json");
    std::fs::create_dir_all(run_json.parent().unwrap()).unwrap();
    std::fs::write(
        &run_json,
        serde_json::json!({ "version": 1, "allow": [pattern], "deny": [], "pending": [] }).to_string(),
    )
    .unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&agent, "3 つ試して").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(800)).await;

    assert_eq!(backend.chats(), 1);
    let invoked: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            CoreEvent::ToolInvoked { call_id, .. } => Some(*call_id),
            _ => None,
        })
        .collect();
    assert_eq!(invoked.len(), 3, "3 本とも走った（Unknown も結果を返す）: {events:?}");
    let detail = orchestrator.tool_call(invoked[0]).expect("詳細が残ること");
    assert!(
        detail.body.contains("終了コード"),
        "allow の 1 本は実際に実行された（終了コードが本文に出る）: {}",
        detail.body
    );
    let rows = system_rows(&events);
    assert!(
        rows.iter().any(|r| r.contains("承認待ちのコマンドが 2 件あり")),
        "N はこの周で Unknown を返した本数: {rows:?}"
    );
    let policy = orchestrator.command_policies().await;
    assert_eq!(policy[0].pending.len(), 2, "別々の 2 本が判断待ちに: {:?}", policy[0].pending);
}

/// (3) 承認を省くモードでは止まらない — `Unknown` の腕が `pending` へ行かないので旗が立たない。
#[tokio::test]
async fn bypass_modes_do_not_stop() {
    for mode in [RunApproval::AutoApprove, RunApproval::NoApproval] {
        let dir = TempDir::new(&format!("bypass-{}", mode.label()));
        let (shell, args): (&str, &[&str]) = if cfg!(windows) {
            ("cmd", &["/C", "ver"])
        } else {
            ("sh", &["-c", "pwd"])
        };
        let backend = ScriptedBackend::running(&[(shell, args)]);
        let orchestrator = setup(&dir, backend.clone()).await;
        let agent = spawn_runner(&orchestrator, &dir, "agent_01").await;
        orchestrator.set_run_approval(mode);

        let mut rx = orchestrator.subscribe();
        orchestrator.send_user_message(&agent, "走らせて").await.unwrap();
        let events = drain_until_quiet(&mut rx, Duration::from_millis(800)).await;

        assert_eq!(backend.chats(), 2, "{mode:?}: 2 周目が回って本文で終わる");
        assert!(approval_stopped(&events).is_empty(), "{mode:?}: 印は立たない");
        assert_eq!(agent_answers(&events, &agent).len(), 1, "{mode:?}: 答えが 1 つ");
    }
}

/// (4) `deny` では止まらない — 承認しても通らないので人の手番が無い。拒否文を返してターンは続く。
#[tokio::test]
async fn a_denied_command_does_not_stop() {
    let dir = TempDir::new("deny");
    let backend = ScriptedBackend::running(&[("git", &["status"])]);
    let orchestrator = setup(&dir, backend.clone()).await;
    let agent = spawn_runner(&orchestrator, &dir, "agent_01").await;
    let run_json = dir.0.join("agents").join("agent_01").join("run.json");
    std::fs::create_dir_all(run_json.parent().unwrap()).unwrap();
    std::fs::write(
        &run_json,
        serde_json::json!({ "version": 1, "allow": [], "deny": ["git *"], "pending": [] }).to_string(),
    )
    .unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&agent, "git を見て").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(500)).await;

    assert_eq!(backend.chats(), 2, "deny は拒否文を返してターンが続く");
    assert!(approval_stopped(&events).is_empty());
    assert_eq!(agent_answers(&events, &agent).len(), 1);
    assert!(orchestrator.command_policies().await[0].pending.is_empty(), "deny は積まない");
}

/// (5) 承認待ちの周で cancel が立っていれば分類は割り込み（`token_budget.precedence`）。
#[tokio::test]
async fn cancel_wins_over_approval_pending_at_the_end_of_the_round() {
    let dir = TempDir::new("cancel");
    let entered = Arc::new(tokio::sync::Notify::new());
    let gate = Arc::new(tokio::sync::Notify::new());
    let backend = ScriptedBackend::gated(&[("git", &["status"])], entered.clone(), gate.clone());
    let orchestrator = setup(&dir, backend.clone()).await;
    let agent = spawn_runner(&orchestrator, &dir, "agent_01").await;

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&agent, "状態を見て").await.unwrap();
    // **LLM 呼び出しに入った後**に割り込みを立てる — 周の頭の検査（`:1631`）は既に通り過ぎて
    // いるので、cancel を拾えるのは周の終わりの検査だけ。`AgentTyping` で待つと、割り込みが
    // 周の頭の検査より先に立ち、承認待ちの腕へ届く前に閉じてしまう（最初の版はそうなっていて、
    // 腕の中の cancel を外す変異が緑のままだった）。
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .expect("LLM 呼び出しに入ること");
    orchestrator.interrupt_turn(&agent).await;
    gate.notify_one();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(500)).await;

    assert!(
        events.iter().any(|e| matches!(e, CoreEvent::ToolInvoked { .. })),
        "run は走った（周の途中では切らない）— 分類は周の終わりで決まった: {events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(e, CoreEvent::TurnInterrupted { .. })),
        "割り込みとして閉じる: {events:?}"
    );
    assert!(approval_stopped(&events).is_empty(), "承認待ちの印は立てない");
    assert!(
        stops(&orchestrator).await.contains(&("interrupted".to_owned(), 1)),
        "stop=interrupted: {:?}",
        stops(&orchestrator).await
    );
}

/// (6) 最終周で `Unknown` が出ても同じ腕で閉じる — 上限の通知もまとめ呼び出しも無い。
#[tokio::test]
async fn the_last_round_also_closes_as_approval_pending() {
    let dir = TempDir::new("last");
    let backend = ScriptedBackend::running(&[("git", &["status"])]);
    let orchestrator = setup(&dir, backend.clone()).await;
    let agent = AgentId::from("agent_01");
    let mut spec = AgentSpec::new(agent.clone(), "ロボット", "tpl");
    spec.enabled_tools = Some(vec!["run".into()]);
    spec.max_tool_iterations = Some(1);
    orchestrator.create_agent(spec).await.unwrap();
    orchestrator.start_agent(&agent).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&agent, "状態を見て").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(500)).await;

    assert_eq!(backend.chats(), 1, "まとめ呼び出し（tool_choice none の 2 回目）は無い");
    assert!(
        !events.iter().any(|e| matches!(e, CoreEvent::ToolLimitReached { .. })),
        "当たったのは上限ではない: {events:?}"
    );
    assert!(stops(&orchestrator).await.contains(&("approval_pending".to_owned(), 1)));
}

/// (7) `ask` で頼まれたワーカーが止まる → 依頼主へ `Reply.kind == ApprovalPending` の定型文が
/// 戻り、依頼主は普通に答える。ワーカーの印は立ち、依頼主の印は立たない。
#[tokio::test]
async fn a_delegated_worker_returns_approval_pending_to_the_requester() {
    let dir = TempDir::new("ask");
    let backend = ScriptedBackend::running(&[("git", &["status"])]);
    let orchestrator = setup(&dir, backend.clone()).await;
    let worker = spawn_runner(&orchestrator, &dir, "agent_02").await;
    let leader = AgentId::from("agent_01");
    let mut spec = AgentSpec::new(leader.clone(), "進行役", "tpl");
    spec.connected_agents = vec![worker.clone()];
    orchestrator.create_agent(spec).await.unwrap();
    orchestrator.start_agent(&leader).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&leader, "部下に調べさせて").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(800)).await;

    let answers = agent_answers(&events, &leader);
    assert_eq!(answers.len(), 1, "依頼主は答える: {events:?}");
    assert!(
        answers[0].contains("この依頼は承認待ちのコマンドで止まりました。答えはありません。"),
        "依頼主に届くのは D3 (4) の定型文: {}",
        answers[0]
    );
    assert!(agent_answers(&events, &worker).is_empty(), "ワーカーは利用者へ答えない");
    assert_eq!(approval_stopped(&events), vec![(worker.clone(), true)]);
    assert!(stopped_flag(&orchestrator, &worker).await);
    assert!(!stopped_flag(&orchestrator, &leader).await);
    // Reply.kind は束ねと作業状況の材料（PlanTaskState）。文字列ではなく型で運ばれる。
    let _: PlanTaskState = PlanTaskState::ApprovalPending;
}

/// (8) `plan` で撒かれたワーカーが止まる → 束ねにその定型文が入り、セルの状態は
/// `ApprovalPending`。進行役の束ねは普通に走る（捨てられない）。
#[tokio::test]
async fn a_fanned_out_worker_lands_in_the_bundle_as_approval_pending() {
    let dir = TempDir::new("plan");
    let backend = ScriptedBackend::running(&[("git", &["status"])]);
    let orchestrator = setup(&dir, backend.clone()).await;
    let w1 = spawn_runner(&orchestrator, &dir, "agent_02").await;
    let w2 = spawn_runner(&orchestrator, &dir, "agent_03").await;
    let leader = AgentId::from("agent_01");
    let mut spec = AgentSpec::new(leader.clone(), "進行役", "tpl");
    spec.connected_agents = vec![w1.clone(), w2.clone()];
    orchestrator.create_agent(spec).await.unwrap();
    orchestrator.start_agent(&leader).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&leader, "2 体に調べさせて").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(800)).await;

    let answers = agent_answers(&events, &leader);
    assert_eq!(answers.len(), 1, "束ねは捨てられず進行役が答える: {events:?}");
    assert_eq!(
        answers[0].matches("この依頼は承認待ちのコマンドで止まりました").count(),
        2,
        "2 体ぶんの定型文が束ねに入る（見出しに表示名があるので本文に名前は無い）: {}",
        answers[0]
    );
    let states: Vec<PlanTaskState> = events
        .iter()
        .filter_map(|e| match e {
            CoreEvent::PlanTaskResolved { state, .. } => Some(*state),
            _ => None,
        })
        .collect();
    assert_eq!(
        states,
        vec![PlanTaskState::ApprovalPending, PlanTaskState::ApprovalPending],
        "セルの状態は承認待ち（失敗でも打ち切りでもない）: {states:?}"
    );
    assert!(stopped_flag(&orchestrator, &w1).await && stopped_flag(&orchestrator, &w2).await);
}

/// (9) 止まった後に「続けさせる」→ 配送が受信箱に入った時点で印が外れ、`stopped: false` が出る。
#[tokio::test]
async fn resuming_clears_the_mark_as_soon_as_the_message_is_queued() {
    let dir = TempDir::new("resume");
    let backend = ScriptedBackend::running(&[("git", &["status"])]);
    let orchestrator = setup(&dir, backend.clone()).await;
    let agent = spawn_runner(&orchestrator, &dir, "agent_01").await;

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&agent, "状態を見て").await.unwrap();
    let _ = drain_until_quiet(&mut rx, Duration::from_millis(500)).await;
    assert!(stopped_flag(&orchestrator, &agent).await);

    orchestrator.resume_after_approval(&agent).await.unwrap();
    assert!(!stopped_flag(&orchestrator, &agent).await, "配送した時点で印は外れている");
    let events = drain_until_quiet(&mut rx, Duration::from_millis(500)).await;
    assert_eq!(
        approval_stopped(&events),
        vec![(agent.clone(), false)],
        "外す側も 1 回だけイベント（入口で外すときは既に外れているので 2 回目は出ない）"
    );
    assert_eq!(backend.chats(), 2, "続行のターンが 1 本走る");
}

/// (10) 止まった後に人の依頼が届く → `handle_message` の先頭で印が外れる。
/// 段 1 で落ちる依頼（テンプレートを消してから届ける）でも印は消え、`Failed` と同時に立たない。
#[tokio::test]
async fn a_new_message_clears_the_mark_even_when_the_turn_fails_at_stage_one() {
    let dir = TempDir::new("stage1");
    let backend = ScriptedBackend::running(&[("git", &["status"])]);
    let orchestrator = setup(&dir, backend.clone()).await;
    let agent = spawn_runner(&orchestrator, &dir, "agent_01").await;

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&agent, "状態を見て").await.unwrap();
    let _ = drain_until_quiet(&mut rx, Duration::from_millis(500)).await;
    assert!(stopped_flag(&orchestrator, &agent).await);

    // 段 2（システムプロンプトの組み立て）で落ちる形を作る — `Construct.md` を**フォルダ**にすると、
    // 読みが「無い」（空として扱う）ではなく I/O エラーで返る。
    // （段 1 のテンプレートは参照中で消せない。段 1 でも段 2 でも `run_turn` の前で抜けるのは同じ）
    let construct = dir.0.join("agents").join("agent_01").join("Construct.md");
    let _ = std::fs::remove_file(&construct);
    std::fs::create_dir_all(&construct).unwrap();
    orchestrator.send_user_message(&agent, "もう一度").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(500)).await;

    assert!(
        events.iter().any(|e| matches!(e, CoreEvent::AgentFailed { .. })),
        "段 1 で落ちた: {events:?}"
    );
    assert_eq!(approval_stopped(&events), vec![(agent.clone(), false)], "入口で印が外れた");
    assert!(!stopped_flag(&orchestrator, &agent).await, "Failed と印は同時に立たない");
}

/// (11) `run` 以外のツールが拒否文と同じ字面を返しても止まらない — 信号は型で運ぶ（D2）。
#[tokio::test]
async fn another_tool_echoing_the_refusal_text_does_not_stop() {
    let dir = TempDir::new("mimic");
    let backend = ScriptedBackend::calling("mimic", &[("git", &["status"])]);
    let orchestrator = setup(&dir, backend.clone()).await;
    orchestrator.register_tool(Arc::new(Mimic)).await;
    let agent = AgentId::from("agent_01");
    let mut spec = AgentSpec::new(agent.clone(), "ロボット", "tpl");
    spec.enabled_tools = Some(vec!["mimic".into()]);
    orchestrator.create_agent(spec).await.unwrap();
    orchestrator.start_agent(&agent).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&agent, "真似して").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(500)).await;

    assert_eq!(backend.chats(), 2, "字面では止まらない");
    assert!(approval_stopped(&events).is_empty());
    assert_eq!(agent_answers(&events, &agent).len(), 1);
}

/// (14) 止まった後に会話を切り替える（新規チャット）→ 全個体の印が外れる。
#[tokio::test]
async fn switching_the_conversation_clears_every_mark() {
    let dir = TempDir::new("switch");
    let backend = ScriptedBackend::running(&[("git", &["status"])]);
    let orchestrator = setup(&dir, backend.clone()).await;
    let agent = spawn_runner(&orchestrator, &dir, "agent_01").await;

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&agent, "状態を見て").await.unwrap();
    let _ = drain_until_quiet(&mut rx, Duration::from_millis(500)).await;
    assert!(stopped_flag(&orchestrator, &agent).await);

    orchestrator.reset_conversation().await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(300)).await;
    assert_eq!(approval_stopped(&events), vec![(agent.clone(), false)]);
    assert!(!stopped_flag(&orchestrator, &agent).await);
    assert!(
        events.iter().any(|e| matches!(e, CoreEvent::ConversationCleared)),
        "切り替えの通知と一緒に出る: {events:?}"
    );
}
