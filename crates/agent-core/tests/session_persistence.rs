//! 会話の永続化の結合テスト（Spec 12 P2 — 配線）。
//!
//! P1 の単体テストは `session_store.rs` の中で保存先の機構だけを見ている。
//! ここで見るのは**配線**: 書き込み点が繋がっているか、再起動で戻るか、
//! 切り替えが契約どおりに拒否されるか。
//!
//! **本丸は画面ではなく履歴**（Spec 12 の S1）。「会話ペインに前回の会話が出る」
//! だけでは通さない — 再起動後の最初のターンで **LLM へ渡ったリクエストの
//! `messages` に復元した履歴が入っていること**を見る。会話ログだけ戻っていて
//! 全員が健忘症で始まる状態は、画面上この状態と区別が付かない。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_core::event::CoreEvent;
use agent_core::llm::{
    ChatMessage, ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, Role, Usage,
};
use agent_core::model::{AgentId, AgentSpec, ModelTemplate};
use agent_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};
use tokio::sync::broadcast::Receiver;

/// テスト用の一時ディレクトリ。終了時に破棄する。
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "concordia-session-it-{tag}-{}",
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

/// 渡されたプロンプトを丸ごと控えるバックエンド。
///
/// 復元した履歴が**実際にモデルへ渡ったか**は、ここでしか見えない。
#[derive(Default)]
struct RecordingBackend {
    seen: std::sync::Mutex<Vec<Vec<ChatMessage>>>,
}

#[async_trait::async_trait]
impl LlmBackend for RecordingBackend {
    fn name(&self) -> &str {
        "recording"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.seen.lock().unwrap().push(req.messages.clone());
        Ok(ChatResponse {
            text: Some("了解".into()),
            tool_calls: Vec::new(),
            finish: Finish::Stop,
            usage: Usage {
                prompt: 1,
                completion: 1,
                cache_read: 0,
            },
            grounding: Default::default(),
        })
    }
}

/// 要約の呼び出しだけ別の本文を返すバックエンド（Spec 12 P4）。
///
/// 要約かどうかは**最終 user 発話の末尾**で見分ける。要約の指示文にしか
/// 現れない一文を鍵にする — 「覚え書き」のような短い語を鍵にすると、
/// 要約本文そのものを差した通常ターンまで要約と誤判定する（実際に踏んだ）。
struct SummarizingBackend {
    seen: std::sync::Mutex<Vec<Vec<ChatMessage>>>,
    /// 要約として返す本文。`None` を入れると「要約に失敗した」経路を再現する。
    summary: Option<&'static str>,
}

impl SummarizingBackend {
    fn new(summary: Option<&'static str>) -> Self {
        Self {
            seen: std::sync::Mutex::new(Vec::new()),
            summary,
        }
    }

    /// 要約以外で渡されたプロンプトだけを返す（通常ターンの検査用）。
    fn turns(&self) -> Vec<Vec<ChatMessage>> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|messages| !is_summary_request(messages))
            .cloned()
            .collect()
    }
}

fn is_summary_request(messages: &[ChatMessage]) -> bool {
    messages
        .last()
        .is_some_and(|m| m.content.contains("要約の本文だけを出力してください"))
}

#[async_trait::async_trait]
impl LlmBackend for SummarizingBackend {
    fn name(&self) -> &str {
        "summarizing"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let summarizing = is_summary_request(&req.messages);
        self.seen.lock().unwrap().push(req.messages.clone());
        let text = if summarizing {
            self.summary.map(str::to_owned)
        } else {
            Some("了解".to_owned())
        };
        Ok(ChatResponse {
            text,
            tool_calls: Vec::new(),
            finish: Finish::Stop,
            usage: Usage {
                prompt: 1,
                completion: 1,
                cache_read: 0,
            },
            grounding: Default::default(),
        })
    }
}

/// 応答に時間の掛かるバックエンド。飛行中ターンを作るために使う。
struct SlowBackend;

#[async_trait::async_trait]
impl LlmBackend for SlowBackend {
    fn name(&self) -> &str {
        "slow"
    }

    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, LlmError> {
        tokio::time::sleep(Duration::from_millis(300)).await;
        Ok(ChatResponse {
            text: Some("遅い応答".into()),
            tool_calls: Vec::new(),
            finish: Finish::Stop,
            usage: Usage {
                prompt: 1,
                completion: 1,
                cache_read: 0,
            },
            grounding: Default::default(),
        })
    }
}

/// 同じワークスペースでオーケストレーターを組む（2 回目以降は「再起動」）。
async fn boot(dir: &TempDir, backend: Arc<dyn LlmBackend>) -> Orchestrator {
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(backend)),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig::default(),
    )
    .await
    .expect("bootstrap できること");

    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();

    orchestrator
}

/// アプリを閉じる。**エージェントを止めてから落とす** — 走っているタスクが
/// `Shared` を握っている間は保存先のファイルも開いたままで、次の起動が
/// 「既に開かれている」で失敗する。
async fn shutdown(orchestrator: Orchestrator, agents: &[AgentId]) {
    for id in agents {
        let _ = orchestrator.stop_agent(id).await;
    }
    drop(orchestrator);
    tokio::task::yield_now().await;
}

/// 一定時間静かになるまでイベントを集める。
async fn drain_until_quiet(rx: &mut Receiver<CoreEvent>, quiet: Duration) -> Vec<CoreEvent> {
    let mut events = Vec::new();
    while let Ok(Ok(event)) = tokio::time::timeout(quiet, rx.recv()).await {
        events.push(event);
    }
    events
}

/// エージェントを 1 体作って起動する。
async fn start_agent(orchestrator: &Orchestrator, id: &AgentId) {
    if orchestrator.snapshot(id).await.is_err() {
        orchestrator
            .create_agent(AgentSpec::new(id.clone(), "PlannerAgent", "tpl"))
            .await
            .unwrap();
    }
    orchestrator.start_agent(id).await.unwrap();
}

/// **本丸**: 再起動後の最初のターンで、復元した履歴がモデルへ渡ること。
///
/// 画面に出ているかは見ない。見るのは `ChatRequest.messages` —
/// 会話ログだけ戻して全員が健忘症で始まる状態と、正しく戻った状態は
/// 画面上区別が付かないため。
#[tokio::test]
async fn a_restarted_village_sends_the_restored_history_to_the_model() {
    let dir = TempDir::new("restart");
    let id = AgentId::from("agent_01");

    // 1 回目の起動。1 往復して閉じる。
    {
        let backend = Arc::new(RecordingBackend::default());
        let orchestrator = boot(&dir, backend).await;
        start_agent(&orchestrator, &id).await;
        let mut rx = orchestrator.subscribe();
        orchestrator
            .send_user_message(&id, "最初の依頼です")
            .await
            .unwrap();
        drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
        shutdown(orchestrator, std::slice::from_ref(&id)).await;
    }

    // 2 回目の起動 = 再起動。
    let backend = Arc::new(RecordingBackend::default());
    let orchestrator = boot(&dir, Arc::clone(&backend) as Arc<dyn LlmBackend>).await;

    assert!(
        !orchestrator.message_log(None).await.is_empty(),
        "会話ペインへ戻す会話ログも復元されていること"
    );

    start_agent(&orchestrator, &id).await;
    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "続きです").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let first_turn = {
        let seen = backend.seen.lock().unwrap();
        seen.first().expect("再起動後に 1 回は呼ばれること").clone()
    };
    assert!(
        first_turn
            .iter()
            .any(|m| m.role == Role::User && m.content.contains("最初の依頼です")),
        "再開前の依頼が履歴として渡ること: {first_turn:#?}"
    );
    assert!(
        first_turn
            .iter()
            .any(|m| m.role == Role::Assistant && m.content.contains("了解")),
        "再開前の自分の応答も渡ること（往復の対を崩さない）: {first_turn:#?}"
    );

    shutdown(orchestrator, &[id]).await;
}

/// 送った文字列と保存された文字列が一致すること（failures.md #45 の保存側）。
///
/// 履歴には**送った形そのもの**が入る（可変文脈を畳んだ後の文字列）。
/// 保存側が畳む前の文字列を持つと、再開後のプロンプトが保存前と食い違い、
/// キャッシュの前方一致もその位置で切れる。
#[tokio::test]
async fn the_saved_exchange_matches_the_string_that_was_actually_sent() {
    let dir = TempDir::new("sent-matches");
    let id = AgentId::from("agent_01");
    let backend = Arc::new(RecordingBackend::default());
    let orchestrator = boot(&dir, Arc::clone(&backend) as Arc<dyn LlmBackend>).await;

    start_agent(&orchestrator, &id).await;
    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&id, "送信と保存の一致を見る")
        .await
        .unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let sent = {
        let seen = backend.seen.lock().unwrap();
        seen.first()
            .expect("1 回は呼ばれること")
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .expect("最終 user 発話があること")
            .content
            .clone()
    };

    let export = dir.0.join("export.jsonl");
    let session_id = orchestrator.current_session();
    orchestrator
        .export_session(&session_id, &export)
        .await
        .expect("書き出せること");

    let text = std::fs::read_to_string(&export).unwrap();
    let saved: Vec<serde_json::Value> = text
        .lines()
        .skip(1) // 1 行目はセッションのヘッダ。
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();

    let exchange = saved
        .iter()
        .find(|line| line["kind"] == "exchange")
        .expect("exchange が保存されていること");
    assert_eq!(
        exchange["sent"].as_str().unwrap(),
        sent,
        "保存された文字列は、実際に送った文字列そのもの"
    );
    assert_eq!(exchange["replied"], "了解");
    assert_eq!(exchange["agentId"], "agent_01");

    // 会話ログ側も同じ書き出しに載る（2 層が両方保存されている）。
    let messages: Vec<&serde_json::Value> =
        saved.iter().filter(|line| line["kind"] == "message").collect();
    assert!(
        messages
            .iter()
            .any(|m| m["content"] == "送信と保存の一致を見る"),
        "会話ログも保存されること: {messages:#?}"
    );

    shutdown(orchestrator, &[id]).await;
}

/// 新規チャットは**捨てない**。前の会話はディスクに残り、開き直せる。
#[tokio::test]
async fn a_new_chat_keeps_the_previous_conversation_and_can_return_to_it() {
    let dir = TempDir::new("new-chat");
    let id = AgentId::from("agent_01");
    let orchestrator = boot(&dir, Arc::new(RecordingBackend::default())).await;

    start_agent(&orchestrator, &id).await;
    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "前の会話").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let first = orchestrator.current_session();
    assert_eq!(orchestrator.list_sessions().await.unwrap().len(), 1);

    orchestrator.reset_conversation().await.unwrap();
    let second = orchestrator.current_session();

    assert_ne!(first, second, "新しいセッションが開くこと");
    assert_eq!(
        orchestrator.list_sessions().await.unwrap().len(),
        2,
        "前の会話は一覧に残ること"
    );
    assert!(
        orchestrator.message_log(None).await.is_empty(),
        "画面は空になること"
    );

    orchestrator.resume_session(&first).await.unwrap();
    assert_eq!(orchestrator.current_session(), first);
    assert!(
        orchestrator
            .message_log(None)
            .await
            .iter()
            .any(|m| m.content == "前の会話"),
        "戻れること"
    );

    shutdown(orchestrator, &[id]).await;
}

/// 表題は最初のユーザー発話から自動生成され、一覧に出ること。
#[tokio::test]
async fn the_session_title_comes_from_the_first_user_message() {
    let dir = TempDir::new("title");
    let id = AgentId::from("agent_01");
    let orchestrator = boot(&dir, Arc::new(RecordingBackend::default())).await;

    start_agent(&orchestrator, &id).await;
    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&id, "黒板の運用を決めたい")
        .await
        .unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let listed = orchestrator.list_sessions().await.unwrap();
    assert_eq!(listed[0].meta.title, "黒板の運用を決めたい");
    assert!(listed[0].meta.record_count >= 3, "message 2 件 + exchange 1 件以上");

    shutdown(orchestrator, &[id]).await;
}

/// `fork` は元を不変のまま残し、複製した側を開く。
#[tokio::test]
async fn fork_opens_the_copy_and_leaves_the_source_untouched() {
    let dir = TempDir::new("fork");
    let id = AgentId::from("agent_01");
    let orchestrator = boot(&dir, Arc::new(RecordingBackend::default())).await;

    start_agent(&orchestrator, &id).await;
    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "1 番目").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    orchestrator.send_user_message(&id, "2 番目").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let source = orchestrator.current_session();
    let source_count = orchestrator
        .list_sessions()
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.id == source)
        .unwrap()
        .meta
        .record_count;

    // 「2 番目」を出す**直前**へ戻る。境界の計算はコア（fork_points）に任せる —
    // UI が seq を自分で引き算する形にすると、境界の意味が 2 箇所に散る。
    let point = orchestrator
        .list_fork_points(&source)
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.text == "2 番目")
        .expect("2 番目が分岐点として出ること");
    assert_eq!(
        point.to.as_ref().map(|a| a.as_str()),
        Some("agent_01"),
        "差し戻す文面の宛先も返ること"
    );

    let forked = orchestrator.fork_session(&source, point.at_seq).await.unwrap();

    assert_eq!(orchestrator.current_session(), forked, "複製した側が開くこと");
    let log = orchestrator.message_log(None).await;
    assert!(
        log.iter().any(|m| m.content == "1 番目"),
        "直前までの会話は載ること: {log:#?}"
    );
    assert!(
        !log.iter().any(|m| m.content == "2 番目"),
        "選んだ依頼は複製先に残らないこと（入力欄へ差し戻す）: {log:#?}"
    );

    let after = orchestrator
        .list_sessions()
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.id == source)
        .unwrap()
        .meta;
    assert_eq!(after.record_count, source_count, "元は不変");
    assert_eq!(after.parent_id, None, "元は分岐元を持たない");

    shutdown(orchestrator, &[id]).await;
}

/// 切り替えの拒否は**既存の会話を開くときだけ**（Spec 12 の不変条件 11）。
///
/// 新規チャットは飛行中でも通る — 着地先が新しい空の会話なので、既存の記録を
/// 汚さない（Spec 03 の案 A は改訂後もそのまま生きる）。
#[tokio::test]
async fn resuming_is_refused_mid_turn_but_a_new_chat_is_not() {
    let dir = TempDir::new("switch-busy");
    let id = AgentId::from("agent_01");
    let orchestrator = boot(&dir, Arc::new(SlowBackend)).await;

    start_agent(&orchestrator, &id).await;
    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "最初").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let first = orchestrator.current_session();
    orchestrator.reset_conversation().await.unwrap();
    let second = orchestrator.current_session();

    // 飛行中ターンを作る。
    orchestrator.send_user_message(&id, "考えて").await.unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;

    let refused = orchestrator.resume_session(&first).await;
    let payload = agent_core::ErrorPayload::from(&refused.expect_err("拒否されること"));
    assert_eq!(payload.code, "SESSION_SWITCH_BLOCKED");
    assert_eq!(
        orchestrator.current_session(),
        second,
        "拒否されたら開いている会話は動かない"
    );

    // 新規チャットは同じ状況でも通る。
    orchestrator
        .create_session()
        .await
        .expect("新規チャットは飛行中でも通ること");

    drain_until_quiet(&mut rx, Duration::from_millis(600)).await;
    shutdown(orchestrator, &[id]).await;
}

/// 切り替えは `conversationCleared` → `sessionSwitched` の順で 2 本出す。
///
/// `conversationCleared` を出さない選択は採らない — 会話ペインを空にする指示は
/// これが唯一の経路で、意味を変えると既存 UI が誤動作する。
#[tokio::test]
async fn switching_emits_cleared_then_switched_in_that_order() {
    let dir = TempDir::new("events");
    let orchestrator = boot(&dir, Arc::new(RecordingBackend::default())).await;

    let mut rx = orchestrator.subscribe();
    orchestrator.reset_conversation().await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(200)).await;

    let order: Vec<&CoreEvent> = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                CoreEvent::ConversationCleared | CoreEvent::SessionSwitched { .. }
            )
        })
        .collect();

    assert_eq!(order.len(), 2, "2 本とも出ること: {events:#?}");
    assert!(matches!(order[0], CoreEvent::ConversationCleared));
    match order[1] {
        CoreEvent::SessionSwitched { session_id } => {
            assert_eq!(*session_id, orchestrator.current_session());
        }
        other => panic!("2 本目は sessionSwitched のはず: {other:#?}"),
    }

    shutdown(orchestrator, &[]).await;
}

/// 開いている会話を消したら、次の会話へ切り替わる。
///
/// 消したまま開きっぱなしにすると、以後の発話が行き先の無いセッションへ
/// 書かれ続ける。
#[tokio::test]
async fn deleting_the_open_session_switches_to_another_one() {
    let dir = TempDir::new("delete");
    let orchestrator = boot(&dir, Arc::new(RecordingBackend::default())).await;

    let first = orchestrator.current_session();
    orchestrator.reset_conversation().await.unwrap();
    let second = orchestrator.current_session();

    orchestrator.delete_session(&second).await.unwrap();

    assert_eq!(
        orchestrator.current_session(),
        first,
        "残っている会話へ切り替わること"
    );
    let listed = orchestrator.list_sessions().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, first);

    // 最後の 1 本を消したら、新しい会話が生まれる（開いていない状態を作らない）。
    orchestrator.delete_session(&first).await.unwrap();
    let now = orchestrator.current_session();
    assert!(!now.is_empty());
    assert_ne!(now, first);
    assert_eq!(orchestrator.list_sessions().await.unwrap().len(), 1);

    shutdown(orchestrator, &[]).await;
}

/// **要約の本丸**: 要約した後のターンで、要約がモデルへ渡ること（Spec 12 P4）。
///
/// 見るのは画面ではなく `ChatRequest.messages`。要約は**履歴の席を持たず**、
/// 可変文脈の畳みへ相乗りする — だから畳んだ結果の user 発話の中に現れる。
#[tokio::test]
async fn a_summary_is_injected_into_the_next_turn_and_folds_the_history() {
    let dir = TempDir::new("summarize");
    let id = AgentId::from("agent_01");
    let backend = Arc::new(SummarizingBackend::new(Some("・アルファという合言葉を預かった")));
    let orchestrator = boot(&dir, Arc::clone(&backend) as Arc<dyn LlmBackend>).await;

    start_agent(&orchestrator, &id).await;
    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&id, "合言葉はアルファ")
        .await
        .unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    assert_eq!(
        orchestrator.summarize_session().await.unwrap(),
        1,
        "履歴を持つサーヴァント 1 体が要約されること"
    );

    orchestrator.send_user_message(&id, "続きです").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let turns = backend.turns();
    let after = turns.last().expect("要約後のターンがあること");
    let folded = after
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .expect("最終 user 発話があること");
    assert!(
        folded.content.contains("・アルファという合言葉を預かった"),
        "要約が可変文脈の畳みに乗ること: {folded:#?}"
    );
    assert!(
        !after
            .iter()
            .any(|m| m.role == Role::Assistant && m.content == "了解"),
        "覆われた往復は履歴から畳まれること（要約が肩代わりする）: {after:#?}"
    );

    // 「coversUpToSeq < 自身の seq」— 自分自身を覆う要約を作らない。
    let export = dir.0.join("summarized.jsonl");
    let session_id = orchestrator.current_session();
    orchestrator.export_session(&session_id, &export).await.unwrap();
    let text = std::fs::read_to_string(&export).unwrap();
    let summary = text
        .lines()
        .skip(1)
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .find(|line| line["kind"] == "summary")
        .expect("summary レコードが積まれること");
    assert!(
        summary["coversUpToSeq"].as_u64().unwrap() < summary["seq"].as_u64().unwrap(),
        "自分自身を覆う要約を作らない: {summary}"
    );
    assert_eq!(summary["agentId"], "agent_01");

    shutdown(orchestrator, std::slice::from_ref(&id)).await;
}

/// 要約は再起動をまたいで効く（元のレコードは消えていない）。
#[tokio::test]
async fn a_summary_survives_a_restart() {
    let dir = TempDir::new("summarize-restart");
    let id = AgentId::from("agent_01");

    {
        let backend = Arc::new(SummarizingBackend::new(Some("・前回の依頼を預かった")));
        let orchestrator = boot(&dir, backend).await;
        start_agent(&orchestrator, &id).await;
        let mut rx = orchestrator.subscribe();
        orchestrator.send_user_message(&id, "最初の依頼").await.unwrap();
        drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
        orchestrator.summarize_session().await.unwrap();
        shutdown(orchestrator, std::slice::from_ref(&id)).await;
    }

    let backend = Arc::new(SummarizingBackend::new(Some("・二度目")));
    let orchestrator = boot(&dir, Arc::clone(&backend) as Arc<dyn LlmBackend>).await;
    start_agent(&orchestrator, &id).await;
    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "続きです").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let turns = backend.turns();
    let first = turns.first().expect("再起動後に 1 回は呼ばれること");
    let folded = first
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .expect("最終 user 発話があること");
    assert!(
        folded.content.contains("・前回の依頼を預かった"),
        "要約が復元されて次のターンへ差されること: {folded:#?}"
    );
    assert!(
        !first
            .iter()
            .any(|m| m.role == Role::User && m.content.contains("最初の依頼")),
        "要約が覆った往復は復元しない（最新 summary + それ以降の exchange）: {first:#?}"
    );

    shutdown(orchestrator, std::slice::from_ref(&id)).await;
}

/// 要約が空で返ったら**履歴を畳まない**。
///
/// 要約に失敗した代償が履歴の喪失になるのは最悪の交換。
#[tokio::test]
async fn an_empty_summary_leaves_the_history_alone() {
    let dir = TempDir::new("summarize-empty");
    let id = AgentId::from("agent_01");
    let backend = Arc::new(SummarizingBackend::new(None));
    let orchestrator = boot(&dir, Arc::clone(&backend) as Arc<dyn LlmBackend>).await;

    start_agent(&orchestrator, &id).await;
    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "覚えておいて").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    assert_eq!(
        orchestrator.summarize_session().await.unwrap(),
        0,
        "空の要約は成功として数えない"
    );

    orchestrator.send_user_message(&id, "続きです").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let turns = backend.turns();
    let after = turns.last().expect("要約後のターンがあること");
    assert!(
        after
            .iter()
            .any(|m| m.role == Role::User && m.content.contains("覚えておいて")),
        "履歴はそのまま残ること: {after:#?}"
    );

    shutdown(orchestrator, std::slice::from_ref(&id)).await;
}

/// **停止中のサーヴァントは要約しない**（2026-08-03 の実機指摘）。
///
/// 要約の目的は以後のプロンプトを短くすることで、停止中の個体には以後のターンが
/// 無い。それでも呼べば、参加していない個体のぶんまで押した人がトークンを払う。
/// 履歴は停止しても消えないので、起動してから押せばそのとき要約される。
#[tokio::test]
async fn only_running_servants_are_summarized() {
    let dir = TempDir::new("summarize-stopped");
    let id = AgentId::from("agent_01");
    let backend = Arc::new(SummarizingBackend::new(Some("・要約されたはず")));
    let orchestrator = boot(&dir, Arc::clone(&backend) as Arc<dyn LlmBackend>).await;

    start_agent(&orchestrator, &id).await;
    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "覚えておいて").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    // 稼働を降ろす。**履歴は残る**（Spec 12 P2 で start_agent のクリアをやめた）。
    orchestrator.stop_agent(&id).await.unwrap();

    assert_eq!(
        orchestrator.summarize_session().await.unwrap(),
        0,
        "停止中のサーヴァントは要約しない"
    );
    let calls = backend.seen.lock().unwrap().len();

    // 起動し直せば、同じ履歴がそのまま要約の対象になる（取り逃がしにはならない）。
    orchestrator.start_agent(&id).await.unwrap();
    assert_eq!(
        orchestrator.summarize_session().await.unwrap(),
        1,
        "起動してから押せば要約される"
    );
    assert_eq!(
        backend.seen.lock().unwrap().len(),
        calls + 1,
        "停止中は 1 回もモデルを呼んでいないこと（呼んだのは起動後の 1 回だけ）"
    );

    shutdown(orchestrator, std::slice::from_ref(&id)).await;
}
