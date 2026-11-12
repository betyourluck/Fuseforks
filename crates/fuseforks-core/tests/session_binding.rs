//! 会話を依頼の根に結び付ける（Spec 75 P1）の結合テスト。
//!
//! 見るのは**書き込み先**と**読み取り元**: 処理中に会話を切り替えても、その因果の発話・1 往復・
//! ターンの記録は依頼した会話へ書かれ（凍結 2）、開いていない会話のターンはその会話の保存先から
//! プロンプトを組む（凍結 4）。書かれた先は `export_session` の JSONL で読む（画面の投影ではなく
//! 保存先そのもの）。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fuseforks_core::event::CoreEvent;
use fuseforks_core::llm::{
    ChatMessage, ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, Role, ToolCall, Usage,
};
use fuseforks_core::model::{AgentId, AgentSpec, Endpoint, ModelTemplate};
use fuseforks_core::llm::retry::{HintSource, RetryClass};
use fuseforks_core::schedule::{Recurrence, ScheduleOptions, Weekday};
use fuseforks_core::schedule_probe::SessionMode;
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};
use tokio::sync::broadcast::Receiver;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "fuseforks-session-binding-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 最終 user 発話の印で振る舞いを変えるバックエンド。渡されたプロンプトを全部控える。
///
/// - `SLOW` — 400 ms 待ってから答える（飛行中のターンを作る）
/// - `FAIL` — 400 ms 待ってから失敗する
/// - `DELEGATE` — 400 ms 待ってから `ask_*`（提示されたもの）を呼ぶ。ツール結果が返った周は
///   `まとめ` と答える
/// - 要約の指示 — 400 ms 待ってから `要約本文` を返す
/// - それ以外 — `了解`
#[derive(Default)]
struct ScriptBackend {
    seen: std::sync::Mutex<Vec<Vec<ChatMessage>>>,
}

impl ScriptBackend {
    fn prompts(&self) -> Vec<Vec<ChatMessage>> {
        self.seen.lock().unwrap().clone()
    }
}

fn reply(text: &str) -> ChatResponse {
    ChatResponse {
        text: Some(text.to_owned()),
        tool_calls: Vec::new(),
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
        replay: None,
    }
}

#[async_trait::async_trait]
impl LlmBackend for ScriptBackend {
    fn name(&self) -> &str {
        "script"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.seen.lock().unwrap().push(req.messages.clone());
        let last = req.messages.last().map(|m| m.content.clone()).unwrap_or_default();
        let has_tool_result = req.messages.iter().any(|m| m.role == Role::Tool);
        if last.contains("要約の本文だけを出力してください") {
            tokio::time::sleep(Duration::from_millis(400)).await;
            return Ok(reply("要約本文"));
        }
        if last.contains("DELEGATE") && !has_tool_result {
            tokio::time::sleep(Duration::from_millis(400)).await;
            let ask = req
                .tools
                .iter()
                .find(|t| t.name.starts_with("ask_"))
                .map(|t| t.name.clone())
                .expect("ask_* が提示されること");
            let mut response = reply("");
            response.text = None;
            response.finish = Finish::ToolUse;
            response.tool_calls = vec![ToolCall {
                id: "call_1".into(),
                name: ask,
                args: serde_json::json!({ "message": "合言葉は？" }),
                extra: None,
            }];
            return Ok(response);
        }
        if has_tool_result {
            return Ok(reply("まとめ"));
        }
        if last.contains("FAIL") {
            tokio::time::sleep(Duration::from_millis(400)).await;
            return Err(LlmError::Api {
                status: 400,
                body: "失敗させた".into(),
                class: RetryClass::ClientError,
                code: None,
                retry_after: None,
                hint_src: HintSource::None,
            });
        }
        if last.contains("SLOW") {
            tokio::time::sleep(Duration::from_millis(400)).await;
            return Ok(reply("遅い答え"));
        }
        Ok(reply("了解"))
    }
}

async fn boot(dir: &TempDir, backend: Arc<ScriptBackend>) -> Orchestrator {
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(backend)),
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
    orchestrator
}

async fn start(orchestrator: &Orchestrator, id: &str, name: &str, connected: &[&str]) -> AgentId {
    let id = AgentId::from(id);
    let mut spec = AgentSpec::new(id.clone(), name, "tpl");
    spec.connected_agents = connected.iter().map(|c| AgentId::from(*c)).collect();
    orchestrator.create_agent(spec).await.unwrap();
    orchestrator.start_agent(&id).await.unwrap();
    id
}

async fn drain_until_quiet(rx: &mut Receiver<CoreEvent>, quiet: Duration) -> Vec<CoreEvent> {
    let mut events = Vec::new();
    while let Ok(Ok(event)) = tokio::time::timeout(quiet, rx.recv()).await {
        events.push(event);
    }
    events
}

/// 会話の保存先の記録（JSONL のヘッダを除いた行）。
async fn records(orchestrator: &Orchestrator, dir: &TempDir, session: &str) -> Vec<serde_json::Value> {
    let path = dir.0.join(format!("export-{session}.jsonl"));
    orchestrator.export_session(session, &path).await.unwrap();
    std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .skip(1)
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// その会話の発話の本文（`kind: message`）。
async fn message_texts(orchestrator: &Orchestrator, dir: &TempDir, session: &str) -> Vec<String> {
    records(orchestrator, dir, session)
        .await
        .iter()
        .filter(|r| r["kind"] == "message")
        .map(|r| r["content"].as_str().unwrap_or_default().to_owned())
        .collect()
}

fn kinds(records: &[serde_json::Value]) -> Vec<String> {
    records
        .iter()
        .map(|r| r["kind"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// 凍結 1・2 — 処理中に新規チャットを押しても、答え・1 往復・ターンの記録は依頼した会話へ。
/// 開いていない会話への発話は `MessageSent` を出さず `SessionUpdated` を出す（凍結 10）。
#[tokio::test]
async fn an_answer_stays_in_the_conversation_that_asked() {
    let dir = TempDir::new("stays");
    let backend = Arc::new(ScriptBackend::default());
    let orchestrator = boot(&dir, backend).await;
    let zari = start(&orchestrator, "agent_01", "ザリ", &[]).await;

    let first = orchestrator.current_session();
    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&zari, "SLOW で答えて").await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    orchestrator.reset_conversation().await.unwrap();
    let second = orchestrator.current_session();
    assert_ne!(first, second);
    let events = drain_until_quiet(&mut rx, Duration::from_millis(800)).await;

    let first_records = records(&orchestrator, &dir, &first).await;
    let first_kinds = kinds(&first_records);
    assert!(
        first_kinds.contains(&"exchange".to_owned()) && first_kinds.contains(&"turn".to_owned()),
        "1 往復とターンが元の会話に: {first_records:?}"
    );
    let texts = message_texts(&orchestrator, &dir, &first).await;
    assert!(texts.iter().any(|t| t.contains("SLOW で答えて")), "{texts:?}");
    assert!(texts.iter().any(|t| t.contains("遅い答え")), "答えも元の会話に: {texts:?}");
    assert!(
        records(&orchestrator, &dir, &second).await.is_empty(),
        "新しい会話には何も書かれない"
    );
    assert!(
        orchestrator.message_log(None).await.is_empty(),
        "画面の会話ログ（開いている会話の写し）にも出ない"
    );

    let answer_sent = events.iter().any(|e| {
        matches!(e, CoreEvent::MessageSent { message } if message.content.contains("遅い答え"))
    });
    assert!(!answer_sent, "開いていない会話への答えで MessageSent を出さない");
    assert!(
        events.iter().any(|e| matches!(
            e,
            CoreEvent::SessionUpdated { session_id, from: Endpoint::Agent { .. } } if *session_id == first
        )),
        "SessionUpdated が元の会話の ID で出る: {events:?}"
    );
    let _ = orchestrator.stop_agent(&zari).await;
}

/// 凍結 2・4 — 新しい会話は前の依頼を知る個体を持たずに始まり、元の会話を開くと戻る。
/// 処理中でも既存の会話を開ける（凍結 11。`SESSION_SWITCH_BLOCKED` は撤去）。
#[tokio::test]
async fn the_new_conversation_forgets_and_reopening_remembers() {
    let dir = TempDir::new("forgets");
    let backend = Arc::new(ScriptBackend::default());
    let orchestrator = boot(&dir, Arc::clone(&backend)).await;
    let zari = start(&orchestrator, "agent_01", "ザリ", &[]).await;

    let first = orchestrator.current_session();
    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&zari, "SLOW 合言葉はブラボー").await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    orchestrator.reset_conversation().await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(800)).await;

    orchestrator.send_user_message(&zari, "新しい話").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    let in_new = backend.prompts().last().unwrap().clone();
    assert!(
        !in_new.iter().any(|m| m.content.contains("ブラボー")),
        "新しい会話のプロンプトに前の依頼が入らない: {in_new:?}"
    );

    // 処理中でも開ける。
    orchestrator.send_user_message(&zari, "SLOW もう一度").await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    orchestrator
        .resume_session(&first)
        .await
        .expect("処理中でも既存の会話を開けること");
    drain_until_quiet(&mut rx, Duration::from_millis(800)).await;

    orchestrator.send_user_message(&zari, "覚えてる？").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    let reopened = backend.prompts().last().unwrap().clone();
    assert!(
        reopened.iter().any(|m| m.content.contains("ブラボー")),
        "元の会話を開くと 1 往復が戻る: {reopened:?}"
    );
    assert!(
        !reopened.iter().any(|m| m.content.contains("もう一度")),
        "開いた後に終わった別の会話のターンは、開いた会話の履歴へ入らない"
    );
    let _ = orchestrator.stop_agent(&zari).await;
}

/// 凍結 4 — 切り替えの後に届いた委譲は、依頼した会話の履歴でプロンプトを組み、答えもその会話へ。
#[tokio::test]
async fn a_delegation_after_the_switch_reads_the_asking_conversation() {
    let dir = TempDir::new("delegate");
    let backend = Arc::new(ScriptBackend::default());
    let orchestrator = boot(&dir, Arc::clone(&backend)).await;
    let gemmy = start(&orchestrator, "agent_02", "ジェミー", &[]).await;
    let zari = start(&orchestrator, "agent_01", "ザリ", &["agent_02"]).await;

    let first = orchestrator.current_session();
    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&gemmy, "合言葉はブラボー").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(300)).await;

    orchestrator.send_user_message(&zari, "DELEGATE してきて").await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    orchestrator.reset_conversation().await.unwrap();
    let second = orchestrator.current_session();
    drain_until_quiet(&mut rx, Duration::from_millis(900)).await;

    let delegated = backend
        .prompts()
        .into_iter()
        .find(|p| p.last().is_some_and(|m| m.content.contains("合言葉は？")))
        .expect("委譲のターンが走ること");
    assert!(
        delegated.iter().any(|m| m.content.contains("ブラボー")),
        "委譲先は依頼した会話の履歴で考える: {delegated:?}"
    );
    let texts = message_texts(&orchestrator, &dir, &first).await;
    assert!(texts.iter().any(|t| t.contains("合言葉は？")), "依頼の発話は元の会話: {texts:?}");
    assert!(texts.iter().any(|t| t.contains("まとめ")), "束ねた答えも元の会話: {texts:?}");
    assert!(records(&orchestrator, &dir, &second).await.is_empty());
    let _ = orchestrator.stop_agent(&zari).await;
    let _ = orchestrator.stop_agent(&gemmy).await;
}

/// 凍結 7 — 要約の途中で会話を切り替えても、要約は始めた会話へ書かれ、開いた会話は畳まれない。
#[tokio::test]
async fn a_summary_mid_switch_lands_where_it_started() {
    let dir = TempDir::new("summary");
    let backend = Arc::new(ScriptBackend::default());
    let orchestrator = Arc::new(boot(&dir, Arc::clone(&backend)).await);
    let zari = start(&orchestrator, "agent_01", "ザリ", &[]).await;

    let first = orchestrator.current_session();
    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&zari, "最初の話").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(300)).await;

    let summarizing = {
        let orchestrator = Arc::clone(&orchestrator);
        tokio::spawn(async move { orchestrator.summarize_session().await })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    orchestrator.reset_conversation().await.unwrap();
    let second = orchestrator.current_session();
    orchestrator.send_user_message(&zari, "新しい話").await.unwrap();
    assert_eq!(summarizing.await.unwrap().unwrap(), 1);
    drain_until_quiet(&mut rx, Duration::from_millis(300)).await;

    assert!(kinds(&records(&orchestrator, &dir, &first).await).contains(&"summary".to_owned()));
    let second_records = records(&orchestrator, &dir, &second).await;
    assert!(
        !kinds(&second_records).contains(&"summary".to_owned()),
        "開いた会話に要約は書かれない: {second_records:?}"
    );

    orchestrator.send_user_message(&zari, "続き").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(300)).await;
    let prompt = backend.prompts().last().unwrap().clone();
    assert!(
        prompt.iter().any(|m| m.content.contains("新しい話")),
        "開いた会話の履歴は畳まれていない: {prompt:?}"
    );
    assert!(!prompt.iter().any(|m| m.content.contains("要約本文")));
    let _ = orchestrator.stop_agent(&zari).await;
}

/// 凍結 9 — 村全体の知らせは書いた時点で開いている会話へ。
#[tokio::test]
async fn a_village_notice_goes_to_the_open_conversation() {
    let dir = TempDir::new("notice");
    let backend = Arc::new(ScriptBackend::default());
    let orchestrator = boot(&dir, backend).await;
    let zari = start(&orchestrator, "agent_01", "ザリ", &[]).await;

    let first = orchestrator.current_session();
    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&zari, "SLOW で").await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    orchestrator.reset_conversation().await.unwrap();
    let second = orchestrator.current_session();
    let _luna = start(&orchestrator, "agent_03", "ルナ", &[]).await;
    drain_until_quiet(&mut rx, Duration::from_millis(800)).await;

    let in_second = message_texts(&orchestrator, &dir, &second).await;
    assert!(
        in_second.iter().any(|t| t.contains("agent_03")),
        "入退室は開いている会話へ: {in_second:?}"
    );
    let in_first = message_texts(&orchestrator, &dir, &first).await;
    assert!(!in_first.iter().any(|t| t.contains("agent_03")), "{in_first:?}");
    let _ = orchestrator.stop_agent(&zari).await;
}

/// 凍結 11 — 処理（飛行中のターン・受信箱で待っている発話）が残っている会話は、開いているかを
/// 問わず消せない。残っていなければ消せる。
#[tokio::test]
async fn a_busy_conversation_cannot_be_deleted() {
    let dir = TempDir::new("busy");
    let backend = Arc::new(ScriptBackend::default());
    let orchestrator = boot(&dir, backend).await;
    let zari = start(&orchestrator, "agent_01", "ザリ", &[]).await;

    let first = orchestrator.current_session();
    let mut rx = orchestrator.subscribe();
    // 1 通目が飛行中、2 通目は受信箱で待つ。
    orchestrator.send_user_message(&zari, "SLOW 1").await.unwrap();
    orchestrator.send_user_message(&zari, "SLOW 2").await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 開いている会話でも断る。
    let refused = orchestrator.delete_session(&first).await.expect_err("処理中は消せない");
    assert_eq!(fuseforks_core::ErrorPayload::from(&refused).code, "SESSION_BUSY");

    orchestrator.reset_conversation().await.unwrap();
    // 1 通目が終わり 2 通目が走っている間も、開いていない会話として断る。
    tokio::time::sleep(Duration::from_millis(450)).await;
    let refused = orchestrator.delete_session(&first).await.expect_err("受信箱の発話も数える");
    assert_eq!(fuseforks_core::ErrorPayload::from(&refused).code, "SESSION_BUSY");

    drain_until_quiet(&mut rx, Duration::from_millis(800)).await;
    orchestrator
        .delete_session(&first)
        .await
        .expect("処理が終われば消せること");
    let _ = orchestrator.stop_agent(&zari).await;
}

/// 凍結 2 — 失敗したターンの 1 往復は、そのターンの会話へ書かれる。
#[tokio::test]
async fn a_failed_turn_is_recorded_in_its_own_conversation() {
    let dir = TempDir::new("failed");
    let backend = Arc::new(ScriptBackend::default());
    let orchestrator = boot(&dir, backend).await;
    let zari = start(&orchestrator, "agent_01", "ザリ", &[]).await;

    let first = orchestrator.current_session();
    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&zari, "FAIL して").await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    orchestrator.reset_conversation().await.unwrap();
    let second = orchestrator.current_session();
    drain_until_quiet(&mut rx, Duration::from_millis(800)).await;

    let first_kinds = kinds(&records(&orchestrator, &dir, &first).await);
    assert!(first_kinds.contains(&"exchange".to_owned()), "{first_kinds:?}");
    assert!(first_kinds.contains(&"turn".to_owned()), "{first_kinds:?}");
    // 失敗で個体が止まった知らせ（村全体の知らせ — 凍結 9）だけは開いている会話へ行く。
    let second_kinds = kinds(&records(&orchestrator, &dir, &second).await);
    assert!(
        !second_kinds.contains(&"exchange".to_owned()) && !second_kinds.contains(&"turn".to_owned()),
        "失敗したターンの記録は開いている会話へ書かれない: {second_kinds:?}"
    );
    let _ = orchestrator.stop_agent(&zari).await;
}

const THU_17: Recurrence = Recurrence::Weekly {
    weekday: Weekday::Thu,
    hour: 17,
    minute: 0,
};

fn jst_thu_17() -> chrono::DateTime<chrono::FixedOffset> {
    use chrono::TimeZone;
    chrono::FixedOffset::east_opt(9 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 7, 30, 17, 0, 29)
        .single()
        .unwrap()
}

async fn schedule(orchestrator: &Orchestrator, to: &AgentId, message: &str, mode: SessionMode) {
    orchestrator
        .create_schedule(
            to.clone(),
            message.to_owned(),
            THU_17,
            ScheduleOptions {
                session_mode: mode,
                ..ScheduleOptions::default()
            },
        )
        .await
        .unwrap();
}

/// 凍結 14 — `fresh` の予定は画面を切り替えず、予定ごとに作った会話へ書く。同じ tick の
/// `continue` は開いている会話へ。
#[tokio::test]
async fn fresh_schedules_write_to_their_own_conversations_without_switching() {
    let dir = TempDir::new("fresh");
    let backend = Arc::new(ScriptBackend::default());
    let orchestrator = boot(&dir, backend).await;
    let a = start(&orchestrator, "agent_01", "ザリ", &[]).await;
    let b = start(&orchestrator, "agent_02", "ジェミー", &[]).await;
    let c = start(&orchestrator, "agent_03", "ルナ", &[]).await;
    schedule(&orchestrator, &a, "fresh の 1", SessionMode::Fresh).await;
    schedule(&orchestrator, &b, "fresh の 2", SessionMode::Fresh).await;
    schedule(&orchestrator, &c, "continue の 1", SessionMode::Continue).await;

    let open = orchestrator.current_session();
    let before: Vec<String> = orchestrator
        .list_sessions()
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    let mut rx = orchestrator.subscribe();
    orchestrator.run_schedule_tick(jst_thu_17()).await;
    let events = drain_until_quiet(&mut rx, Duration::from_millis(500)).await;

    assert_eq!(orchestrator.current_session(), open, "開いている会話は動かない");
    assert!(
        !events.iter().any(|e| matches!(e, CoreEvent::SessionSwitched { .. })),
        "SessionSwitched が出ない"
    );
    let created: Vec<String> = orchestrator
        .list_sessions()
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .filter(|id| !before.contains(id))
        .collect();
    assert_eq!(created.len(), 2, "fresh の予定ごとに会話が 1 つずつできる: {created:?}");
    for id in &created {
        let texts = message_texts(&orchestrator, &dir, id).await;
        assert!(texts.iter().any(|t| t.contains("fresh の")), "{texts:?}");
        assert!(texts.iter().any(|t| t.contains("了解")), "答えも作った会話へ: {texts:?}");
        assert!(!texts.iter().any(|t| t.contains("continue の")), "{texts:?}");
    }
    let open_texts = message_texts(&orchestrator, &dir, &open).await;
    assert!(open_texts.iter().any(|t| t.contains("continue の 1")), "{open_texts:?}");
    assert!(!open_texts.iter().any(|t| t.contains("fresh の")), "{open_texts:?}");
    // 画面の会話ログ（開いている会話のメモリの写し）にも fresh の発話は入らない（凍結 2）。
    let shown: Vec<String> = orchestrator
        .message_log(None)
        .await
        .into_iter()
        .map(|m| m.content)
        .collect();
    assert!(!shown.iter().any(|t| t.contains("fresh の")), "{shown:?}");
    assert!(
        !events.iter().any(|e| matches!(
            e,
            CoreEvent::MessageSent { message } if message.content.contains("fresh の")
        )),
        "fresh の発話で MessageSent を出さない"
    );
    for id in [&a, &b, &c] {
        let _ = orchestrator.stop_agent(id).await;
    }
}
