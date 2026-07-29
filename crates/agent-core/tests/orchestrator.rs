//! オーケストレーターの結合テスト。
//!
//! [`EchoBackend`] を挿すことでネットワークなしに全経路を走らせる。
//! ここで検証したいのは LLM の賢さではなく、**ライフサイクル・配送・打ち切り**の
//! 3 点が仕様どおりに動くこと。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_core::event::CoreEvent;
use agent_core::{
    AgentTool, ConfigFileKind, DiffTool, FdTool, GrepTool, RememberTool, SdTool, ToolContext,
    YqTool,
};
use agent_core::model::{
    AgentId, AgentSpec, AgentStatus, CredentialSource, Endpoint, ModelTemplate, ModelTemplateId,
};
use agent_core::llm::{
    ChatMessage, ChatRequest, ChatResponse, Finish, Grounding, GroundingSource, LlmBackend,
    LlmError, Role, ToolCall, Usage,
};
use agent_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
    SecretStore,
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
                extra: None,
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
            grounding: Default::default(),
        })
    }
}

/// 委譲（`ask_*`）ツールがあり、まだ結果を受け取っていなければ ask する。
/// 受け取っていれば、その内容を引用して会話を終える。
#[derive(Default)]
struct AskingBackend;

#[async_trait::async_trait]
impl LlmBackend for AskingBackend {
    fn name(&self) -> &str {
        "asking"
    }
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let answered = req.messages.iter().find(|m| m.role == Role::Tool);
        let ask_tool = req.tools.iter().find(|t| t.name.starts_with("ask_"));

        if let (None, Some(tool)) = (answered, ask_tool) {
            return Ok(ChatResponse {
                text: Some(String::new()),
                tool_calls: vec![ToolCall {
                    id: "call_1".into(),
                    name: tool.name.clone(),
                    args: serde_json::json!({ "message": "自己紹介をお願いします" }),
                    extra: None,
                }],
                finish: Finish::ToolUse,
                usage: Usage { prompt: 1, completion: 1, cache_read: 0 },
                grounding: Default::default(),
            });
        }

        let text = match answered {
            Some(result) => format!("受け取りました → {}", result.content),
            None => "ブラボーの自己紹介です".to_owned(),
        };
        Ok(ChatResponse {
            text: Some(text),
            tool_calls: Vec::new(),
            finish: Finish::Stop,
            usage: Usage { prompt: 1, completion: 1, cache_read: 0 },
            grounding: Default::default(),
        })
    }
}

/// 提示された転送ツールを**すべて同時に**呼ぶバックエンド。
///
/// Claude / Gemini は 1 応答で複数の tool call を普通に返す（並列ツール呼び出し）。
/// 「みんなに挨拶して」に対してモデルが全接続先へ転送を要求する状況を再現する。
struct FanOutBackend;

#[async_trait::async_trait]
impl LlmBackend for FanOutBackend {
    fn name(&self) -> &str {
        "fan-out"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let text = "みんなへ".to_owned();
        let tool_calls = req
            .tools
            .iter()
            .filter(|tool| tool.name.starts_with("transfer_to_"))
            .enumerate()
            .map(|(index, tool)| ToolCall {
                id: format!("call_{index}"),
                name: tool.name.clone(),
                args: serde_json::json!({ "message": format!("{} への挨拶", tool.name) }),
                extra: None,
            })
            .collect();

        Ok(ChatResponse {
            text: Some(text.clone()),
            tool_calls,
            finish: Finish::Stop,
            usage: Usage {
                prompt: 10,
                completion: 5,
                cache_read: 0,
            },
            grounding: Default::default(),
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
                extra: None,
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
            grounding: Default::default(),
        })
    }
}

/// ツールが提示されている限り**ツール呼び出しだけ**を返すバックエンド。
///
/// ツール実行上限による打ち切りの経路を再現する。実機では「調査系の依頼で
/// モデルがツールを呼び続け、上限に達した周の応答にテキストが無い」形で起きる。
/// ツールが提示されなければテキストを返す — まとめ呼び出し（tools 無し）に
/// 対する実モデルの挙動と同じ。
#[derive(Default)]
struct EndlessToolBackend {
    calls: std::sync::Mutex<usize>,
}

#[async_trait::async_trait]
impl LlmBackend for EndlessToolBackend {
    fn name(&self) -> &str {
        "endless-tool"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let mut calls = self.calls.lock().unwrap();
        *calls += 1;
        let n = *calls;

        if req.tools.is_empty() {
            return Ok(ChatResponse {
                text: Some("ここまでの調査のまとめです。".into()),
                tool_calls: Vec::new(),
                finish: Finish::Stop,
                usage: Usage {
                    prompt: 1,
                    completion: 1,
                    cache_read: 0,
                },
                grounding: Default::default(),
            });
        }

        Ok(ChatResponse {
            text: None,
            tool_calls: vec![ToolCall {
                id: format!("call_{n}"),
                name: "remember".into(),
                // note を毎回変え、remember の重複排除に吸われないようにする。
                args: serde_json::json!({ "note": format!("調査メモ {n}") }),
                extra: None,
            }],
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

/// まとめ要求（tools 無し）にも無言を貫くバックエンド。
///
/// まとめ呼び出しまで失敗した最悪経路で、最終フォールバック文言が出ることを
/// 確かめるために使う。
#[derive(Default)]
struct SilentToolBackend;

#[async_trait::async_trait]
impl LlmBackend for SilentToolBackend {
    fn name(&self) -> &str {
        "silent-tool"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let tool_calls = if req.tools.is_empty() {
            Vec::new()
        } else {
            vec![ToolCall {
                id: "call_s".into(),
                name: "remember".into(),
                args: serde_json::json!({ "note": "沈黙" }),
                extra: None,
            }]
        };
        Ok(ChatResponse {
            text: None,
            tool_calls,
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

/// 提示されたツール名の集合を記録するバックエンド（提示制御の検証用）。
#[derive(Default)]
struct ToolsProbeBackend {
    /// 呼び出しごとの、提示されたツール名（ソート済み）。
    presented: std::sync::Mutex<Vec<Vec<String>>>,
}

#[async_trait::async_trait]
impl LlmBackend for ToolsProbeBackend {
    fn name(&self) -> &str {
        "tools-probe"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let mut names: Vec<String> = req.tools.iter().map(|t| t.name.clone()).collect();
        names.sort();
        self.presented.lock().unwrap().push(names);

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

/// MCP 由来ツールの代役（同梱ではない名前空間つきの名前を持つ）。
struct McpLikeTool;

#[async_trait::async_trait]
impl AgentTool for McpLikeTool {
    fn name(&self) -> &str {
        "memoria__recall"
    }
    fn description(&self) -> String {
        "テスト用の MCP 風ツール".into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    async fn call(&self, _ctx: &ToolContext, _args: &serde_json::Value) -> agent_core::CoreResult<String> {
        Ok("ok".into())
    }
}

/// 同梱 6 本 + MCP 風 1 本を登録する（提示制御テストの土台）。
async fn register_all_tools(orchestrator: &Orchestrator, dir: &TempDir) {
    let store = ConfigStore::new(&dir.0);
    orchestrator.register_tool(Arc::new(RememberTool::new(store))).await;
    orchestrator.register_tool(Arc::new(GrepTool)).await;
    orchestrator.register_tool(Arc::new(FdTool)).await;
    orchestrator.register_tool(Arc::new(DiffTool)).await;
    orchestrator.register_tool(Arc::new(SdTool)).await;
    orchestrator.register_tool(Arc::new(YqTool)).await;
    orchestrator.register_tool(Arc::new(McpLikeTool)).await;
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
            grounding: Default::default(),
        })
    }
}

/// 接地の来歴を載せて返し、受け取ったリクエストも記録するバックエンド。
///
/// Gemini ネイティブ経路（Spec 05）で `groundingMetadata` が付いた応答が
/// 返ってくる状況を、ネットワークなしで再現する。
#[derive(Default)]
struct GroundedBackend {
    seen: std::sync::Mutex<Vec<Vec<ChatMessage>>>,
}

/// テスト内で「これが漏れたら失敗」と判る目印。
const GROUNDED_URI: &str = "https://example.test/nhk-article-42";
const GROUNDED_QUERY: &str = "ザリガニ 生息数 2026";

#[async_trait::async_trait]
impl LlmBackend for GroundedBackend {
    fn name(&self) -> &str {
        "grounded"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.seen.lock().unwrap().push(req.messages.clone());
        Ok(ChatResponse {
            text: Some("調べた結果をお伝えします".into()),
            tool_calls: Vec::new(),
            finish: Finish::Stop,
            usage: Usage {
                prompt: 1,
                completion: 1,
                cache_read: 0,
            },
            grounding: Grounding {
                queries: vec![GROUNDED_QUERY.to_owned()],
                sources: vec![GroundingSource {
                    uri: GROUNDED_URI.to_owned(),
                    title: "ザリガニの生息数について".to_owned(),
                }],
            },
        })
    }
}

/// 接地の来歴が発話に添って表示層まで届くこと（Spec 05 Phase 4）。
///
/// 専用イベントを立てず `MessageSent` に相乗りさせているので、
/// ここが壊れると来歴は UI から丸ごと消える。
#[tokio::test]
async fn grounding_rides_on_the_recorded_message() {
    let dir = TempDir::new("grounding-ride");
    let orchestrator = setup_with(
        &dir,
        Arc::new(GroundedBackend::default()),
        OrchestratorConfig::default(),
    )
    .await;

    let a = AgentId::from("agent_a");
    orchestrator
        .create_agent(AgentSpec::new(a.clone(), "ジェミー", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&a).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&a, "最近のニュースを調べて").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let reply = messages(&events)
        .into_iter()
        .find(|m| m.from == Endpoint::Agent { id: a.clone() })
        .expect("エージェントの発話が記録されること");

    assert_eq!(reply.grounding.queries, vec![GROUNDED_QUERY]);
    assert_eq!(
        reply.grounding.sources.iter().map(|s| s.uri.as_str()).collect::<Vec<_>>(),
        vec![GROUNDED_URI],
        "参照元が発話に添って届くこと: {:#?}",
        reply.grounding,
    );
}

/// 接地の来歴が**次のターンのプロンプトへ戻らない**こと（Spec 05 Notes 9）。
///
/// 接地はそのターンの中で起き、参照元は答えと同時に返る。次ターンの
/// プロンプトへ入れれば、それは前の話題の出典であり、モデルが今引用したい
/// 相手ではない。前ターンの URL を現ターンの根拠として見せるのは新種の
/// 誤帰属で、捏造を別の形へ置き換えるだけになる。
#[tokio::test]
async fn grounding_never_returns_to_the_prompt() {
    let dir = TempDir::new("grounding-no-return");
    let backend = Arc::new(GroundedBackend::default());
    let orchestrator = setup_with(&dir, backend.clone(), OrchestratorConfig::default()).await;

    let a = AgentId::from("agent_a");
    orchestrator
        .create_agent(AgentSpec::new(a.clone(), "ジェミー", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&a).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&a, "最近のニュースを調べて").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    // 2 ターン目。1 ターン目の来歴が履歴・広場ログのどちらかに混ざれば、ここで見える。
    orchestrator.send_user_message(&a, "続けて").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let seen = backend.seen.lock().unwrap();
    assert!(seen.len() >= 2, "2 ターンぶんのリクエストが記録されること: {}", seen.len());
    for (turn, messages) in seen.iter().enumerate() {
        for message in messages {
            assert!(
                !message.content.contains(GROUNDED_URI),
                "参照元 URL がプロンプトへ戻っている（{turn} 周目・{:?}）: {}",
                message.role,
                message.content,
            );
            assert!(
                !message.content.contains(GROUNDED_QUERY),
                "検索語がプロンプトへ戻っている（{turn} 周目・{:?}）: {}",
                message.role,
                message.content,
            );
        }
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

/// 貼り付け由来の前後空白・改行は登録時に落とすこと。
///
/// 混入すると送信時の 401 (Invalid token 等) としてしか表面化せず、
/// 利用者は「正しいキーを貼ったのに拒否される」状態から抜けられない。
#[tokio::test]
async fn credentials_are_trimmed_before_storage() {
    let dir = TempDir::new("credential-trim");
    let secrets = Arc::new(InMemorySecretStore::new());
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::echo("[echo]")),
        Arc::clone(&secrets) as Arc<dyn agent_core::SecretStore>,
        OrchestratorConfig::default(),
    )
    .await
    .unwrap();
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();

    orchestrator
        .set_credential(&"tpl".into(), "  uuid:secret-value\n")
        .await
        .unwrap();

    use agent_core::SecretStore as _;
    assert_eq!(
        secrets.get("tpl").unwrap().as_deref(),
        Some("uuid:secret-value"),
        "前後の空白・改行が落ちて保存されること"
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

/// 古い下書きでテンプレートを保存し直しても、`keyring` が `unset` へ巻き戻らないこと。
///
/// `credential` はコアが所有する派生状態で、正当な遷移経路は
/// `set_credential` / `clear_credential` だけ。UI の下書きは登録前の
/// スナップショットを保持しうるので、upsert がそれを素通しにすると
/// 「キーは資格情報ストアに実在するのに、設定上は未登録」という
/// 実際に起きた不整合が再現する（Gemini テンプレートで表面化）。
#[tokio::test]
async fn saving_a_stale_template_does_not_downgrade_the_keyring_credential() {
    let dir = TempDir::new("stale-upsert");
    let orchestrator = setup(&dir, OrchestratorConfig::default()).await;
    let id = ModelTemplateId::from("tpl");

    orchestrator.set_credential(&id, "secret-value").await.unwrap();
    assert_eq!(
        orchestrator.templates().await[0].credential,
        CredentialSource::Keyring
    );

    // 登録前に開いたダイアログの下書き（credential: unset）で保存し直す。
    let stale = ModelTemplate::new("tpl", "既定", "mock-model");
    assert_eq!(stale.credential, CredentialSource::Unset);
    orchestrator.upsert_template(stale).await.unwrap();

    // 秘密は残っているのだから、取得元も keyring のままであること。
    assert!(orchestrator.has_credential(&id).unwrap());
    assert_eq!(
        orchestrator.templates().await[0].credential,
        CredentialSource::Keyring
    );

    // 巻き戻りがディスクへ固定されないこと（再起動後も接続できること）。
    let on_disk = std::fs::read_to_string(dir.0.join("world.json")).unwrap();
    assert!(on_disk.contains("keyring"));
}

/// `unset` なのに秘密が実在するテンプレートは、起動時に `keyring` へ昇格すること。
///
/// `clear_credential` は秘密の削除と `unset` への遷移を一体で行うので、
/// 「unset かつ秘密あり」は正規の操作では作れない。過去の巻き戻り事故で
/// 固定された状態であり、放置するとユーザーはキーを貼り直すまで接続できない。
#[tokio::test]
async fn bootstrap_promotes_unset_credential_when_the_secret_already_exists() {
    let dir = TempDir::new("heal-credential");
    std::fs::write(
        dir.0.join("world.json"),
        r#"{
            "agents": [],
            "modelTemplates": [{
                "id": "tpl", "name": "gemini",
                "baseUrl": "https://generativelanguage.googleapis.com/v1beta",
                "model": "gemini-3.6-flash", "contextLength": 128000,
                "temperature": null, "maxOutputTokens": 4096,
                "credential": "unset",
                "provider": null, "useTools": true, "effort": null,
                "requestTimeoutSecs": 120, "maxRetries": 3
            }]
        }"#,
    )
    .unwrap();

    let secrets = Arc::new(InMemorySecretStore::new());
    secrets.set("tpl", "secret-value").unwrap();

    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::echo("[echo]")),
        secrets,
        OrchestratorConfig::default(),
    )
    .await
    .unwrap();

    assert_eq!(
        orchestrator.templates().await[0].credential,
        CredentialSource::Keyring
    );
    // 昇格は起動時にディスクへも書き戻される。
    let on_disk = std::fs::read_to_string(dir.0.join("world.json")).unwrap();
    assert!(on_disk.contains("keyring"));
}

/// 秘密が無いのに `keyring` を主張する下書きは `unset` へ正規化されること。
///
/// これを素通しにすると、送信時に「資格情報ストアに見つかりません」という
/// 一段遠いエラーへずれ込む。設定不備は保存の時点で `unset`（= 未登録の警告表示）に
/// 引き戻しておく。
#[tokio::test]
async fn an_unverified_keyring_claim_is_normalized_to_unset_on_upsert() {
    let dir = TempDir::new("keyring-claim");
    let orchestrator = setup(&dir, OrchestratorConfig::default()).await;

    let mut claimed = ModelTemplate::new("tpl2", "無根拠", "mock-model");
    claimed.credential = CredentialSource::Keyring;
    orchestrator.upsert_template(claimed).await.unwrap();

    let templates = orchestrator.templates().await;
    let saved = templates.iter().find(|t| t.id.as_str() == "tpl2").unwrap();
    assert_eq!(saved.credential, CredentialSource::Unset);
}

/// 「認証不要」の明示は upsert 経由の正当な遷移として通ること。
///
/// ローカル推論サーバ向けのチェックボックスはこの経路しか持たない。
/// 巻き戻り防止の対象は keyring だけで、unset ⇄ not_required を塞いではいけない。
#[tokio::test]
async fn not_required_transitions_still_flow_through_upsert() {
    let dir = TempDir::new("not-required");
    let orchestrator = setup(&dir, OrchestratorConfig::default()).await;

    let mut template = ModelTemplate::new("tpl", "既定", "mock-model");
    template.credential = CredentialSource::NotRequired;
    orchestrator.upsert_template(template.clone()).await.unwrap();
    assert_eq!(
        orchestrator.templates().await[0].credential,
        CredentialSource::NotRequired
    );

    template.credential = CredentialSource::Unset;
    orchestrator.upsert_template(template).await.unwrap();
    assert_eq!(
        orchestrator.templates().await[0].credential,
        CredentialSource::Unset
    );
}

/// 同報の注記が**受信者にだけ**入り、宛先外には発話の存在ごと見えないこと。
///
/// ユーザーが「みんなこんにちは」を同報すると、各受信者は自分しか受け取って
/// いないように見えるため、律儀に接続先へ転送して反響が起きる（実機で観測）。
/// 転送を禁止するのではなく、「全員が既に受け取っている」という事実を
/// 封筒に書くことで、転送する理由そのものを消す。
#[tokio::test]
async fn broadcast_note_names_the_recipients_and_stays_invisible_to_others() {
    let backend = Arc::new(RecordingBackend::default());
    let dir = TempDir::new("broadcast-note");
    let orchestrator = setup_with(
        &dir,
        Arc::clone(&backend) as Arc<dyn LlmBackend>,
        OrchestratorConfig::default(),
    )
    .await;

    let (a, b, c) = (
        AgentId::from("agent_a"),
        AgentId::from("agent_b"),
        AgentId::from("agent_c"),
    );
    for (id, name) in [(&a, "アルファ"), (&b, "ブラボー"), (&c, "チャーリー")] {
        orchestrator
            .create_agent(AgentSpec::new(id.clone(), name, "tpl"))
            .await
            .unwrap();
        orchestrator.start_agent(id).await.unwrap();
    }

    let mut rx = orchestrator.subscribe();

    // UI の同報と同じ形: 宛先 a と b へ 1 通ずつ、同報の全宛先を添えて投入する。
    // c は宛先に含まれない。
    for target in [&a, &b] {
        orchestrator
            .send_user_message_broadcast(target, "みんなこんにちは", &[a.clone(), b.clone()])
            .await
            .unwrap();
    }
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    // 受信者のプロンプトに同報の注記が入り、宛先の名前が列挙されること。
    let requests = backend.seen.lock().unwrap().clone();
    assert_eq!(requests.len(), 2, "処理されるのは宛先 2 体ぶんだけ");
    for messages in &requests {
        let note = messages
            .iter()
            .find(|m| m.role == Role::System && m.content.contains("同報"))
            .expect("同報の注記が入ること");
        assert!(note.content.contains("アルファ"), "実際: {}", note.content);
        assert!(note.content.contains("ブラボー"), "実際: {}", note.content);
        assert!(
            !note.content.contains("チャーリー"),
            "宛先外の名前を列挙しない: {}",
            note.content
        );
        assert!(
            note.content.contains("転送する必要はありません"),
            "転送不要の根拠を伝える: {}",
            note.content
        );
        // 転送だけを禁じても、「代わりに促す」経路が残る。実機では
        // 「ユーザーから依頼です、自己紹介お願いします」という**新しい発話**を
        // 他の参加者へ配って回り、同じ混乱が起きた。
        assert!(
            note.content.contains("促す必要もありません"),
            "発言を促す必要も無いことを伝える: {}",
            note.content
        );
    }

    // 宛先外の c には配送されず、ログにも c 宛の発話が存在しない。
    let log = orchestrator.message_log(None).await;
    assert!(
        log.iter().all(|m| m.to != Endpoint::Agent { id: c.clone() }),
        "宛先外のエージェントは発話の存在を知らない"
    );
}

/// 委譲（ask）— 頼んだ答えが**依頼主に戻る**こと。
///
/// 転送（handoff）は制御ごと相手へ渡す機構で、相手の答えはユーザーへ返る。
/// だが「ロボットくん1号、自己紹介をお願いします」のように**答えを受け取って
/// 自分の話を続けたい**場面では、転送では依頼主が結果を知れない。
/// OpenAI Agents SDK の agent-as-tool と同じ、委譲の経路を用意する。
#[tokio::test]
async fn asking_another_agent_returns_the_answer_to_the_caller() {
    let dir = TempDir::new("ask");
    let orchestrator =
        setup_with(&dir, Arc::new(AskingBackend), OrchestratorConfig::default()).await;

    let (a, b) = (AgentId::from("agent_a"), AgentId::from("agent_b"));
    orchestrator
        .create_agent(AgentSpec::new(a.clone(), "アルファ", "tpl"))
        .await
        .unwrap();
    orchestrator
        .create_agent(AgentSpec::new(b.clone(), "ブラボー", "tpl"))
        .await
        .unwrap();
    orchestrator.set_connections(&a, vec![b.clone()]).await.unwrap();
    orchestrator.start_agent(&a).await.unwrap();
    orchestrator.start_agent(&b).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&a, "ブラボーに聞いて").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(600)).await;
    let log = messages(&events);

    // アルファがユーザーへ返した最終出力に、ブラボーの答えが載っていること。
    let final_reply = log
        .iter()
        .find(|m| m.from == Endpoint::Agent { id: a.clone() } && m.to == Endpoint::User)
        .expect("アルファがユーザーへ返すこと");
    assert!(
        final_reply.content.contains("ブラボーの自己紹介です"),
        "頼んだ答えが依頼主へ戻ること。実際: {}",
        final_reply.content
    );

    // ブラボーの答えは「ユーザーへの最終出力」ではなく、アルファ宛として記録される。
    let brabo_reply = log
        .iter()
        .find(|m| m.from == Endpoint::Agent { id: b.clone() })
        .expect("ブラボーの発話が記録されること");
    assert_eq!(
        brabo_reply.to,
        Endpoint::Agent { id: a.clone() },
        "依頼主へ返した発話として記録されること: {brabo_reply:#?}"
    );
}

/// 進行役 1 体へ頼む形（orchestrator-workers）が収束すること。
///
/// 同報で「みんな自己紹介して」と投げると、全員のターンが**並列に走る**。
/// 進行役が促そうとした時点で他の答えはまだ存在せず、結果として同じ相手が
/// 二度答える。一方、**進行役 1 体だけに頼む**と、その 1 体が順に委譲し、
/// 答えを受け取ってからまとめる — 各エージェントはちょうど 1 回ずつ話し、
/// 重複が構造的に起こらない。
/// Anthropic が orchestrator-workers と呼ぶ形と同じ。
#[tokio::test]
async fn asking_one_facilitator_converges_without_duplicates() {
    /// 委譲ツールが提示されていれば**全員へ**委譲し、答えが揃ったらまとめる。
    #[derive(Default)]
    struct FacilitatorBackend;

    #[async_trait::async_trait]
    impl LlmBackend for FacilitatorBackend {
        fn name(&self) -> &str {
            "facilitator"
        }
        async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
            let answered = req.messages.iter().any(|m| m.role == Role::Tool);
            let asks: Vec<_> = req
                .tools
                .iter()
                .filter(|t| t.name.starts_with("ask_"))
                .collect();

            if !answered && !asks.is_empty() {
                return Ok(ChatResponse {
                    text: Some(String::new()),
                    tool_calls: asks
                        .iter()
                        .enumerate()
                        .map(|(index, tool)| ToolCall {
                            id: format!("call_{index}"),
                            name: tool.name.clone(),
                            args: serde_json::json!({ "message": "自己紹介をお願いします" }),
                            extra: None,
                        })
                        .collect(),
                    finish: Finish::ToolUse,
                    usage: Usage { prompt: 1, completion: 1, cache_read: 0 },
                    grounding: Default::default(),
                });
            }

            let text = if answered {
                let collected: Vec<&str> = req
                    .messages
                    .iter()
                    .filter(|m| m.role == Role::Tool)
                    .map(|m| m.content.as_str())
                    .collect();
                format!("みんなの自己紹介です: {}", collected.join(" / "))
            } else {
                "わたしの自己紹介です".to_owned()
            };
            Ok(ChatResponse {
                text: Some(text),
                tool_calls: Vec::new(),
                finish: Finish::Stop,
                usage: Usage { prompt: 1, completion: 1, cache_read: 0 },
                grounding: Default::default(),
            })
        }
    }

    let dir = TempDir::new("facilitator");
    let orchestrator =
        setup_with(&dir, Arc::new(FacilitatorBackend), OrchestratorConfig::default()).await;

    let (host, b, c) = (
        AgentId::from("agent_host"),
        AgentId::from("agent_b"),
        AgentId::from("agent_c"),
    );
    for (id, name) in [(&host, "ザリ"), (&b, "ロボ"), (&c, "ジェミー")] {
        orchestrator
            .create_agent(AgentSpec::new(id.clone(), name, "tpl"))
            .await
            .unwrap();
        orchestrator.start_agent(id).await.unwrap();
    }
    orchestrator
        .set_connections(&host, vec![b.clone(), c.clone()])
        .await
        .unwrap();

    let mut rx = orchestrator.subscribe();
    // **進行役 1 体だけ**へ頼む（同報しない）。
    orchestrator
        .send_user_message(&host, "みんなに自己紹介するように言って")
        .await
        .unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(800)).await;
    let log = messages(&events);

    // 各ワーカーはちょうど 1 回ずつ答える（重複が起きない）。
    for worker in [&b, &c] {
        let spoken: Vec<_> = log
            .iter()
            .filter(|m| m.from == Endpoint::Agent { id: worker.clone() })
            .collect();
        assert_eq!(spoken.len(), 1, "{worker} の発話が 1 回であること: {spoken:#?}");
        assert_eq!(
            spoken[0].to,
            Endpoint::Agent { id: host.clone() },
            "答えは進行役へ戻ること"
        );
    }

    // 進行役はユーザーへ 1 本にまとめて返す。
    let final_replies: Vec<_> = log
        .iter()
        .filter(|m| m.from == Endpoint::Agent { id: host.clone() } && m.to == Endpoint::User)
        .collect();
    assert_eq!(final_replies.len(), 1, "実際: {final_replies:#?}");
    assert!(
        final_replies[0].content.contains("みんなの自己紹介です"),
        "受け取った答えを踏まえてまとめること: {}",
        final_replies[0].content
    );
}

/// 応答しない相手への委譲が、会話を永久に止めないこと。
///
/// 委譲は相手の応答を待って**ブロックする**。相手が停止中なら即座に失敗するが、
/// 相互に ask し合う配置では待ち合わせが起きうる。上限を持たせて必ず戻す。
#[tokio::test]
async fn asking_a_stopped_agent_fails_without_hanging() {
    let dir = TempDir::new("ask-stopped");
    let orchestrator =
        setup_with(&dir, Arc::new(AskingBackend), OrchestratorConfig::default()).await;

    let (a, b) = (AgentId::from("agent_a"), AgentId::from("agent_b"));
    orchestrator
        .create_agent(AgentSpec::new(a.clone(), "アルファ", "tpl"))
        .await
        .unwrap();
    orchestrator
        .create_agent(AgentSpec::new(b.clone(), "ブラボー", "tpl"))
        .await
        .unwrap();
    orchestrator.set_connections(&a, vec![b.clone()]).await.unwrap();
    // b は起動しない。
    orchestrator.start_agent(&a).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&a, "止まっている相手に頼む").await.unwrap();

    // 会話が返ってくること自体が検証内容（無限に待たない）。
    let events = drain_until_quiet(&mut rx, Duration::from_millis(600)).await;
    let log = messages(&events);
    assert!(
        log.iter().any(|m| m.to == Endpoint::User),
        "ユーザーへ何かが返ること: {log:#?}"
    );
}

/// 居合わせた会話（広場ログ）が見えること。
///
/// 各エージェントの履歴は私的で、他人の発言は一切見えなかった。
/// 「みんなに自己紹介して」と頼んでも、互いの自己紹介が届かない。
/// 村の広場では、話は宛先でなくても聞こえる — ただし**返事をするのは
/// 呼ばれた人だけ**（聞こえることと反応することは別の軸）。
#[tokio::test]
async fn agents_overhear_what_others_said_in_the_room() {
    /// 転送ツールがあれば渡し、無ければ本文で終える。全リクエストを記録する。
    #[derive(Default)]
    struct RoomBackend {
        seen: std::sync::Mutex<Vec<Vec<ChatMessage>>>,
    }

    #[async_trait::async_trait]
    impl LlmBackend for RoomBackend {
        fn name(&self) -> &str {
            "room"
        }
        async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
            self.seen.lock().unwrap().push(req.messages.clone());
            let tool_calls = match req.tools.iter().find(|t| t.name.starts_with("transfer_to_")) {
                Some(tool) => vec![ToolCall {
                    id: "call_1".into(),
                    name: tool.name.clone(),
                    args: serde_json::json!({ "message": "秘密の合言葉です" }),
                    extra: None,
                }],
                None => Vec::new(),
            };
            Ok(ChatResponse {
                text: Some("了解".into()),
                tool_calls,
                finish: Finish::Stop,
                usage: Usage { prompt: 1, completion: 1, cache_read: 0 },
                grounding: Default::default(),
            })
        }
    }

    let backend = Arc::new(RoomBackend::default());
    let dir = TempDir::new("room-log");
    let orchestrator = setup_with(
        &dir,
        Arc::clone(&backend) as Arc<dyn LlmBackend>,
        OrchestratorConfig::default(),
    )
    .await;

    let (a, b, c) = (
        AgentId::from("agent_a"),
        AgentId::from("agent_b"),
        AgentId::from("agent_c"),
    );
    for (id, name) in [(&a, "アルファ"), (&b, "ブラボー"), (&c, "チャーリー")] {
        orchestrator
            .create_agent(AgentSpec::new(id.clone(), name, "tpl"))
            .await
            .unwrap();
        orchestrator.start_agent(id).await.unwrap();
    }
    // a → b だけを繋ぐ。c は誰とも繋がっていない「居合わせただけ」の第三者。
    orchestrator.set_connections(&a, vec![b.clone()]).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&a, "ブラボーへ伝えて").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    // ここまでで a → b の発話がログに残っている。次に c へ話しかける。
    orchestrator.send_user_message(&c, "何か聞こえた？").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let requests = backend.seen.lock().unwrap().clone();
    let last = requests.last().expect("c のリクエスト");
    let joined = last
        .iter()
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        joined.contains("秘密の合言葉"),
        "居合わせた会話が見えること。実際:\n{joined}"
    );
    assert!(
        joined.contains("アルファ"),
        "誰の発言かが分かること。実際:\n{joined}"
    );
}

/// 広場ログは受信側でオプトアウトできること（Spec 03）。
///
/// false でも自分の発話は他者の広場ログに載る（受信側だけの設定）ことは
/// 逆向きの検証（b には c の存在が見える必要は無いのでここでは a の発話で確認）。
#[tokio::test]
async fn an_agent_can_opt_out_of_the_room_log() {
    let backend = Arc::new(RecordingBackend::default());
    let dir = TempDir::new("room-optout");
    let orchestrator = setup_with(
        &dir,
        Arc::clone(&backend) as Arc<dyn LlmBackend>,
        OrchestratorConfig::default(),
    )
    .await;

    let (a, b, c) = (
        AgentId::from("agent_a"),
        AgentId::from("agent_b"),
        AgentId::from("agent_c"),
    );
    orchestrator
        .create_agent(AgentSpec::new(a.clone(), "アルファ", "tpl"))
        .await
        .unwrap();
    orchestrator
        .create_agent(AgentSpec::new(b.clone(), "ブラボー", "tpl"))
        .await
        .unwrap();
    let mut spec_c = AgentSpec::new(c.clone(), "チャーリー", "tpl");
    spec_c.hears_room_log = false;
    orchestrator.create_agent(spec_c).await.unwrap();
    for id in [&a, &b, &c] {
        orchestrator.start_agent(id).await.unwrap();
    }

    let mut rx = orchestrator.subscribe();
    // a の応答（エージェント発の発話）を広場ログの原料として作る。
    orchestrator.send_user_message(&a, "挨拶して").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    orchestrator.send_user_message(&b, "どう？").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    orchestrator.send_user_message(&c, "どう？").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let requests = backend.seen.lock().unwrap().clone();
    let join = |index: usize| -> String {
        requests[index]
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    };

    assert!(
        join(1).contains("この場で交わされていた会話"),
        "既定（true）の b には広場ログが入ること。実際:\n{}",
        join(1)
    );
    assert!(
        !join(2).contains("この場で交わされていた会話"),
        "オプトアウトした c には広場ログが入らないこと。実際:\n{}",
        join(2)
    );
}

/// 新規チャットは会話だけを消し、エージェントは消さないこと（Spec 03）。
#[tokio::test]
async fn a_new_chat_resets_the_conversation_but_not_the_agent() {
    let backend = Arc::new(RecordingBackend::default());
    let dir = TempDir::new("new-chat");
    let orchestrator = setup_with(
        &dir,
        Arc::clone(&backend) as Arc<dyn LlmBackend>,
        OrchestratorConfig::default(),
    )
    .await;
    let id = AgentId::from("agent_01");

    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "PlannerAgent", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "一回目").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let tokens_before = orchestrator.snapshot(&id).await.unwrap().total_tokens;
    assert!(tokens_before > 0, "リセット前にトークンが積まれていること");
    assert!(!orchestrator.message_log(None).await.is_empty());

    orchestrator.reset_conversation().await;
    let events = drain_until_quiet(&mut rx, Duration::from_millis(200)).await;

    assert!(
        events
            .iter()
            .any(|e| matches!(e, CoreEvent::ConversationCleared)),
        "リセットが通知されること"
    );
    assert!(orchestrator.message_log(None).await.is_empty(), "会話ログが消えること");
    let snapshot = orchestrator.snapshot(&id).await.unwrap();
    assert_eq!(snapshot.status, AgentStatus::Running, "稼働状態は維持");
    assert_eq!(snapshot.total_tokens, tokens_before, "累積統計は維持");

    // 次のターンはコールドスタート: 旧履歴がプロンプトに入らない。
    orchestrator.send_user_message(&id, "二回目").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let requests = backend.seen.lock().unwrap().clone();
    let last = requests.last().unwrap();
    assert!(
        !last.iter().any(|m| m.content.contains("一回目")),
        "旧履歴が消えていること: {last:#?}"
    );
}

/// リセット中に飛行していたターンの完了書き込みは許容されること（案 A）。
///
/// 発話は起きた事実であり、ログに残す（hop 打ち切りの「記録してから
/// 打ち切る」と同じ規律）。世代管理による破棄は採らない。
#[tokio::test]
async fn an_in_flight_turn_may_land_after_a_reset() {
    /// 応答に時間がかかるバックエンド（飛行中状態を作る）。
    struct SlowEchoBackend;

    #[async_trait::async_trait]
    impl LlmBackend for SlowEchoBackend {
        fn name(&self) -> &str {
            "slow-echo"
        }
        async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, LlmError> {
            tokio::time::sleep(Duration::from_millis(200)).await;
            Ok(ChatResponse {
                text: Some("遅い応答".into()),
                tool_calls: Vec::new(),
                finish: Finish::Stop,
                usage: Usage { prompt: 1, completion: 1, cache_read: 0 },
                grounding: Default::default(),
            })
        }
    }

    let dir = TempDir::new("reset-inflight");
    let orchestrator = setup_with(
        &dir,
        Arc::new(SlowEchoBackend),
        OrchestratorConfig::default(),
    )
    .await;
    let id = AgentId::from("agent_01");

    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "PlannerAgent", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "考えて").await.unwrap();
    // 飛行中（LLM 応答待ち）にリセットする。
    tokio::time::sleep(Duration::from_millis(50)).await;
    orchestrator.reset_conversation().await;

    drain_until_quiet(&mut rx, Duration::from_millis(500)).await;

    let log = orchestrator.message_log(None).await;
    assert_eq!(log.len(), 1, "飛行中だった発話 1 件だけが載ること: {log:#?}");
    assert!(log[0].content.contains("遅い応答"), "{log:#?}");
}

/// ユーザーが宛先を選んだ発話は、宛先外のエージェントには広場ログにも出ないこと。
///
/// 「その人が通知に入っていないときは、そのエージェントはメッセージがあったこと
/// すら知らないべき」（ユーザー指示）。広場ログは**エージェント同士の発話**を
/// 共有する機構で、ユーザーが選んだ聴衆を迂回する裏口にしてはいけない。
#[tokio::test]
async fn a_private_user_message_never_leaks_into_the_room_log() {
    let backend = Arc::new(RecordingBackend::default());
    let dir = TempDir::new("room-privacy");
    let orchestrator = setup_with(
        &dir,
        Arc::clone(&backend) as Arc<dyn LlmBackend>,
        OrchestratorConfig::default(),
    )
    .await;

    let (a, b) = (AgentId::from("agent_a"), AgentId::from("agent_b"));
    for (id, name) in [(&a, "アルファ"), (&b, "ブラボー")] {
        orchestrator
            .create_agent(AgentSpec::new(id.clone(), name, "tpl"))
            .await
            .unwrap();
        orchestrator.start_agent(id).await.unwrap();
    }

    let mut rx = orchestrator.subscribe();
    // a にだけ内緒話をする。
    orchestrator.send_user_message(&a, "これはアルファだけに言う内緒話").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    // 次に b へ話しかける。
    orchestrator.send_user_message(&b, "何か聞いた？").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let requests = backend.seen.lock().unwrap().clone();
    let last = requests.last().expect("b のリクエスト");
    let joined = last
        .iter()
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        !joined.contains("内緒話"),
        "ユーザーが選んだ聴衆を広場ログが迂回してはいけない。実際:\n{joined}"
    );
}

/// 転送ツールが**表示名**で相手を指すこと。
///
/// 会話は表示名（「ザリ・ロブステル」）で流れるのに、ツールは内部 ID
/// （`agent_2`）でしか相手を示していなかった。名前と ID を結ぶ情報が
/// プロンプトのどこにも無く、モデルは「誰に渡せばよいか」を推測するしかない。
/// 実機では、宛先を取り違える・自分で全員のセリフを書く、として現れた。
#[tokio::test]
async fn handoff_tools_identify_targets_by_display_name() {
    /// リクエストのツール定義を記録するバックエンド。
    #[derive(Default)]
    struct ToolSpyBackend {
        seen: std::sync::Mutex<Vec<agent_core::llm::ToolSpec>>,
    }

    #[async_trait::async_trait]
    impl LlmBackend for ToolSpyBackend {
        fn name(&self) -> &str {
            "tool-spy"
        }
        async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
            *self.seen.lock().unwrap() = req.tools.clone();
            Ok(ChatResponse {
                text: Some("了解".into()),
                tool_calls: Vec::new(),
                finish: Finish::Stop,
                usage: Usage { prompt: 1, completion: 1, cache_read: 0 },
                grounding: Default::default(),
            })
        }
    }

    let backend = Arc::new(ToolSpyBackend::default());
    let dir = TempDir::new("handoff-names");
    let orchestrator = setup_with(
        &dir,
        Arc::clone(&backend) as Arc<dyn LlmBackend>,
        OrchestratorConfig::default(),
    )
    .await;

    let (a, b) = (AgentId::from("agent_1"), AgentId::from("agent_2"));
    orchestrator
        .create_agent(AgentSpec::new(a.clone(), "ジェミー", "tpl"))
        .await
        .unwrap();
    orchestrator
        .create_agent(AgentSpec::new(b.clone(), "ザリ・ロブステル", "tpl"))
        .await
        .unwrap();
    orchestrator.set_connections(&a, vec![b.clone()]).await.unwrap();
    orchestrator.start_agent(&a).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&a, "こんにちは").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let tools = backend.seen.lock().unwrap().clone();
    let handoff = tools
        .iter()
        .find(|t| t.name.starts_with("transfer_to_"))
        .expect("転送ツールが提示されること");

    assert!(
        handoff.description.contains("ザリ・ロブステル"),
        "説明が表示名で相手を示すこと。実際: {}",
        handoff.description
    );
}

/// 村の条例（ワークスペース全体の規則）が全エージェントのプロンプト最上段に入ること。
///
/// 規則の序列は「ベンダーの憲法 > 村の条例 > 各エージェントの個別設定」。
/// 条例はモデル間の憲法差（振る舞いの既定値の違い）を吸収する正規化層でもあり、
/// どのモデルのエージェントも同じ場の規則を同じ位置で受け取る。
#[tokio::test]
async fn the_ordinance_prefixes_every_agents_system_prompt() {
    let backend = Arc::new(RecordingBackend::default());
    let dir = TempDir::new("ordinance");
    let orchestrator = setup_with(
        &dir,
        Arc::clone(&backend) as Arc<dyn LlmBackend>,
        OrchestratorConfig::default(),
    )
    .await;

    let id = AgentId::from("agent_a");
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "アルファ", "tpl"))
        .await
        .unwrap();
    orchestrator
        .write_config(&id, ConfigFileKind::Construct, "私は挨拶担当です。")
        .await
        .unwrap();
    orchestrator
        .write_ordinance("ここは Outcasts 村です。雰囲気より検証できる話を大事にします。")
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "こんにちは").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let requests = backend.seen.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    let system = requests[0]
        .iter()
        .find(|m| m.role == Role::System)
        .expect("システムプロンプトがあること");

    let ordinance_at = system.content.find("Outcasts 村です").expect("条例が入ること");
    let construct_at = system.content.find("挨拶担当です").expect("個別設定が入ること");
    assert!(
        ordinance_at < construct_at,
        "条例は個別設定より上に置く（序列がプロンプトの物理順になる）: {}",
        system.content
    );

    // 読み戻しの往復。
    assert_eq!(
        orchestrator.read_ordinance().await.unwrap(),
        "ここは Outcasts 村です。雰囲気より検証できる話を大事にします。"
    );
}

/// 条例が空なら、プロンプトへ空のセクションを差し込まないこと。
#[tokio::test]
async fn an_empty_ordinance_leaves_the_prompt_untouched() {
    let backend = Arc::new(RecordingBackend::default());
    let dir = TempDir::new("no-ordinance");
    let orchestrator = setup_with(
        &dir,
        Arc::clone(&backend) as Arc<dyn LlmBackend>,
        OrchestratorConfig::default(),
    )
    .await;

    let id = AgentId::from("agent_a");
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "アルファ", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "こんにちは").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let requests = backend.seen.lock().unwrap().clone();
    assert!(
        requests[0]
            .iter()
            .all(|m| !m.content.contains("村の条例")),
        "未設定の条例は痕跡を残さない"
    );
}

/// 受信した発話に**送り手の名前**が封筒として付くこと。
///
/// ユーザーの言葉もエージェントからの転送も、同じ user ロールで届く。
/// 送り手を書かないと受信側は区別できず、実際にユーザーの発話を
/// 「他のエージェントが話した言葉」と取り違えた。
#[tokio::test]
async fn incoming_messages_carry_the_sender_name() {
    /// 記録しつつ、最初の 1 回だけ転送するバックエンド。
    /// user → a （ユーザー発話の封筒）と a → b （エージェント発話の封筒）の
    /// 両方を 1 本のシナリオで観測する。
    #[derive(Default)]
    struct RecordingHandoffBackend {
        seen: std::sync::Mutex<Vec<Vec<ChatMessage>>>,
        calls: std::sync::Mutex<usize>,
    }

    #[async_trait::async_trait]
    impl LlmBackend for RecordingHandoffBackend {
        fn name(&self) -> &str {
            "recording-handoff"
        }

        async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
            self.seen.lock().unwrap().push(req.messages.clone());
            let mut calls = self.calls.lock().unwrap();
            let first = *calls == 0;
            *calls += 1;

            let tool_calls = if first {
                match req.tools.iter().find(|t| t.name.starts_with("transfer_to_")) {
                    Some(tool) => vec![ToolCall {
                        id: "call_1".into(),
                        name: tool.name.clone(),
                        args: serde_json::json!({ "message": "アルファからの相談です" }),
                        extra: None,
                    }],
                    None => Vec::new(),
                }
            } else {
                Vec::new()
            };

            Ok(ChatResponse {
                text: Some("了解".into()),
                tool_calls,
                finish: Finish::Stop,
                usage: Usage { prompt: 1, completion: 1, cache_read: 0 },
                grounding: Default::default(),
            })
        }
    }

    let backend = Arc::new(RecordingHandoffBackend::default());
    let dir = TempDir::new("sender-envelope");
    let orchestrator = setup_with(
        &dir,
        Arc::clone(&backend) as Arc<dyn LlmBackend>,
        OrchestratorConfig::default(),
    )
    .await;

    let (a, b) = (AgentId::from("agent_a"), AgentId::from("agent_b"));
    orchestrator.create_agent(AgentSpec::new(a.clone(), "アルファ", "tpl")).await.unwrap();
    orchestrator.create_agent(AgentSpec::new(b.clone(), "ブラボー", "tpl")).await.unwrap();
    orchestrator.set_connections(&a, vec![b.clone()]).await.unwrap();
    orchestrator.start_agent(&a).await.unwrap();
    orchestrator.start_agent(&b).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&a, "こんにちは").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let requests = backend.seen.lock().unwrap().clone();
    assert_eq!(requests.len(), 2, "a と b の 2 回処理される");

    // a が受けたのはユーザーの言葉。封筒にそう書いてあること。
    let a_incoming = requests[0].iter().rev().find(|m| m.role == Role::User).unwrap();
    assert!(
        a_incoming.content.contains("送り手: ユーザー"),
        "実際: {}",
        a_incoming.content
    );

    // b が受けたのはアルファの言葉。ユーザーの言葉と取り違えないこと。
    let b_incoming = requests[1].iter().rev().find(|m| m.role == Role::User).unwrap();
    assert!(
        b_incoming.content.contains("送り手: アルファ"),
        "実際: {}",
        b_incoming.content
    );
    assert!(
        !b_incoming.content.contains("送り手: ユーザー"),
        "エージェントの転送をユーザーの言葉として偽装しない: {}",
        b_incoming.content
    );
}

/// 単独宛の送信には同報の注記が入らないこと。
///
/// 1 対 1 の会話に「同報です」と書くのは嘘であり、モデルの判断を歪める。
#[tokio::test]
async fn a_plain_send_carries_no_broadcast_note() {
    let backend = Arc::new(RecordingBackend::default());
    let dir = TempDir::new("no-broadcast-note");
    let orchestrator = setup_with(
        &dir,
        Arc::clone(&backend) as Arc<dyn LlmBackend>,
        OrchestratorConfig::default(),
    )
    .await;

    let id = AgentId::from("agent_a");
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "アルファ", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "こんにちは").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let requests = backend.seen.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].iter().all(|m| !m.content.contains("同報")),
        "単独宛に同報の注記を入れない"
    );
}

/// 最小の正当な WebP コンテナ（RIFF ヘッダ + "WEBP"）を作る。
fn webp_bytes(payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(4u32 + payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"WEBP");
    bytes.extend_from_slice(payload);
    bytes
}

/// アイコンの保存・取得・削除の往復。
///
/// 中身は WebP に固定する契約（変換は UI 層の責務）。
/// コアはマジック番号とサイズ上限で入口を絞り、任意バイト列の書き込み経路を塞ぐ。
#[tokio::test]
async fn agent_icon_round_trips_and_rejects_non_webp() {
    let dir = TempDir::new("icon");
    let orchestrator = setup(&dir, OrchestratorConfig::default()).await;
    let id = AgentId::from("agent_01");

    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "IconAgent", "tpl"))
        .await
        .unwrap();

    // 未設定は None（エラーではない）。
    assert_eq!(orchestrator.agent_icon(&id).await.unwrap(), None);

    // WebP でないバイト列は拒否される。PNG のマジックで偽装しても通らない。
    let err = orchestrator
        .set_agent_icon(&id, b"\x89PNG\r\n\x1a\n....")
        .await
        .unwrap_err();
    assert_eq!(err.code(), "INVALID_ICON");

    // サイズ上限（512 KB）超過も拒否される。
    let oversized = webp_bytes(&vec![0u8; 512 * 1024]);
    let err = orchestrator.set_agent_icon(&id, &oversized).await.unwrap_err();
    assert_eq!(err.code(), "INVALID_ICON");

    // 正当な WebP は保存でき、そのまま読み戻せる。
    let icon = webp_bytes(b"icon-payload");
    orchestrator.set_agent_icon(&id, &icon).await.unwrap();
    assert_eq!(orchestrator.agent_icon(&id).await.unwrap().as_deref(), Some(icon.as_slice()));

    // 削除は冪等。
    orchestrator.clear_agent_icon(&id).await.unwrap();
    assert_eq!(orchestrator.agent_icon(&id).await.unwrap(), None);
    orchestrator.clear_agent_icon(&id).await.unwrap();

    // 未登録エージェントには読み書きさせない。
    let ghost = AgentId::from("ghost");
    assert_eq!(
        orchestrator.agent_icon(&ghost).await.unwrap_err().code(),
        "AGENT_NOT_FOUND"
    );
    assert_eq!(
        orchestrator
            .set_agent_icon(&ghost, &webp_bytes(b"x"))
            .await
            .unwrap_err()
            .code(),
        "AGENT_NOT_FOUND"
    );
}

/// エージェント削除でアイコンも消えること（設定ディレクトリごと消す既存挙動の確認）。
#[tokio::test]
async fn deleting_an_agent_removes_its_icon_too() {
    let dir = TempDir::new("icon-cleanup");
    let orchestrator = setup(&dir, OrchestratorConfig::default()).await;
    let id = AgentId::from("agent_01");

    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "IconAgent", "tpl"))
        .await
        .unwrap();
    orchestrator
        .set_agent_icon(&id, &webp_bytes(b"icon"))
        .await
        .unwrap();

    orchestrator.delete_agent(&id).await.unwrap();
    assert!(
        !dir.0.join("agents").join("agent_01").exists(),
        "設定ディレクトリごと消える（アイコンの孤児を残さない）"
    );
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
    // EchoBackend は受信した本文をそのまま返す。本文には送り手の封筒が付く
    // （ユーザーの言葉とエージェントの転送を受信側が区別するため）。
    assert_eq!(log[1].content, "[echo] 【送り手: ユーザー】\n計画を立てて");
    assert!(log[1].tokens > 0, "トークンが計上されること");
}

/// 入力中イベントは処理の開始と終了で対になって流れ、応答を挟むこと。
///
/// `active: false` が流れないと UI の「入力中…」が出しっぱなしになるので、
/// 対であること自体が契約。
#[tokio::test]
async fn typing_events_bracket_the_reply() {
    let dir = TempDir::new("typing");
    let orchestrator = setup(&dir, OrchestratorConfig::default()).await;
    let id = AgentId::from("agent_01");

    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "PlannerAgent", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "こんにちは").await.unwrap();

    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let typing_reply_sequence: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            CoreEvent::AgentTyping { active: true, .. } => Some("typing-on"),
            CoreEvent::AgentTyping { active: false, .. } => Some("typing-off"),
            CoreEvent::MessageSent { message } if matches!(message.from, Endpoint::Agent { .. }) => {
                Some("reply")
            }
            _ => None,
        })
        .collect();

    assert_eq!(
        typing_reply_sequence,
        vec!["typing-on", "reply", "typing-off"],
        "開始 → 応答 → 終了の順で対になること。実際: {events:?}"
    );
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

/// 1 応答内の複数の転送要求が、**全宛先へ**配送されること（fan-out）。
///
/// かつては `Outcome::Handoff` が単一宛先の型で、`decide()` も最初の 1 本で
/// 打ち切っていた。モデルが「みんなへ渡す」つもりで並列ツール呼び出しを
/// 返しても 2 本目以降は黙って捨てられ、「みんなに挨拶して」が
/// 原理的に成立しなかった（ジェミーだけトークン 0 のまま、という形で表面化）。
#[tokio::test]
async fn a_single_response_fans_out_to_every_requested_target() {
    let dir = TempDir::new("fan-out");
    let orchestrator =
        setup_with(&dir, Arc::new(FanOutBackend), OrchestratorConfig::default()).await;
    let (hub, b, c) = (
        AgentId::from("agent_hub"),
        AgentId::from("agent_b"),
        AgentId::from("agent_c"),
    );

    for (id, name) in [(&hub, "Hub"), (&b, "Left"), (&c, "Right")] {
        orchestrator
            .create_agent(AgentSpec::new(id.clone(), name, "tpl"))
            .await
            .unwrap();
    }
    orchestrator
        .set_connections(&hub, vec![b.clone(), c.clone()])
        .await
        .unwrap();
    for id in [&hub, &b, &c] {
        orchestrator.start_agent(id).await.unwrap();
    }

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&hub, "みんなに挨拶して").await.unwrap();

    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    let log = messages(&events);

    // user → hub、hub → b、hub → c、b → user、c → user の 5 件。
    // b / c は接続先が無くツールが提示されないため、そこで会話が終わる。
    assert_eq!(log.len(), 5, "実際: {log:#?}");

    let hub_deliveries: Vec<_> = log
        .iter()
        .filter(|m| m.from == Endpoint::Agent { id: hub.clone() })
        .collect();
    assert_eq!(hub_deliveries.len(), 2, "hub は 2 宛先へ配送する");
    let destinations: Vec<_> = hub_deliveries.iter().map(|m| &m.to).collect();
    assert!(destinations.contains(&&Endpoint::Agent { id: b.clone() }));
    assert!(destinations.contains(&&Endpoint::Agent { id: c.clone() }));

    // 宛先ごとに個別の本文が渡ること（全員に同一文のブロードキャストではない）。
    assert!(hub_deliveries.iter().all(|m| m.content.contains("への挨拶")));
    // 同じターン由来なので hop は揃う。
    assert!(hub_deliveries.iter().all(|m| m.hop == 1));

    // トークンは 1 ターンぶんの消費。宛先数で二重計上せず、先頭の 1 通にだけ載る。
    let tokens: Vec<u32> = hub_deliveries.iter().map(|m| m.tokens).collect();
    assert_eq!(tokens.iter().filter(|t| **t > 0).count(), 1, "実際: {tokens:?}");

    // 双方の枝が独立にユーザーへ返る。
    let finishes: Vec<_> = log.iter().filter(|m| m.to == Endpoint::User).collect();
    assert_eq!(finishes.len(), 2);
}

/// 同じ内容を複数宛先へ渡す fan-out は、エージェント発の同報として封筒に載ること。
///
/// ユーザー同報 (#20) と同じ理屈がエージェント発にも要る。ジェミーが 2 体へ
/// 同じ挨拶を fan-out したとき、受け手同士が「相手はこれを知らない」と誤解して
/// 伝言し合う経路は、ユーザー起点と何も変わらない。
/// 一方、宛先ごとに**内容が違う** fan-out は同報ではないので載せない
/// （「全員が同じ内容を受け取っている」という注記が嘘になる）。
#[tokio::test]
async fn identical_fan_out_is_marked_as_broadcast_but_distinct_messages_are_not() {
    /// 最初の呼び出しで全 transfer_to_* を同一 message で呼ぶバックエンド。
    struct IdenticalFanOutBackend;

    #[async_trait::async_trait]
    impl LlmBackend for IdenticalFanOutBackend {
        fn name(&self) -> &str {
            "identical-fan-out"
        }

        async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
            let tool_calls = req
                .tools
                .iter()
                .filter(|tool| tool.name.starts_with("transfer_to_"))
                .enumerate()
                .map(|(index, tool)| ToolCall {
                    id: format!("call_{index}"),
                    name: tool.name.clone(),
                    args: serde_json::json!({ "message": "はじめまして、よろしく" }),
                    extra: None,
                })
                .collect();
            Ok(ChatResponse {
                text: Some("挨拶します".into()),
                tool_calls,
                finish: Finish::Stop,
                usage: Usage { prompt: 10, completion: 5, cache_read: 0 },
                grounding: Default::default(),
            })
        }
    }

    let dir = TempDir::new("identical-fan-out");
    let orchestrator = setup_with(
        &dir,
        Arc::new(IdenticalFanOutBackend),
        OrchestratorConfig::default(),
    )
    .await;
    let (hub, b, c) = (
        AgentId::from("agent_hub"),
        AgentId::from("agent_b"),
        AgentId::from("agent_c"),
    );
    for (id, name) in [(&hub, "Hub"), (&b, "Left"), (&c, "Right")] {
        orchestrator
            .create_agent(AgentSpec::new(id.clone(), name, "tpl"))
            .await
            .unwrap();
    }
    orchestrator.set_connections(&hub, vec![b.clone(), c.clone()]).await.unwrap();
    for id in [&hub, &b, &c] {
        orchestrator.start_agent(id).await.unwrap();
    }

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&hub, "挨拶して").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    let log = messages(&events);

    // hub からの 2 通は、どちらも同報として宛先 2 体を封筒に持つ。
    let deliveries: Vec<_> = log
        .iter()
        .filter(|m| m.from == Endpoint::Agent { id: hub.clone() } && m.to != Endpoint::User)
        .collect();
    assert_eq!(deliveries.len(), 2, "実際: {log:#?}");
    for delivery in &deliveries {
        assert_eq!(
            delivery.co_recipients.len(),
            2,
            "同内容 fan-out は同報の封筒を持つ: {delivery:#?}"
        );
        assert!(delivery.co_recipients.contains(&b));
        assert!(delivery.co_recipients.contains(&c));
    }
}

/// 宛先ごとに内容が違う fan-out には同報の封筒が付かないこと。
#[tokio::test]
async fn distinct_fan_out_messages_carry_no_broadcast_envelope() {
    let dir = TempDir::new("distinct-fan-out");
    // FanOutBackend は宛先ごとに違う本文（ツール名入り）を渡す。
    let orchestrator =
        setup_with(&dir, Arc::new(FanOutBackend), OrchestratorConfig::default()).await;
    let (hub, b, c) = (
        AgentId::from("agent_hub"),
        AgentId::from("agent_b"),
        AgentId::from("agent_c"),
    );
    for (id, name) in [(&hub, "Hub"), (&b, "Left"), (&c, "Right")] {
        orchestrator
            .create_agent(AgentSpec::new(id.clone(), name, "tpl"))
            .await
            .unwrap();
    }
    orchestrator.set_connections(&hub, vec![b.clone(), c.clone()]).await.unwrap();
    for id in [&hub, &b, &c] {
        orchestrator.start_agent(id).await.unwrap();
    }

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&hub, "個別に頼んで").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    let log = messages(&events);

    let deliveries: Vec<_> = log
        .iter()
        .filter(|m| m.from == Endpoint::Agent { id: hub.clone() } && m.to != Endpoint::User)
        .collect();
    assert_eq!(deliveries.len(), 2);
    for delivery in &deliveries {
        assert!(
            delivery.co_recipients.is_empty(),
            "内容が違う fan-out は同報ではない: {delivery:#?}"
        );
    }
}

/// 同じ宛先への重複した転送要求は 1 通にまとめられること。
///
/// モデルは同じツールを同じ引数で 2 回呼ぶことがある（実際に起きる）。
/// 素通しにすると同一内容が二重配送され、受け手の履歴が汚れる。
#[tokio::test]
async fn duplicate_handoff_requests_to_one_target_are_collapsed() {
    struct DuplicateHandoffBackend;

    #[async_trait::async_trait]
    impl LlmBackend for DuplicateHandoffBackend {
        fn name(&self) -> &str {
            "duplicate-handoff"
        }

        async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
            let tool_calls = match req.tools.iter().find(|t| t.name.starts_with("transfer_to_")) {
                Some(tool) => vec![
                    ToolCall {
                        id: "call_1".into(),
                        name: tool.name.clone(),
                        args: serde_json::json!({ "message": "一通目" }),
                        extra: None,
                    },
                    ToolCall {
                        id: "call_2".into(),
                        name: tool.name.clone(),
                        args: serde_json::json!({ "message": "二通目" }),
                        extra: None,
                    },
                ],
                None => Vec::new(),
            };
            Ok(ChatResponse {
                text: Some("渡します".into()),
                tool_calls,
                finish: Finish::Stop,
                usage: Usage { prompt: 10, completion: 5, cache_read: 0 },
                grounding: Default::default(),
            })
        }
    }

    let dir = TempDir::new("dup-handoff");
    let orchestrator = setup_with(
        &dir,
        Arc::new(DuplicateHandoffBackend),
        OrchestratorConfig::default(),
    )
    .await;
    let (a, b) = (AgentId::from("agent_01"), AgentId::from("agent_02"));

    orchestrator.create_agent(AgentSpec::new(a.clone(), "A", "tpl")).await.unwrap();
    orchestrator.create_agent(AgentSpec::new(b.clone(), "B", "tpl")).await.unwrap();
    orchestrator.set_connections(&a, vec![b.clone()]).await.unwrap();
    orchestrator.start_agent(&a).await.unwrap();
    orchestrator.start_agent(&b).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&a, "始めて").await.unwrap();

    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    let log = messages(&events);

    let deliveries: Vec<_> = log
        .iter()
        .filter(|m| m.from == Endpoint::Agent { id: a.clone() })
        .collect();
    assert_eq!(deliveries.len(), 1, "同一宛先は 1 通に畳む: {log:#?}");
    assert_eq!(deliveries[0].content, "一通目", "先勝ち");
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
    // 履歴の受信側には送り手の封筒が付く（プロンプトと履歴の形を揃える）。
    let second = &seen[1];
    assert!(
        second
            .iter()
            .any(|m| m.role == Role::User && m.content == "【送り手: ユーザー】\n一回目"),
        "前回の受信が封筒付きで履歴に入る: {second:#?}"
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

/// ツール上限で打ち切られたら、ツール無しの最終呼び出しで**ここまでの結果を
/// 文章化**して返すこと。
///
/// 中間のツール結果はそのターンにしか存在しない。まとめずに捨てると、
/// 利用者が「続けて」と送るたびにゼロから調査をやり直して同じ上限に当たり、
/// トークンだけが燃え続ける（実機で 3 ターン連続 146k tok を観測）。
#[tokio::test]
async fn a_tool_limit_cutoff_summarizes_the_findings_so_far() {
    let dir = TempDir::new("tool-limit-summary");
    let backend = Arc::new(EndlessToolBackend::default());
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
    orchestrator.send_user_message(&id, "調べて").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    assert!(
        events
            .iter()
            .any(|e| matches!(e, CoreEvent::ToolLimitReached { .. })),
        "上限に達したことが通知されること"
    );

    let log = messages(&events);
    let reply = log
        .iter()
        .find(|m| matches!(m.from, Endpoint::Agent { .. }))
        .expect("応答が記録されること");
    assert_eq!(
        reply.content, "ここまでの調査のまとめです。",
        "打ち切り時はまとめ呼び出しの本文が応答になること"
    );

    // 毒が残らないこと: 次のターンも普通に処理され、エージェントは落ちない。
    orchestrator.send_user_message(&id, "続けて").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    assert!(
        !events.iter().any(|e| matches!(e, CoreEvent::AgentFailed { .. })),
        "前ターンの打ち切りが次のターンを壊さないこと"
    );
    assert!(
        messages(&events)
            .iter()
            .any(|m| matches!(m.from, Endpoint::Agent { .. })),
        "次のターンも応答が返ること"
    );
}

/// まとめ呼び出しまで無言だった最悪経路でも、空ではなく読める文言が返ること。
#[tokio::test]
async fn a_tool_limit_cutoff_still_produces_a_non_empty_reply() {
    let dir = TempDir::new("tool-limit-empty");
    let orchestrator = setup_with(
        &dir,
        Arc::new(SilentToolBackend),
        OrchestratorConfig::default(),
    )
    .await;
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
    orchestrator.send_user_message(&id, "調べて").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let log = messages(&events);
    let reply = log
        .iter()
        .find(|m| matches!(m.from, Endpoint::Agent { .. }))
        .expect("応答が記録されること");
    assert!(!reply.content.trim().is_empty(), "空の応答を記録しないこと");
    assert!(
        reply.content.contains("上限"),
        "打ち切りの理由が読めること: {}",
        reply.content
    );
}

/// 同梱ツールの提示は enabled_tools と作業フォルダで絞られること。
///
/// 使わないツールのスキーマは毎ターンの固定費になる（トークン節約は
/// 最重要課題）。MCP 由来ツールはこのフィルタの対象外で常に提示される。
#[tokio::test]
async fn bundled_tool_presentation_is_gated_by_enabled_tools_and_work_dir() {
    let dir = TempDir::new("tool-gate");
    let backend = Arc::new(ToolsProbeBackend::default());
    let orchestrator = setup_with(&dir, backend.clone(), OrchestratorConfig::default()).await;
    register_all_tools(&orchestrator, &dir).await;

    // A: 明示 ["remember"] + 作業フォルダあり → remember と MCP 風だけ。
    let mut spec_a = AgentSpec::new("agent_a", "選択型", "tpl");
    spec_a.enabled_tools = Some(vec!["remember".into()]);
    spec_a.work_dir = Some("D:\\somewhere".into());
    orchestrator.create_agent(spec_a).await.unwrap();

    // B: 既定 (null) + 作業フォルダ無し → ファイル系 5 本が自動除外。
    orchestrator
        .create_agent(AgentSpec::new("agent_b", "既定型", "tpl"))
        .await
        .unwrap();

    let mut rx = orchestrator.subscribe();
    for id in ["agent_a", "agent_b"] {
        orchestrator.start_agent(&id.into()).await.unwrap();
        orchestrator.send_user_message(&id.into(), "こんにちは").await.unwrap();
        drain_until_quiet(&mut rx, Duration::from_millis(300)).await;
    }

    let presented = backend.presented.lock().unwrap();
    let a = &presented[0];
    assert!(a.contains(&"remember".to_string()), "{a:?}");
    assert!(a.contains(&"memoria__recall".to_string()), "MCP 由来は対象外: {a:?}");
    assert!(!a.contains(&"grep".to_string()), "列挙外は提示しない: {a:?}");
    assert!(!a.contains(&"yq".to_string()), "{a:?}");

    let b = &presented[1];
    assert!(b.contains(&"remember".to_string()), "{b:?}");
    assert!(b.contains(&"memoria__recall".to_string()), "{b:?}");
    for tool in ["grep", "fd", "diff", "sd", "yq"] {
        assert!(
            !b.contains(&tool.to_string()),
            "作業フォルダ未設定ならファイル系は自動除外: {b:?}"
        );
    }
}

/// 自動除外は明示指定より優先され、空配列は同梱 0 本を意味すること。
#[tokio::test]
async fn work_dir_auto_exclusion_beats_explicit_selection() {
    let dir = TempDir::new("tool-gate-2");
    let backend = Arc::new(ToolsProbeBackend::default());
    let orchestrator = setup_with(&dir, backend.clone(), OrchestratorConfig::default()).await;
    register_all_tools(&orchestrator, &dir).await;

    // C: 明示 ["grep"] だが作業フォルダ無し → 同梱 0 本。
    let mut spec_c = AgentSpec::new("agent_c", "空振り型", "tpl");
    spec_c.enabled_tools = Some(vec!["grep".into()]);
    orchestrator.create_agent(spec_c).await.unwrap();

    // D: 明示 [] + 作業フォルダあり → 同梱 0 本。
    let mut spec_d = AgentSpec::new("agent_d", "丸腰型", "tpl");
    spec_d.enabled_tools = Some(Vec::new());
    spec_d.work_dir = Some("D:\\somewhere".into());
    orchestrator.create_agent(spec_d).await.unwrap();

    let mut rx = orchestrator.subscribe();
    for id in ["agent_c", "agent_d"] {
        orchestrator.start_agent(&id.into()).await.unwrap();
        orchestrator.send_user_message(&id.into(), "こんにちは").await.unwrap();
        drain_until_quiet(&mut rx, Duration::from_millis(300)).await;
    }

    let presented = backend.presented.lock().unwrap();
    for (label, tools) in [("C", &presented[0]), ("D", &presented[1])] {
        for bundled in ["remember", "grep", "fd", "diff", "sd", "yq"] {
            assert!(
                !tools.contains(&bundled.to_string()),
                "{label} に同梱ツールが提示されないこと: {tools:?}"
            );
        }
        assert!(
            tools.contains(&"memoria__recall".to_string()),
            "{label} にも MCP 由来は提示: {tools:?}"
        );
    }
}

/// エージェント別 MCP の状態は稼働に紐付き、壊れた mcp.json でも起動が
/// 止まらないこと（失敗二分類 (1') と (2)）。
#[tokio::test]
async fn agent_mcp_state_follows_the_agent_lifecycle() {
    let dir = TempDir::new("agent-mcp-life");
    let orchestrator = setup(&dir, OrchestratorConfig::default()).await;
    let id = AgentId::from("agent_01");

    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "PlannerAgent", "tpl"))
        .await
        .unwrap();

    // 停止中: 未接続としか答えられない（状態は永続化しない）。
    let idle = orchestrator.agent_mcp_status(&id).await.unwrap();
    assert!(!idle.running);
    assert!(idle.servers.is_empty());

    // 外部編集で壊れた mcp.json（保存経路を迂回してディスクを直接壊す）。
    std::fs::create_dir_all(dir.0.join("agents/agent_01")).unwrap();
    std::fs::write(dir.0.join("agents/agent_01/mcp.json"), "{ broken").unwrap();

    // 起動は成功し、読み込み失敗が状態から読める（分類 1'）。
    orchestrator.start_agent(&id).await.unwrap();
    let broken = orchestrator.agent_mcp_status(&id).await.unwrap();
    assert!(broken.running);
    assert!(broken.load_error.is_some(), "読み込み失敗が保持されること");
    assert_eq!(orchestrator.snapshot(&id).await.unwrap().status, AgentStatus::Running);

    orchestrator.stop_agent(&id).await.unwrap();
    assert!(!orchestrator.agent_mcp_status(&id).await.unwrap().running);

    // 起動しないコマンドの宣言 → 起動は成功し、サーバー単位のエラー（分類 2）。
    std::fs::write(
        dir.0.join("agents/agent_01/mcp.json"),
        r#"{ "mcpServers": { "ghost": { "command": "no-such-command-xyz" } } }"#,
    )
    .unwrap();
    orchestrator.start_agent(&id).await.unwrap();
    let status = orchestrator.agent_mcp_status(&id).await.unwrap();
    assert!(status.running);
    assert!(status.load_error.is_none(), "宣言自体は読めている");
    assert_eq!(status.servers.len(), 1);
    assert_eq!(status.servers[0].name, "ghost");
    assert!(!status.servers[0].connected);
    assert!(status.servers[0].error.is_some(), "接続失敗が読めること");

    orchestrator.stop_agent(&id).await.unwrap();
}

/// 稼働中に mcp.json を保存すると個別接続が張り直されること。
#[tokio::test]
async fn saving_the_agent_mcp_config_reconnects_while_running() {
    let dir = TempDir::new("agent-mcp-save");
    let orchestrator = setup(&dir, OrchestratorConfig::default()).await;
    let id = AgentId::from("agent_01");

    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "PlannerAgent", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    assert!(orchestrator.agent_mcp_status(&id).await.unwrap().servers.is_empty());

    // 稼働中の保存 → 新しい宣言で張り直される。
    orchestrator
        .write_config(
            &id,
            ConfigFileKind::Mcp,
            r#"{ "mcpServers": { "ghost": { "command": "no-such-command-xyz" } } }"#,
        )
        .await
        .unwrap();
    let status = orchestrator.agent_mcp_status(&id).await.unwrap();
    assert_eq!(status.servers.len(), 1, "保存が即座に反映されること");

    // 壊れた JSON は保存拒否（分類 1）で、接続状態は変わらない。
    let err = orchestrator
        .write_config(&id, ConfigFileKind::Mcp, "{ broken")
        .await
        .unwrap_err();
    assert_eq!(err.code(), "SERDE_FAILED");
    assert_eq!(
        orchestrator.agent_mcp_status(&id).await.unwrap().servers.len(),
        1,
        "拒否された保存で接続状態が壊れないこと"
    );

    orchestrator.stop_agent(&id).await.unwrap();
}

/// ツール実行の上限はエージェント個別に上書きできること。
#[tokio::test]
async fn per_agent_tool_iteration_limits_override_the_default() {
    let dir = TempDir::new("tool-limit-override");
    let backend = Arc::new(EndlessToolBackend::default());
    let orchestrator = setup_with(&dir, backend.clone(), OrchestratorConfig::default()).await;
    let id = AgentId::from("agent_01");

    orchestrator
        .register_tool(Arc::new(RememberTool::new(ConfigStore::new(&dir.0))))
        .await;
    let mut spec = AgentSpec::new(id.clone(), "PlannerAgent", "tpl");
    spec.max_tool_iterations = Some(2);
    orchestrator.create_agent(spec).await.unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "調べて").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    assert_eq!(
        *backend.calls.lock().unwrap(),
        3,
        "個別上限 2 でツール周回が止まること（既定 6 ではなく。3 回目はまとめ呼び出し）"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            CoreEvent::ToolLimitReached { max_iterations: 2, .. }
        )),
        "通知にも個別上限の値が載ること"
    );
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
