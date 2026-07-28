//! オーケストレーターの結合テスト。
//!
//! [`EchoBackend`] を挿すことでネットワークなしに全経路を走らせる。
//! ここで検証したいのは LLM の賢さではなく、**ライフサイクル・配送・打ち切り**の
//! 3 点が仕様どおりに動くこと。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_core::event::CoreEvent;
use agent_core::model::{
    AgentId, AgentSpec, AgentStatus, CredentialSource, Endpoint, ModelTemplate, ModelTemplateId,
};
use agent_core::{ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig};
use tokio::sync::broadcast::Receiver;

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
    assert_eq!(templates[0].credential, CredentialSource::None);
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
        CredentialSource::None
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

#[tokio::test]
async fn message_is_routed_along_the_configured_topology() {
    let dir = TempDir::new("routing");
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
    assert_eq!(log.len(), 3, "実際: {log:#?}");
    assert_eq!(log[1].from, Endpoint::Agent { id: a.clone() });
    assert_eq!(log[1].to, Endpoint::Agent { id: b.clone() });
    assert_eq!(log[2].from, Endpoint::Agent { id: b.clone() });
    assert_eq!(log[2].to, Endpoint::User);
    // b は a の出力を受けて二重に echo する。
    assert_eq!(log[2].content, "[echo] [echo] 始めて");
}

#[tokio::test]
async fn mutually_connected_agents_stop_at_the_hop_limit() {
    let dir = TempDir::new("hop");
    let config = OrchestratorConfig {
        max_hops: 4,
        ..Default::default()
    };
    let orchestrator = setup(&dir, config).await;
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
