//! オーケストレーターの結合テスト。
//!
//! [`EchoBackend`] を挿すことでネットワークなしに全経路を走らせる。
//! ここで検証したいのは LLM の賢さではなく、**ライフサイクル・配送・打ち切り**の
//! 3 点が仕様どおりに動くこと。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_core::event::CoreEvent;
use agent_core::{ConfigFileKind, RememberTool};
use agent_core::model::{
    AgentId, AgentSpec, AgentStatus, CredentialSource, Endpoint, ModelTemplate, ModelTemplateId,
};
use agent_core::llm::{
    ChatMessage, ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, Role, ToolCall, Usage,
};
use agent_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};
use tokio::sync::broadcast::Receiver;

/// 任意のバックエンドでオーケストレーターを組む。
async fn setup_with(
    dir: &TempDir,
    backend: Arc<dyn LlmBackend>,
    config: OrchestratorConfig,
) -> Orchestrator {
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(backend)),
        Arc::new(InMemorySecretStore::new()),
        config,
    )
    .await
    .expect("bootstrap できること");

    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();

    orchestrator
}

/// テスト用の一時ディレクトリ。終了時に破棄する。
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "concordia-it-{tag}-{}",
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

/// テンプレート 1 件だけ登録済みのオーケストレーターを組む。
async fn setup(dir: &TempDir, config: OrchestratorConfig) -> Orchestrator {
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::echo("[echo]")),
        Arc::new(InMemorySecretStore::new()),
        config,
    )
    .await
    .expect("bootstrap できること");

    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();

    orchestrator
}

/// 一定時間静かになるまでイベントを集める。
///
/// 固定 sleep で待つと、遅いマシンで取りこぼし・速いマシンで無駄待ちになる。
/// 「最後のイベントから `quiet` 経過したら完了」という条件で待つ。
async fn drain_until_quiet(rx: &mut Receiver<CoreEvent>, quiet: Duration) -> Vec<CoreEvent> {
    let mut events = Vec::new();
    while let Ok(Ok(event)) = tokio::time::timeout(quiet, rx.recv()).await {
        events.push(event);
    }
    events
}

/// 発話イベントだけを抜き出す。
fn messages(events: &[CoreEvent]) -> Vec<&agent_core::AgentMessage> {
    events
        .iter()
        .filter_map(|e| match e {
            CoreEvent::MessageSent { message } => Some(message),
            _ => None,
        })
        .collect()
}

/// 提示された `transfer_to_*` ツールを必ず呼ぶバックエンド。
///
/// 「会話が続く」側の経路を再現する。ツールが提示されなければ本文だけを返すので、
/// 接続先を持たないエージェントでは自然に会話が終わる。
struct AlwaysHandoffBackend;

#[async_trait::async_trait]
impl LlmBackend for AlwaysHandoffBackend {
    fn name(&self) -> &str {
        "always-handoff"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let last_user = req
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .map(|m| m.content.as_str())
            .unwrap_or("");
        let text = format!("[handoff] {last_user}");

        let tool_calls = match req.tools.first() {
            Some(tool) => vec![ToolCall {
                id: "call_1".into(),
                name: tool.name.clone(),
                args: serde_json::json!({ "message": text }),
            }],
            None => Vec::new(),
        };

        Ok(ChatResponse {
            text: Some(text),
            tool_calls,
            finish: Finish::Stop,
            usage: Usage {
                prompt: 10,
                completion: 5,
                cache_read: 0,
            },
        })
    }
}

/// 提示されたツールを 1 回だけ呼び、2 回目以降は本文で終えるバックエンド。
///
/// 実行ループ（呼ぶ → 結果を積む → もう一度呼ぶ → 終える）を再現する。
#[derive(Default)]
struct ToolCallingBackend {
    tool: String,
    args: serde_json::Value,
    calls: std::sync::Mutex<usize>,
    /// 最後に受け取ったメッセージ列。結果が積まれたかの確認に使う。
    last: std::sync::Mutex<Vec<ChatMessage>>,
}

#[async_trait::async_trait]
impl LlmBackend for ToolCallingBackend {
    fn name(&self) -> &str {
        "tool-calling"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        *self.last.lock().unwrap() = req.messages.clone();
        let mut calls = self.calls.lock().unwrap();
        let first = *calls == 0;
        *calls += 1;

        let tool_calls = if first {
            vec![ToolCall {
                id: "call_1".into(),
                name: self.tool.clone(),
                args: self.args.clone(),
            }]
        } else {
            Vec::new()
        };

        Ok(ChatResponse {
            text: Some(if first { String::new() } else { "終わりました".into() }),
            tool_calls,
            finish: Finish::Stop,
            usage: Usage {
                prompt: 1,
                completion: 1,
                cache_read: 0,
            },
        })
    }
}

/// 受け取ったリクエストを記録するバックエンド。履歴が積まれるかの検証に使う。
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
        })
    }
}

/// 旧形式（`apiKeyEnv` を持つ設定ファイル）でも開けること。
///
/// 旧フィールドは環境変数名しか持たず、そこから移せる値が無い。開けなくするより、
/// 未知フィールドとして無視して「認証情報が未登録」の状態から始めるほうが良い。
#[tokio::test]
async fn a_legacy_world_file_still_opens() {
    let dir = TempDir::new("legacy");

    std::fs::write(
        dir.0.join("world.json"),
        r#"{
            "agents": [],
            "modelTemplates": [{
                "id": "tpl", "name": "既定",
                "baseUrl": "https://api.anthropic.com/v1",
                "model": "claude-sonnet-5", "contextLength": 128000,
                "temperature": null, "maxOutputTokens": 4096,
                "apiKeyEnv": "ANTHROPIC_API_KEY",
                "provider": "anthropic", "useTools": true, "effort": null,
                "requestTimeoutSecs": 120, "maxRetries": 3
            }]
        }"#,
    )
    .unwrap();

    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::echo("[echo]")),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig::default(),
    )
    .await
    .unwrap();

    let templates = orchestrator.templates().await;
    assert_eq!(templates.len(), 1);
    assert_eq!(templates[0].model, "claude-sonnet-5");
    assert_eq!(templates[0].credential, CredentialSource::Unset);
}

/// 資格情報の登録・削除が、取得元の切り替えと連動すること。
///
/// 秘密だけ入れて `credential` が `None` のままだと、登録したのに使われない。
#[tokio::test]
async fn registering_a_credential_switches_the_template_to_the_keyring() {
    let dir = TempDir::new("credential");
    let orchestrator = setup(&dir, OrchestratorConfig::default()).await;
    let id = ModelTemplateId::from("tpl");

    assert!(!orchestrator.has_credential(&id).unwrap());

    orchestrator.set_credential(&id, "secret-value").await.unwrap();

    assert!(orchestrator.has_credential(&id).unwrap());
    assert_eq!(
        orchestrator.templates().await[0].credential,
        CredentialSource::Keyring
    );
    // 秘密は平文の設定ファイルに現れない。
    let on_disk = std::fs::read_to_string(dir.0.join("world.json")).unwrap();
    assert!(!on_disk.contains("secret-value"));

    orchestrator.clear_credential(&id).await.unwrap();
    assert!(!orchestrator.has_credential(&id).unwrap());
    assert_eq!(
        orchestrator.templates().await[0].credential,
        CredentialSource::Unset
    );
}

/// 存在しないテンプレートに対して秘密を書き込ませない。
#[tokio::test]
async fn a_credential_cannot_be_stored_for_an_unknown_template() {
    let dir = TempDir::new("orphan-credential");
    let orchestrator = setup(&dir, OrchestratorConfig::default()).await;

    let err = orchestrator
        .set_credential(&ModelTemplateId::from("ghost"), "secret-value")
        .await
        .unwrap_err();
    assert_eq!(err.code(), "MODEL_TEMPLATE_NOT_FOUND");
}

/// テンプレートを消したら、資格情報ストアの登録も消えること。
///
/// 設定だけ消して秘密を残すと、画面のどこからも見えない孤児が OS 側に溜まる。
#[tokio::test]
async fn deleting_a_template_also_removes_its_stored_credential() {
    let dir = TempDir::new("credential-cleanup");
    let orchestrator = setup(&dir, OrchestratorConfig::default()).await;
    let id = ModelTemplateId::from("tpl");

    orchestrator.set_credential(&id, "secret-value").await.unwrap();
    orchestrator.remove_template(&id).await.unwrap();

    assert!(!orchestrator.has_credential(&id).unwrap());
}

#[tokio::test]
async fn lifecycle_transitions_are_guarded_in_both_directions() {
    let dir = TempDir::new("lifecycle");
    let orchestrator = setup(&dir, OrchestratorConfig::default()).await;
    let id = AgentId::from("agent_01");

    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "PlannerAgent", "tpl"))
        .await
        .unwrap();

    // 停止中に停止を要求してもエラーになる（黙って成功させない）。
    assert_eq!(
        orchestrator.stop_agent(&id).await.unwrap_err().code(),
        "NOT_RUNNING"
    );

    orchestrator.start_agent(&id).await.unwrap();
    assert_eq!(
        orchestrator.snapshot(&id).await.unwrap().status,
        AgentStatus::Running
    );

    // 二重起動は拒否される。
    assert_eq!(
        orchestrator.start_agent(&id).await.unwrap_err().code(),
        "ALREADY_RUNNING"
    );

    orchestrator.stop_agent(&id).await.unwrap();
    assert_eq!(
        orchestrator.snapshot(&id).await.unwrap().status,
        AgentStatus::Idle
    );
}

#[tokio::test]
async fn message_to_a_leaf_agent_comes_back_to_the_user() {
    let dir = TempDir::new("leaf");
    let orchestrator = setup(&dir, OrchestratorConfig::default()).await;
    let id = AgentId::from("agent_01");

    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "PlannerAgent", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&id, "計画を立てて")
        .await
        .unwrap();

    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    let log = messages(&events);

    assert_eq!(log.len(), 2, "ユーザー発話と応答の 2 件");
    assert_eq!(log[0].from, Endpoint::User);
    assert_eq!(log[1].from, Endpoint::Agent { id: id.clone() });
    // 接続先が無いのでユーザーへ返る。
    assert_eq!(log[1].to, Endpoint::User);
    assert_eq!(log[1].content, "[echo] 計画を立てて");
    assert!(log[1].tokens > 0, "トークンが計上されること");
}

/// **ツールを呼ばなければ会話は終わる。**
///
/// 接続先を持っていても、転送を要求しない応答はそこで完結してユーザーへ返る。
/// 主要フレームワークが共通して採る「ツール呼び出しの無いテキスト出力が最終出力」
/// という規則（failures.md #11）。
#[tokio::test]
async fn an_agent_that_does_not_request_a_handoff_ends_the_conversation() {
    let dir = TempDir::new("finish");
    let orchestrator = setup(&dir, OrchestratorConfig::default()).await;
    let (a, b) = (AgentId::from("agent_01"), AgentId::from("agent_02"));

    orchestrator
        .create_agent(AgentSpec::new(a.clone(), "PlannerAgent", "tpl"))
        .await
        .unwrap();
    orchestrator
        .create_agent(AgentSpec::new(b.clone(), "CriticAgent", "tpl"))
        .await
        .unwrap();
    orchestrator.set_connections(&a, vec![b.clone()]).await.unwrap();

    orchestrator.start_agent(&a).await.unwrap();
    orchestrator.start_agent(&b).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&a, "始めて").await.unwrap();

    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    let log = messages(&events);

    // EchoBackend はツールを呼ばない = 会話終了。b へは渡らない。
    assert_eq!(log.len(), 2, "接続先があっても転送しない: {log:#?}");
    assert_eq!(log[1].to, Endpoint::User);
}

#[tokio::test]
async fn message_is_routed_when_the_agent_requests_a_handoff() {
    let dir = TempDir::new("routing");
    let orchestrator =
        setup_with(&dir, Arc::new(AlwaysHandoffBackend), OrchestratorConfig::default()).await;
    let (a, b) = (AgentId::from("agent_01"), AgentId::from("agent_02"));

    orchestrator
        .create_agent(AgentSpec::new(a.clone(), "PlannerAgent", "tpl"))
        .await
        .unwrap();
    orchestrator
        .create_agent(AgentSpec::new(b.clone(), "CriticAgent", "tpl"))
        .await
        .unwrap();
    orchestrator
        .set_connections(&a, vec![b.clone()])
        .await
        .unwrap();

    orchestrator.start_agent(&a).await.unwrap();
    orchestrator.start_agent(&b).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&a, "始めて").await.unwrap();

    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    let log = messages(&events);

    // user -> a, a -> b, b -> user の 3 件。
    // b は接続先を持たないのでツールが提示されず、そこで終わる。
    assert_eq!(log.len(), 3, "実際: {log:#?}");
    assert_eq!(log[1].from, Endpoint::Agent { id: a.clone() });
    assert_eq!(log[1].to, Endpoint::Agent { id: b.clone() });
    assert_eq!(log[2].from, Endpoint::Agent { id: b.clone() });
    assert_eq!(log[2].to, Endpoint::User);
}

/// 履歴が積まれ、次のターンのプロンプトへ入ること。
///
/// これが無いとエージェントは毎回コールドスタートになり、
/// 同じ入力に同じ出力を返し続けて収束しない（failures.md #12）。
#[tokio::test]
async fn each_turn_sees_the_previous_exchange() {
    let dir = TempDir::new("history");
    let backend = Arc::new(RecordingBackend::default());
    let orchestrator = setup_with(&dir, backend.clone(), OrchestratorConfig::default()).await;
    let id = AgentId::from("agent_01");

    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "PlannerAgent", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "一回目").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(300)).await;
    orchestrator.send_user_message(&id, "二回目").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(300)).await;

    let seen = backend.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);

    // 1 回目は履歴なし。
    let first: Vec<&str> = seen[0].iter().map(|m| m.content.as_str()).collect();
    assert!(!first.iter().any(|c| c.contains("了解")));

    // 2 回目は「一回目」と自分の応答「了解」が入っている。
    let second = &seen[1];
    assert!(
        second
            .iter()
            .any(|m| m.role == Role::User && m.content == "一回目"),
        "前回の受信が履歴に入る: {second:#?}"
    );
    assert!(
        second
            .iter()
            .any(|m| m.role == Role::Assistant && m.content == "了解"),
        "自分の発言が履歴に入る: {second:#?}"
    );
}

/// 履歴は起動のたびにクリアされる。
#[tokio::test]
async fn restarting_an_agent_starts_a_fresh_conversation() {
    let dir = TempDir::new("history-reset");
    let backend = Arc::new(RecordingBackend::default());
    let orchestrator = setup_with(&dir, backend.clone(), OrchestratorConfig::default()).await;
    let id = AgentId::from("agent_01");

    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "PlannerAgent", "tpl"))
        .await
        .unwrap();

    orchestrator.start_agent(&id).await.unwrap();
    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "一回目").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(300)).await;

    orchestrator.stop_agent(&id).await.unwrap();
    orchestrator.start_agent(&id).await.unwrap();
    orchestrator.send_user_message(&id, "二回目").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(300)).await;

    let seen = backend.seen.lock().unwrap();
    let second = seen.last().unwrap();
    assert!(
        !second.iter().any(|m| m.content == "一回目"),
        "再起動で履歴が残らない: {second:#?}"
    );
}

/// ツールを呼んだら実行し、結果を積んでもう一度モデルへ渡すこと。
///
/// OpenAI Agents SDK と同じループ。呼び出しと結果は**対で**履歴に残す必要があり、
/// 結果だけ積むとプロバイダが「対応する呼び出しが無い結果」として拒否する。
#[tokio::test]
async fn a_tool_call_is_executed_and_its_result_is_fed_back() {
    let dir = TempDir::new("tool-loop");
    let backend = Arc::new(ToolCallingBackend {
        tool: "remember".into(),
        args: serde_json::json!({ "note": "相手は簡潔な返答を好む" }),
        ..Default::default()
    });
    let orchestrator = setup_with(&dir, backend.clone(), OrchestratorConfig::default()).await;
    let id = AgentId::from("agent_01");

    orchestrator
        .register_tool(Arc::new(RememberTool::new(ConfigStore::new(&dir.0))))
        .await;
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "PlannerAgent", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "覚えておいて").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    // モデルは 2 回呼ばれる（1 回目でツール、2 回目で最終出力）。
    assert_eq!(*backend.calls.lock().unwrap(), 2);

    // 2 回目のプロンプトに、呼び出しと結果が対で入っている。
    let last = backend.last.lock().unwrap().clone();
    assert!(
        last.iter()
            .any(|m| m.role == Role::Assistant && !m.tool_calls.is_empty()),
        "呼び出しが履歴に残ること: {last:#?}"
    );
    assert!(
        last.iter()
            .any(|m| m.role == Role::Tool && m.tool_call_id.as_deref() == Some("call_1")),
        "結果が対応する ID つきで積まれること: {last:#?}"
    );

    // 実行そのものが通知される（会話ログには現れないため）。
    assert!(
        events.iter().any(|e| matches!(
            e,
            CoreEvent::ToolInvoked { tool, ok: true, .. } if tool == "remember"
        )),
        "ツール実行が通知されること"
    );

    // 副作用が実際に起きている。
    let saved = ConfigStore::new(&dir.0)
        .read_config(&id, ConfigFileKind::Memory)
        .await
        .unwrap();
    assert!(saved.contains("簡潔な返答"), "Memory.md へ書かれること: {saved}");

    // 最終出力がユーザーへ返る。
    let log = messages(&events);
    assert_eq!(log.last().unwrap().content, "終わりました");
}

/// 未知のツール名は会話を止めず、モデルが読める文字列として返ること。
#[tokio::test]
async fn an_unknown_tool_name_does_not_kill_the_turn() {
    let dir = TempDir::new("tool-unknown");
    let backend = Arc::new(ToolCallingBackend {
        tool: "does_not_exist".into(),
        args: serde_json::json!({}),
        ..Default::default()
    });
    let orchestrator = setup_with(&dir, backend.clone(), OrchestratorConfig::default()).await;
    let id = AgentId::from("agent_01");

    // 提示側にも登録しておかないと、そもそも実行対象として拾われない。
    orchestrator
        .register_tool(Arc::new(RememberTool::new(ConfigStore::new(&dir.0))))
        .await;
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "PlannerAgent", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "やって").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    // 未知の名前は実行対象に含まれないので、そのまま最終出力として扱われる。
    assert_eq!(*backend.calls.lock().unwrap(), 1);
    let log = messages(&events);
    assert_eq!(log.len(), 2, "会話は成立して終わる: {log:#?}");
    assert_eq!(
        orchestrator.snapshot(&id).await.unwrap().status,
        AgentStatus::Running,
        "エージェントは落ちない"
    );
}

#[tokio::test]
async fn mutually_connected_agents_stop_at_the_hop_limit() {
    let dir = TempDir::new("hop");
    let config = OrchestratorConfig {
        max_hops: 4,
        ..Default::default()
    };
    let orchestrator = setup_with(&dir, Arc::new(AlwaysHandoffBackend), config).await;
    let (a, b) = (AgentId::from("agent_01"), AgentId::from("agent_02"));

    orchestrator
        .create_agent(AgentSpec::new(a.clone(), "A", "tpl"))
        .await
        .unwrap();
    orchestrator
        .create_agent(AgentSpec::new(b.clone(), "B", "tpl"))
        .await
        .unwrap();
    // 相互接続。トポロジーとしては正当で、止めるのは hop の役目。
    orchestrator.set_connections(&a, vec![b.clone()]).await.unwrap();
    orchestrator.set_connections(&b, vec![a.clone()]).await.unwrap();

    orchestrator.start_agent(&a).await.unwrap();
    orchestrator.start_agent(&b).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&a, "ping").await.unwrap();

    let events = drain_until_quiet(&mut rx, Duration::from_millis(600)).await;
    let log = messages(&events);

    // ユーザー発話 1 + hop 1..=4 の 4 発話 = 5 件で収束する。
    assert_eq!(log.len(), 5, "無限往復せず収束すること: {log:#?}");
    assert_eq!(log.last().unwrap().hop, 4);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, CoreEvent::HopLimitReached { max_hops: 4, .. })),
        "打ち切りが通知されること"
    );
}

#[tokio::test]
async fn sending_to_a_stopped_agent_is_refused() {
    let dir = TempDir::new("stopped");
    let orchestrator = setup(&dir, OrchestratorConfig::default()).await;
    let id = AgentId::from("agent_01");

    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "PlannerAgent", "tpl"))
        .await
        .unwrap();

    let err = orchestrator.send_user_message(&id, "起きてる？").await.unwrap_err();
    assert_eq!(err.code(), "NOT_RUNNING");
    assert!(!err.is_retryable());
}

#[tokio::test]
async fn token_usage_is_aggregated_per_agent() {
    let dir = TempDir::new("usage");
    let orchestrator = setup(&dir, OrchestratorConfig::default()).await;
    let id = AgentId::from("agent_01");

    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "PlannerAgent", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "一回目").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(300)).await;
    orchestrator.send_user_message(&id, "二回目").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(300)).await;

    let usage = orchestrator.token_usage_by_agent().await.unwrap();
    let snapshot = orchestrator.snapshot(&id).await.unwrap();

    assert_eq!(usage.len(), 1, "ユーザー発話は集計対象外");
    assert_eq!(usage[&id], snapshot.total_tokens, "ログ集計と統計が一致すること");
}

#[tokio::test]
async fn state_survives_a_restart_but_agents_do_not_auto_start() {
    let dir = TempDir::new("persist");
    let id = AgentId::from("agent_01");

    {
        let orchestrator = setup(&dir, OrchestratorConfig::default()).await;
        let mut spec = AgentSpec::new(id.clone(), "PlannerAgent", "tpl");
        spec.rag_sources = vec!["wiki_db".into()];
        orchestrator.create_agent(spec).await.unwrap();
        orchestrator.start_agent(&id).await.unwrap();
        orchestrator.shutdown().await;
    }

    let reopened = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::echo("[echo]")),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig::default(),
    )
    .await
    .unwrap();

    let snapshot = reopened.snapshot(&id).await.unwrap();
    assert_eq!(snapshot.name, "PlannerAgent");
    assert_eq!(snapshot.rag_sources, vec!["wiki_db".to_string()]);
    // 再起動で勝手に走り出さない（開いた瞬間に課金が始まらない）。
    assert_eq!(snapshot.status, AgentStatus::Idle);
}
