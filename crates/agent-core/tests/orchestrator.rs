//! オーケストレーターの結合テスト。
//!
//! [`EchoBackend`] を挿すことでネットワークなしに全経路を走らせる。
//! ここで検証したいのは LLM の賢さではなく、**ライフサイクル・配送・打ち切り**の
//! 3 点が仕様どおりに動くこと。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_core::event::CoreEvent;
use agent_core::plan::PlanTaskState;
use agent_core::{
    AgentTool, ConfigFileKind, DiffTool, FdTool, FileTool, GrepTool, RememberTool, SdTool,
    ToolContext, YqTool,
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

/// `plan` が提示されていれば全接続先へ 1 波で撒き、結果を受け取ったら終える。
///
/// 進行役（orchestrator-workers の要）の役回りをネットワークなしで再現する。
/// ワーカー側は素の応答を返すだけなので、束ねの形と並列性だけが観測できる。
struct PlanningBackend {
    /// 進行役が撒く依頼。`None` なら `plan` の tasks をワーカー数ぶん自動生成する。
    tasks: Option<serde_json::Value>,
    /// 同時に処理中だったワーカーの最大数。**並列性はこれで測る**。
    ///
    /// 壁時計で測らないのは、この repo が `drain_until_quiet` で既に
    /// 避けている問題（遅いマシンで取りこぼし・速いマシンで無駄待ち）を
    /// 持ち込まないため。同時実行数は時間に依存せず並列を直接示す。
    in_flight: Arc<std::sync::Mutex<(usize, usize)>>,
    /// ワーカーが 1 回の応答で待つ時間。重なりを作るために要る。
    worker_delay: Duration,
}

impl PlanningBackend {
    fn new() -> Self {
        Self {
            tasks: None,
            in_flight: Arc::new(std::sync::Mutex::new((0, 0))),
            worker_delay: Duration::from_millis(120),
        }
    }

    /// 進行役が撒く tasks を明示する（不正な波の検証用）。
    fn with_tasks(tasks: serde_json::Value) -> Self {
        Self {
            tasks: Some(tasks),
            ..Self::new()
        }
    }

    /// 観測された同時実行数の最大値。
    fn peak_in_flight(&self) -> usize {
        self.in_flight.lock().unwrap().1
    }
}

#[async_trait::async_trait]
impl LlmBackend for PlanningBackend {
    fn name(&self) -> &str {
        "planning"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let usage = Usage {
            prompt: 1,
            completion: 1,
            cache_read: 0,
        };
        let plan = req.tools.iter().find(|tool| tool.name == "plan");
        let answered = req.messages.iter().any(|m| m.role == Role::Tool);

        // 進行役: plan を持っていて、まだ結果を受け取っていない。
        if let (Some(plan), false) = (plan, answered) {
            let tasks = self.tasks.clone().unwrap_or_else(|| {
                // 提示された enum がそのまま接続先の一覧になっている。
                let ids = plan.parameters["properties"]["tasks"]["items"]["properties"]["to"]
                    ["enum"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                serde_json::Value::Array(
                    ids.iter()
                        .map(|id| {
                            serde_json::json!({
                                "to": id,
                                "message": format!("{} への依頼", id.as_str().unwrap_or(""))
                            })
                        })
                        .collect(),
                )
            });
            return Ok(ChatResponse {
                text: Some(String::new()),
                tool_calls: vec![ToolCall {
                    id: "call_plan".into(),
                    name: "plan".into(),
                    args: serde_json::json!({ "tasks": tasks }),
                    extra: None,
                }],
                finish: Finish::ToolUse,
                usage,
                grounding: Default::default(),
            });
        }

        // 進行役の 2 周目: 束ねた結果をそのまま最終出力にする（検証しやすい形）。
        if let Some(result) = req.messages.iter().find(|m| m.role == Role::Tool) {
            return Ok(ChatResponse {
                text: Some(format!("まとめ\n{}", result.content)),
                tool_calls: Vec::new(),
                finish: Finish::Stop,
                usage,
                grounding: Default::default(),
            });
        }

        // ワーカー: 在室時間を作って重なりを観測する。
        {
            let mut guard = self.in_flight.lock().unwrap();
            guard.0 += 1;
            guard.1 = guard.1.max(guard.0);
        }
        tokio::time::sleep(self.worker_delay).await;
        self.in_flight.lock().unwrap().0 -= 1;

        Ok(ChatResponse {
            text: Some("作業しました".into()),
            tool_calls: Vec::new(),
            finish: Finish::Stop,
            usage,
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

/// ツールの使用が許されている限り**ツール呼び出しだけ**を返すバックエンド。
///
/// ツール実行上限による打ち切りの経路を再現する。実機では「調査系の依頼で
/// モデルがツールを呼び続け、上限に達した周の応答にテキストが無い」形で起きる。
/// `tool_choice: None`（まとめ呼び出し）ならテキストを返す。
///
/// まとめ呼び出しの判定を `tools.is_empty()` にしないのが要点 — まとめでも
/// **tools は空にならない**（履歴に tool ブロックが残る限り、tools の定義は
/// ワイヤ契約の一部。空にすると Anthropic が 400 を返す。failures.md #36）。
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

        if req.tool_choice == agent_core::llm::ToolChoice::None {
            assert!(
                !req.tools.is_empty(),
                "まとめ呼び出しでも tools の定義は残ること（空だと実プロバイダで 400）"
            );
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

/// 同じツールを**同じ引数で**呼び続けるバックエンド（失敗ループの再現）。
///
/// 実機で燃えた形（failures.md #39 / #41）そのもの: 引数を変えずに呼び直し、
/// 同じ結果を受け取り、また同じ引数で呼ぶ。
#[derive(Default)]
struct StuckToolBackend {
    /// tool_choice: None（まとめ）以外の呼び出し回数。
    calls: std::sync::Mutex<usize>,
}

#[async_trait::async_trait]
impl LlmBackend for StuckToolBackend {
    fn name(&self) -> &str {
        "stuck-tool"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        if req.tool_choice == agent_core::llm::ToolChoice::None {
            return Ok(ChatResponse {
                text: Some("同じ操作しかできず、目的は果たせませんでした。".into()),
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

        let mut calls = self.calls.lock().unwrap();
        *calls += 1;
        let n = *calls;
        Ok(ChatResponse {
            text: None,
            tool_calls: vec![ToolCall {
                // id だけは毎回変わる（実プロバイダと同じ）。同一判定に使わない。
                id: format!("call_{n}"),
                name: "stuck_probe".into(),
                args: serde_json::json!({ "path": "存在しない.md" }),
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

/// 何度呼んでも同じ本文を返すツール。
///
/// **失敗を `Err` ではなく `Ok` の本文で返す**のは同梱ツール（`file` / `sd` /
/// `grep`）と同じ作法。繰り返し検出がこの形を数えられないと、実機の失敗ループは
/// 1 件も捕まらない。
#[derive(Default)]
struct StuckTool {
    /// 実行された回数。
    runs: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl AgentTool for StuckTool {
    fn name(&self) -> &str {
        "stuck_probe"
    }
    fn description(&self) -> String {
        "テスト用。いつも同じ失敗を返す".into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    async fn call(
        &self,
        _ctx: &ToolContext,
        _args: &serde_json::Value,
    ) -> agent_core::CoreResult<String> {
        self.runs
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok("`存在しない.md` を読めません: 見つかりません".into())
    }
}

/// 毎周「同じ読み直し 1 本 + 新しい仕事 1 本」を**並列で**呼ぶバックエンド。
///
/// 実機の主な形（2026-07-31 のログ）。隣接だけを見る判定はこの形で必ず数えが
/// 切れて 1 件も発火しなかった。ここで検証するのは 2 つ:
/// 重複した 1 本だけが止まること、**ターンは止まらない**こと。
#[derive(Default)]
struct MixedToolBackend {
    calls: std::sync::Mutex<usize>,
}

#[async_trait::async_trait]
impl LlmBackend for MixedToolBackend {
    fn name(&self) -> &str {
        "mixed-tool"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        if req.tool_choice == agent_core::llm::ToolChoice::None {
            return Ok(ChatResponse {
                text: Some("まとめました。".into()),
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

        let n = {
            let mut calls = self.calls.lock().unwrap();
            *calls += 1;
            *calls
        };
        Ok(ChatResponse {
            text: None,
            tool_calls: vec![
                // 毎周まったく同じ読み直し。
                ToolCall {
                    id: format!("stuck_{n}"),
                    name: "stuck_probe".into(),
                    args: serde_json::json!({ "path": "存在しない.md" }),
                    extra: None,
                },
                // 毎周ちがう仕事。これがある限り、その周は空振りではない。
                ToolCall {
                    id: format!("fresh_{n}"),
                    name: "remember".into(),
                    args: serde_json::json!({ "note": format!("調査メモ {n}") }),
                    extra: None,
                },
            ],
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
///
/// **note は毎回変える。** 固定にすると `remember` の重複排除で結果本文まで
/// 同一になり、上限（12 周）へ届く前に繰り返し検出が切ってしまう
/// （failures.md #41 の処方 1。ここで検証したいのは上限側の経路）。
#[derive(Default)]
struct SilentToolBackend {
    calls: std::sync::Mutex<usize>,
}

#[async_trait::async_trait]
impl LlmBackend for SilentToolBackend {
    fn name(&self) -> &str {
        "silent-tool"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let n = {
            let mut calls = self.calls.lock().unwrap();
            *calls += 1;
            *calls
        };
        let tool_calls = if req.tools.is_empty() {
            Vec::new()
        } else {
            vec![ToolCall {
                id: format!("call_s{n}"),
                name: "remember".into(),
                args: serde_json::json!({ "note": format!("沈黙 {n}") }),
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

/// 同梱 7 本 + MCP 風 1 本を登録する（提示制御テストの土台）。
///
/// **同梱ツールを足したらここにも足すこと。** 登録漏れがあると、提示制御の
/// テストがそのツールだけ素通しになる（Spec 09 の file で実際に漏れた）。
async fn register_all_tools(orchestrator: &Orchestrator, dir: &TempDir) {
    let store = ConfigStore::new(&dir.0);
    orchestrator.register_tool(Arc::new(RememberTool::new(store))).await;
    orchestrator.register_tool(Arc::new(GrepTool)).await;
    orchestrator.register_tool(Arc::new(FdTool)).await;
    orchestrator.register_tool(Arc::new(DiffTool)).await;
    orchestrator.register_tool(Arc::new(SdTool)).await;
    orchestrator.register_tool(Arc::new(YqTool)).await;
    orchestrator.register_tool(Arc::new(FileTool)).await;
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

/// 提示されたツール名だけを記録するバックエンド。提示条件の検証に使う。
#[derive(Default)]
struct ToolNameBackend {
    seen: std::sync::Mutex<Vec<Vec<String>>>,
}

#[async_trait::async_trait]
impl LlmBackend for ToolNameBackend {
    fn name(&self) -> &str {
        "tool-names"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.seen
            .lock()
            .unwrap()
            .push(req.tools.iter().map(|t| t.name.clone()).collect());
        Ok(ChatResponse {
            text: Some("はい".into()),
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

/// 依頼に自分で答えず、接続先へ転送してしまうワーカーを含む配置。
///
/// 進行役（`plan` を持つ）→ ワーカー（転送ツールを持つ）→ 第三者、の 3 役を
/// ツールの顔ぶれで見分ける。
#[derive(Default)]
struct TransferringWorkerBackend;

/// 依頼主がワーカーへ投げる文面。役の見分けに使う目印。
const WORKER_REQUEST: &str = "ワーカー宛の調査依頼";

#[async_trait::async_trait]
impl LlmBackend for TransferringWorkerBackend {
    fn name(&self) -> &str {
        "transferring-worker"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let usage = Usage {
            prompt: 1,
            completion: 1,
            cache_read: 0,
        };
        let answered = req.messages.iter().any(|m| m.role == Role::Tool);
        // 役は**受け取った依頼の文面**で見分ける。ツールの顔ぶれでは分けられない —
        // ワーカーは接続先を持つ以上 `transfer_to_*` と `ask_*` の両方を持つので、
        // 「ask を持っていれば依頼主」という判定はワーカーにも当たってしまう
        // （実際に当たって、転送ではなく委譲が起きた）。
        let incoming = req
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .map(|m| m.content.clone())
            .unwrap_or_default();
        let is_worker = incoming.contains(WORKER_REQUEST);

        // 依頼主: ask を 1 回だけ投げ、結果を受け取ったらそれを最終出力にする。
        if let Some(ask) = req.tools.iter().find(|t| t.name.starts_with("ask_"))
            && !is_worker
        {
            if !answered {
                return Ok(ChatResponse {
                    text: Some(String::new()),
                    tool_calls: vec![ToolCall {
                        id: "call_1".into(),
                        name: ask.name.clone(),
                        args: serde_json::json!({ "message": WORKER_REQUEST }),
                        extra: None,
                    }],
                    finish: Finish::ToolUse,
                    usage,
                    grounding: Default::default(),
                });
            }
            let result = req
                .messages
                .iter()
                .find(|m| m.role == Role::Tool)
                .map(|m| m.content.clone())
                .unwrap_or_default();
            return Ok(ChatResponse {
                text: Some(format!("受領: {result}")),
                tool_calls: Vec::new(),
                finish: Finish::Stop,
                usage,
                grounding: Default::default(),
            });
        }

        // ワーカー: 自分で答えず、接続先へ会話を渡してしまう。
        if let Some(transfer) = req.tools.iter().find(|t| t.name.starts_with("transfer_to_"))
            && is_worker
        {
            return Ok(ChatResponse {
                text: Some("私では分かりません".into()),
                tool_calls: vec![ToolCall {
                    id: "call_2".into(),
                    name: transfer.name.clone(),
                    args: serde_json::json!({ "message": "代わりに答えて" }),
                    extra: None,
                }],
                finish: Finish::ToolUse,
                usage,
                grounding: Default::default(),
            });
        }

        Ok(ChatResponse {
            text: Some("第三者の答え".into()),
            tool_calls: Vec::new(),
            finish: Finish::Stop,
            usage,
            grounding: Default::default(),
        })
    }
}

/// 進行役 1 体 + ワーカー N 体を組み、全員を起動する。
async fn setup_facilitator(
    dir: &TempDir,
    backend: Arc<dyn LlmBackend>,
    workers: &[(&str, &str)],
    config: OrchestratorConfig,
) -> (Orchestrator, AgentId, Vec<AgentId>) {
    let orchestrator = setup_with(dir, backend, config).await;
    let lead = AgentId::from("agent_lead");
    orchestrator
        .create_agent(AgentSpec::new(lead.clone(), "進行役", "tpl"))
        .await
        .unwrap();

    let mut ids = Vec::new();
    for (id, name) in workers {
        let worker = AgentId::from(*id);
        orchestrator
            .create_agent(AgentSpec::new(worker.clone(), *name, "tpl"))
            .await
            .unwrap();
        ids.push(worker);
    }
    orchestrator.set_connections(&lead, ids.clone()).await.unwrap();
    orchestrator.start_agent(&lead).await.unwrap();
    for worker in &ids {
        orchestrator.start_agent(worker).await.unwrap();
    }
    (orchestrator, lead, ids)
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
        // 同報かどうかは発話ごとに変わるので System では積まず、最終発話へ
        // 畳んでいる（System で積むと adapter が先頭へ畳んで前方一致を切る。
        // failures.md #45）。畳んだ発話には入退室なども同居するので、
        // **セクション単位**（空行区切り）で注記だけを取り出す — 発話全体で見ると
        // 「宛先外の名前を列挙しない」の検査が同居した入退室の名前を拾って落ちる。
        let note = messages
            .iter()
            .flat_map(|m| m.content.split("\n\n"))
            .find(|section| section.contains("同報"))
            .expect("同報の注記が入ること");
        assert!(note.contains("アルファ"), "実際: {}", note);
        assert!(note.contains("ブラボー"), "実際: {}", note);
        assert!(
            !note.contains("チャーリー"),
            "宛先外の名前を列挙しない: {}",
            note
        );
        assert!(
            note.contains("転送する必要はありません"),
            "転送不要の根拠を伝える: {}",
            note
        );
        // 転送だけを禁じても、「代わりに促す」経路が残る。実機では
        // 「ユーザーから依頼です、自己紹介お願いします」という**新しい発話**を
        // 他の参加者へ配って回り、同じ混乱が起きた。
        assert!(
            note.contains("促す必要もありません"),
            "発言を促す必要も無いことを伝える: {}",
            note
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

/// 広場ログが**抜粋である**ことと、全文の取り方をモデルへ書くこと。
///
/// 起点は実機（2026-08-04）— 利用者が「チャットの全文が渡っていない、途中で
/// 途切れているとソネットが報告する」と言った。原因は広場ログの 200 字打ち切りで
/// 機構は設計どおりだったが、届く本文には `…` しか無く、**「省略された」のか
/// 「相手がそこで言い終えた」のかを区別できない**。
///
/// 打ち切りは母数と次の手を書く規律（failures.md #44 / #55）は、同梱ツールだけで
/// なく**プロンプト合成にも掛かる**。#55 の一般化 1 —「黙って切らない」は
/// 「切ったと言う」ではなく「切る前の量を言う」。
#[tokio::test]
async fn the_room_log_declares_it_is_an_excerpt_and_how_to_get_the_full_text() {
    #[derive(Default)]
    struct LongBackend {
        seen: std::sync::Mutex<Vec<Vec<ChatMessage>>>,
    }

    #[async_trait::async_trait]
    impl LlmBackend for LongBackend {
        fn name(&self) -> &str {
            "long"
        }
        async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
            self.seen.lock().unwrap().push(req.messages.clone());
            Ok(ChatResponse {
                text: Some("あ".repeat(500)),
                tool_calls: Vec::new(),
                finish: Finish::Stop,
                usage: Usage { prompt: 1, completion: 1, cache_read: 0 },
                grounding: Default::default(),
            })
        }
    }

    let backend = Arc::new(LongBackend::default());
    let dir = TempDir::new("room-excerpt");
    let orchestrator = setup_with(
        &dir,
        Arc::clone(&backend) as Arc<dyn LlmBackend>,
        OrchestratorConfig::default(),
    )
    .await;

    let (a, b) = (AgentId::from("agent_a"), AgentId::from("agent_b"));
    orchestrator
        .create_agent(AgentSpec::new(a.clone(), "アルファ", "tpl"))
        .await
        .unwrap();
    orchestrator
        .create_agent(AgentSpec::new(b.clone(), "ブラボー", "tpl"))
        .await
        .unwrap();
    for id in [&a, &b] {
        orchestrator.start_agent(id).await.unwrap();
    }

    let mut rx = orchestrator.subscribe();
    // a に 500 字を喋らせ、b の広場ログの原料にする。
    orchestrator.send_user_message(&a, "長く話して").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    orchestrator.send_user_message(&b, "どう？").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let requests = backend.seen.lock().unwrap().clone();
    let joined = requests[1]
        .iter()
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        joined.contains("この場で交わされていた会話"),
        "前提: b に広場ログが載ること。実際:\n{joined}"
    );
    assert!(
        joined.contains("抜粋"),
        "抜粋であることを明示すること（`…` だけでは省略と読めない）。実際:\n{joined}"
    );
    assert!(
        joined.contains("全 500 字"),
        "切った行に元の長さを書くこと（母数が無いと「全部見た」と読まれる = #55）。実際:\n{joined}"
    );
    assert!(
        joined.contains("ask"),
        "全文を得る次の手を書くこと（#44。歯止めの先に道が要る）。実際:\n{joined}"
    );
}

/// **送った user 発話が、次のターンの履歴にそのまま現れること。**
///
/// 食い違うとその位置で前方一致が切れる。プロバイダに依らない — Anthropic の
/// 明示的な breakpoint も、一致するプレフィックスが無ければ読み取りに落ちない。
/// 切れた先がどれだけ伸びても載らないので、**会話を続けるほど率が下がる**。
#[tokio::test]
async fn what_we_send_is_what_the_next_turn_replays_as_history() {
    let backend = Arc::new(RecordingBackend::default());
    let dir = TempDir::new("send-equals-store");
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
    orchestrator.send_user_message(&id, "一度目").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    orchestrator.send_user_message(&id, "二度目").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let requests = backend.seen.lock().unwrap().clone();
    assert_eq!(requests.len(), 2, "2 ターン記録されること");

    let sent_first = requests[0]
        .last()
        .expect("1 ターン目の最終発話")
        .content
        .clone();

    assert!(
        requests[1].iter().any(|m| m.content == sent_first),
        "1 ターン目に送った発話が 2 ターン目の履歴へそのまま乗ること。\
         乗らないとその位置で前方一致が切れ、以後いくら会話が伸びても\
         キャッシュは system + tools で頭打ちになる。\n\
         送った: {sent_first:?}\n\
         2 ターン目の中身: {:?}",
        requests[1].iter().map(|m| &m.content).collect::<Vec<_>>()
    );
}

/// 毎ターン変わるブロックは**履歴より後ろ**に置くこと。
///
/// プロンプトキャッシュは前方一致で効く。毎ターン変わるものを伸びる履歴より前に
/// 置くと**そこで一致が切れ、安定プレフィックスが二度と伸びない** — 載るのは
/// system の 1,500 トークン前後だけになり、Gemini の最小長 4,096 に届かず
/// 暗黙キャッシュが無言で no-op する（2026-08-01 実測。failures.md #45）。
///
/// **順序を戻しても他のテストは 1 本も落ちない**ので、ここで固定する。
/// 壊れても画面に出るのは「キャッシュ 0%」だけで、原因は請求まで分からない。
#[tokio::test]
async fn volatile_blocks_sit_after_the_history_so_the_cached_prefix_can_grow() {
    let backend = Arc::new(RecordingBackend::default());
    let dir = TempDir::new("cache-order");
    let orchestrator = setup_with(
        &dir,
        Arc::clone(&backend) as Arc<dyn LlmBackend>,
        OrchestratorConfig::default(),
    )
    .await;

    let (a, b) = (AgentId::from("agent_a"), AgentId::from("agent_b"));
    orchestrator
        .create_agent(AgentSpec::new(a.clone(), "アルファ", "tpl"))
        .await
        .unwrap();
    orchestrator
        .create_agent(AgentSpec::new(b.clone(), "ブラボー", "tpl"))
        .await
        .unwrap();
    for id in [&a, &b] {
        orchestrator.start_agent(id).await.unwrap();
    }

    let mut rx = orchestrator.subscribe();
    // a に履歴を作る。
    orchestrator.send_user_message(&a, "一度目の依頼").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    // b を喋らせて a の広場ログの原料にする（自分の発話は自分の広場ログに載らない）。
    orchestrator.send_user_message(&b, "別件").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    // a の 2 度目。ここで履歴と広場ログが両方載る。
    orchestrator.send_user_message(&a, "二度目の依頼").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let requests = backend.seen.lock().unwrap().clone();
    let last = requests.last().expect("a の 2 度目のリクエストがあること");

    let history_at = last
        .iter()
        .position(|m| m.content.contains("一度目の依頼"))
        .expect("履歴が載ること");
    let room_at = last
        .iter()
        .position(|m| m.content.contains("この場で交わされていた会話"))
        .expect("広場ログが載ること");

    assert!(
        room_at > history_at,
        "広場ログは履歴より後ろに置くこと（前に置くと前方一致がそこで切れる）: \
         history_at={history_at} room_at={room_at}"
    );

    // **位置だけでは足りない。** adapter は Role::System のメッセージを配列の
    // どこにあっても全部引き抜いて 1 つの system / systemInstruction へ連結する
    // （gemini.rs / anthropic.rs の encode）。System で積んだ時点で、履歴の後ろに
    // 置いても前方一致の先頭へ戻る — 実際にそれで 1 度直し損ねた（failures.md #45）。
    assert!(
        !last
            .iter()
            .any(|m| m.role == Role::System && m.content.contains("この場で交わされていた会話")),
        "広場ログを Role::System で積まないこと（位置に関係なく先頭へ畳まれる）"
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

    orchestrator.reset_conversation().await.unwrap();
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
    orchestrator.reset_conversation().await.unwrap();

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
    //
    // 完全一致では見ない — 最終発話には**その周だけの文脈**（入退室・広場ログ・
    // 参照資料）が前置きされる。System で積むと adapter が先頭へ畳んで前方一致を
    // 切るので、そちらへは置けない（failures.md #45）。封筒と本文が末尾に、
    // 壊れずに乗っていることを見る。
    assert!(
        log[1].content.starts_with("[echo] "),
        "実際: {}",
        log[1].content
    );
    assert!(
        log[1].content.ends_with("【送り手: ユーザー】\n計画を立てて"),
        "封筒と本文が末尾に壊れずに乗ること。実際: {}",
        log[1].content
    );
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
    //
    // 完全一致では見ない — 履歴には**送った文字列がそのまま**入り、その周だけの
    // 文脈（入退室・広場ログ・参照資料）が前置きされる。揃えないと次のターンで
    // 前方一致がその位置で切れる（failures.md #45）。封筒と本文が末尾に来ることを見る。
    let second = &seen[1];
    assert!(
        second
            .iter()
            .any(|m| m.role == Role::User && m.content.ends_with("【送り手: ユーザー】\n一回目")),
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

/// **その周のツールが全部止まったら**ループを切ること（failures.md #41 の処方 1）。
///
/// 上限（既定 12 周）まで走らせないことが本体。回数の上限はコストの上限に
/// ならない（1 周ごとに履歴を送り直すので単位コストが増え続ける）ため、
/// 「回数を減らす」のではなく「無駄な回数を発生させない」側で止める。
#[tokio::test]
async fn a_repeated_identical_tool_call_is_blocked_before_the_iteration_limit() {
    let dir = TempDir::new("repeat-guard");
    let backend = Arc::new(StuckToolBackend::default());
    let orchestrator = setup_with(&dir, backend.clone(), OrchestratorConfig::default()).await;
    let id = AgentId::from("agent_01");

    let tool = Arc::new(StuckTool::default());
    orchestrator.register_tool(tool.clone()).await;
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "PlannerAgent", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "読んで").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    // 3 回目は実行していない。上限（12）まで走っていたら 12 になる欄。
    assert_eq!(
        tool.runs.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "同じ結果が 2 回続いた時点で実行を止めること"
    );

    assert!(
        events.iter().any(|e| matches!(
            e,
            CoreEvent::ToolRepeatBlocked { tool, repeats: 2, .. } if tool == "stuck_probe"
        )),
        "繰り返しでの打ち切りが通知されること: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, CoreEvent::ToolLimitReached { .. })),
        "当たったのは上限ではないので、上限到達は通知しないこと"
    );

    // 燃えたぶんの成果は答えに変える（上限打ち切りと同じ規律）。
    let log = messages(&events);
    let reply = log
        .iter()
        .find(|m| matches!(m.from, Endpoint::Agent { .. }))
        .expect("応答が記録されること");
    assert_eq!(
        reply.content, "同じ操作しかできず、目的は果たせませんでした。",
        "打ち切り後もまとめ呼び出しの本文が応答になること"
    );

    // 毒が残らないこと（履歴の対が崩れていれば次のターンが 400 で落ちる）。
    orchestrator.send_user_message(&id, "続けて").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, CoreEvent::AgentFailed { .. })),
        "打ち切ったターンが次のターンを壊さないこと"
    );
}

/// 重複した 1 本だけを止め、**ターンは止めない**こと（failures.md #41 の処方 1）。
///
/// 実機の主な形は「同じ読み直し 1 本 + 新しい仕事 1 本」の並列呼び出しで、
/// 隣接だけを見ていた最初の実装はここで数えが切れて 1 件も発火しなかった。
/// 逆に、重複を見つけるたびにループごと切ると、**並列の 1 本が重複しただけで
/// 進行中の作業を殺す**。止めるのはその 1 本、というのがこのテストの本体。
#[tokio::test]
async fn a_duplicate_call_is_blocked_without_stopping_the_turn() {
    let dir = TempDir::new("repeat-guard-mixed");
    let orchestrator = setup_with(
        &dir,
        Arc::new(MixedToolBackend::default()),
        OrchestratorConfig::default(),
    )
    .await;
    let id = AgentId::from("agent_01");

    let tool = Arc::new(StuckTool::default());
    orchestrator.register_tool(tool.clone()).await;
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
    let events = drain_until_quiet(&mut rx, Duration::from_millis(600)).await;

    assert_eq!(
        tool.runs.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "重複した呼び出しは 3 回目から実行しないこと（間に別の呼び出しが挟まっても）"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            CoreEvent::ToolRepeatBlocked { tool, .. } if tool == "stuck_probe"
        )),
        "止めたことが通知されること"
    );

    // ターンは続いている。新しい仕事のほうは上限まで走り切る。
    let remembers = events
        .iter()
        .filter(|e| matches!(e, CoreEvent::ToolInvoked { tool, .. } if tool == "remember"))
        .count();
    assert!(
        remembers > 2,
        "重複を止めてもターンは続くこと（remember の実行回数: {remembers}）"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, CoreEvent::ToolLimitReached { .. })),
        "止まらずに走り切った先は上限であること"
    );

    let log = messages(&events);
    let reply = log
        .iter()
        .find(|m| matches!(m.from, Endpoint::Agent { .. }))
        .expect("応答が記録されること");
    assert!(!reply.content.trim().is_empty(), "空の応答を記録しないこと");
}

/// まとめ呼び出しまで無言だった最悪経路でも、空ではなく読める文言が返ること。
#[tokio::test]
async fn a_tool_limit_cutoff_still_produces_a_non_empty_reply() {
    let dir = TempDir::new("tool-limit-empty");
    let orchestrator = setup_with(
        &dir,
        Arc::new(SilentToolBackend::default()),
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

    // B: 既定 (null) + 作業フォルダ無し → ファイル系 6 本が自動除外。
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
    for tool in ["grep", "fd", "diff", "sd", "yq", "file"] {
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

/// 未知のツール名は**捨てずにモデルへ返し**、モデルが自分で直せること。
///
/// 以前はここで呼び出しごと落としていた（`tool_result` もログも残らない）。
/// モデルから見ると「呼んだのに何も起きない」ので、実機では実在しない名前の
/// 呼び出しが静かに消え、本文だけが答えとして配信された（2026-08-02）。
/// `execute_tool` には「そのツールはありません」という文言が元からあるのに、
/// 捨てられた呼び出しはそこへ到達できなかった（到達不能な分岐）。
#[tokio::test]
async fn an_unknown_tool_name_is_reported_back_so_the_model_can_recover() {
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

    // 「無い」と伝えたうえで、もう 1 周モデルに機会を渡す（自己修正の余地）。
    assert_eq!(
        *backend.calls.lock().unwrap(),
        2,
        "捨てて終わりにせず、結果を返してもう 1 周回すこと"
    );

    // モデルの手元に「その名前は無い」が届いていること。
    let last = backend.last.lock().unwrap().clone();
    let told = last.iter().any(|m| {
        m.tool_name.as_deref() == Some("does_not_exist")
            && m.content.contains("というツールはありません")
    });
    assert!(told, "未知の名前は tool_result として返ること: {last:#?}");

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

// ---------------------------------------------------------------------------
// 並列委譲（plan / Spec 04）
// ---------------------------------------------------------------------------

/// 1 波が並列に届き、束ねた結果が依頼主へ戻ること。
///
/// **並列性は壁時計ではなく「同時に処理中だったワーカー数の最大値」で測る。**
/// 固定時間に依存するテストは遅いマシンで壊れ、速いマシンで無駄に待つ
/// （この repo が `drain_until_quiet` で既に避けている問題）。
#[tokio::test]
async fn plan_delivers_in_parallel_and_bundles_the_answers() {
    let dir = TempDir::new("plan-parallel");
    let backend = Arc::new(PlanningBackend::new());
    let (orchestrator, lead, workers) = setup_facilitator(
        &dir,
        backend.clone(),
        &[("agent_w1", "一号"), ("agent_w2", "二号")],
        OrchestratorConfig::default(),
    )
    .await;

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&lead, "手分けして調べて").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(600)).await;
    let log = messages(&events);

    assert_eq!(
        backend.peak_in_flight(),
        2,
        "2 体が同時に処理中になること（並列配送の実証）"
    );

    let summary = log
        .iter()
        .find(|m| m.from == Endpoint::Agent { id: lead.clone() } && m.to == Endpoint::User)
        .expect("進行役がユーザーへ返すこと");

    // 見出しは `agent_id（表示名）`。表示名だけにすると同名の 2 体を区別できない。
    assert!(summary.content.contains("## agent_w1（一号）"), "{}", summary.content);
    assert!(summary.content.contains("## agent_w2（二号）"), "{}", summary.content);
    // 順序は入力順に戻す。
    let first = summary.content.find("agent_w1").unwrap();
    let second = summary.content.find("agent_w2").unwrap();
    assert!(first < second, "束ねの順序が入力順であること: {}", summary.content);

    // 答えはユーザーへ散らない。ワーカーの発話は依頼主宛として記録される。
    for worker in &workers {
        let reply = log
            .iter()
            .find(|m| m.from == Endpoint::Agent { id: worker.clone() })
            .unwrap_or_else(|| panic!("{worker} の発話が記録されること"));
        assert_eq!(
            reply.to,
            Endpoint::Agent { id: lead.clone() },
            "ワーカーの答えは依頼主へ戻ること: {reply:#?}"
        );
    }
}

/// 接続先が 1 体以下なら plan を提示しないこと。
///
/// 使えない選択肢のスキーマは毎ターンの固定費になる（トークン節約は最重要課題）。
#[tokio::test]
async fn plan_is_not_offered_below_two_connections() {
    let dir = TempDir::new("plan-not-offered");
    let backend = Arc::new(ToolNameBackend::default());
    let (orchestrator, lead, _) = setup_facilitator(
        &dir,
        backend.clone(),
        &[("agent_w1", "一号")],
        OrchestratorConfig::default(),
    )
    .await;

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&lead, "調べて").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let seen = backend.seen.lock().unwrap();
    let presented = seen.first().expect("リクエストが記録されること");
    assert!(
        !presented.iter().any(|name| name == "plan"),
        "接続 1 体では plan を出さないこと: {presented:?}"
    );
    assert!(
        presented.iter().any(|name| name.starts_with("ask_")),
        "委譲は出ること（plan の代わりにこちらで足りる）: {presented:?}"
    );
}

/// 静的な不正は**何も配送せず**差し戻すこと（部分実行を作らない）。
#[tokio::test]
async fn an_invalid_target_cancels_the_whole_wave() {
    let dir = TempDir::new("plan-invalid-target");
    // 1 件目は正当、2 件目は接続外。1 件目も配送されてはいけない。
    let backend = Arc::new(PlanningBackend::with_tasks(serde_json::json!([
        { "to": "agent_w1", "message": "これは正当" },
        { "to": "agent_stranger", "message": "接続していない相手" }
    ])));
    let (orchestrator, lead, workers) = setup_facilitator(
        &dir,
        backend,
        &[("agent_w1", "一号"), ("agent_w2", "二号")],
        OrchestratorConfig::default(),
    )
    .await;

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&lead, "頼んで").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(500)).await;
    let log = messages(&events);

    for worker in &workers {
        assert!(
            !log.iter().any(|m| m.to == Endpoint::Agent { id: worker.clone() }),
            "1 件でも不正なら誰にも配送しないこと（{worker} へ届いている）"
        );
    }
    let summary = log
        .iter()
        .find(|m| m.to == Endpoint::User)
        .expect("進行役がユーザーへ返すこと");
    assert!(
        summary.content.contains("接続先ではありません"),
        "理由が読める文言で返ること: {}",
        summary.content
    );
    // 波 = 配送が起きた単位（Spec 08）。差し戻しは波として記録しない。
    assert!(
        orchestrator.list_plan_waves().await.is_empty(),
        "配送ゼロの plan は波として記録しないこと"
    );
}

/// 同一宛先の重複も静的な不正として全体を差し戻すこと。
#[tokio::test]
async fn a_duplicate_target_cancels_the_whole_wave() {
    let dir = TempDir::new("plan-duplicate");
    let backend = Arc::new(PlanningBackend::with_tasks(serde_json::json!([
        { "to": "agent_w1", "message": "1 件目" },
        { "to": "agent_w1", "message": "2 件目" }
    ])));
    let (orchestrator, lead, workers) = setup_facilitator(
        &dir,
        backend,
        &[("agent_w1", "一号"), ("agent_w2", "二号")],
        OrchestratorConfig::default(),
    )
    .await;

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&lead, "頼んで").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(500)).await;
    let log = messages(&events);

    assert!(
        !log.iter().any(|m| m.to == Endpoint::Agent { id: workers[0].clone() }),
        "重複があれば何も配送しないこと"
    );
    let summary = log.iter().find(|m| m.to == Endpoint::User).unwrap();
    assert!(
        summary.content.contains("2 回あります"),
        "理由が読める文言で返ること: {}",
        summary.content
    );
}

/// 空の波は静的な不正。通すと何も配送されない空の束ねが返り、hop だけ消える。
#[tokio::test]
async fn an_empty_wave_is_refused() {
    let dir = TempDir::new("plan-empty");
    let backend = Arc::new(PlanningBackend::with_tasks(serde_json::json!([])));
    let (orchestrator, lead, _) = setup_facilitator(
        &dir,
        backend,
        &[("agent_w1", "一号"), ("agent_w2", "二号")],
        OrchestratorConfig::default(),
    )
    .await;

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&lead, "頼んで").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(500)).await;

    let summary = messages(&events)
        .into_iter()
        .find(|m| m.to == Endpoint::User)
        .expect("進行役がユーザーへ返すこと");
    assert!(
        summary.content.contains("tasks が空"),
        "空の波は理由つきで断ること: {}",
        summary.content
    );
}

/// 1 件だけの波は**許容する**。
///
/// 「1 体なら `ask_*` で足りる」はツールの提示条件の話で、波の大きさの話ではない。
/// 進行役が 2 波目で 1 体にだけ追加調査を頼むのは正当で、そこでツールを
/// 持ち替えさせる理由が無い。
#[tokio::test]
async fn a_single_task_wave_is_allowed() {
    let dir = TempDir::new("plan-single");
    let backend = Arc::new(PlanningBackend::with_tasks(serde_json::json!([
        { "to": "agent_w2", "message": "君だけに頼む" }
    ])));
    let (orchestrator, lead, _) = setup_facilitator(
        &dir,
        backend,
        &[("agent_w1", "一号"), ("agent_w2", "二号")],
        OrchestratorConfig::default(),
    )
    .await;

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&lead, "頼んで").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(600)).await;
    let log = messages(&events);

    assert!(
        log.iter().any(|m| m.to == Endpoint::Agent { id: AgentId::from("agent_w2") }),
        "1 件でも通常どおり配送されること"
    );
    let summary = log.iter().find(|m| m.to == Endpoint::User).unwrap();
    assert!(
        summary.content.contains("## agent_w2（二号）"),
        "1 件でも束ねの形は同じであること: {}",
        summary.content
    );
}

/// 停止中のワーカーが居ても、生きている側の答えは束ねに入ること（道連れなし）。
///
/// 稼働状態は**動的**なので事前検証には含めない。確かめた瞬間と配送の瞬間で
/// 違いうる値を検証に含めると、嘘の保証になる。
#[tokio::test]
async fn a_stopped_worker_does_not_take_down_the_wave() {
    let dir = TempDir::new("plan-stopped");
    let backend = Arc::new(PlanningBackend::new());
    let (orchestrator, lead, workers) = setup_facilitator(
        &dir,
        backend,
        &[("agent_w1", "一号"), ("agent_w2", "二号")],
        OrchestratorConfig::default(),
    )
    .await;
    orchestrator.stop_agent(&workers[1]).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&lead, "手分けして").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(600)).await;

    let summary = messages(&events)
        .into_iter()
        .find(|m| m.from == Endpoint::Agent { id: lead.clone() } && m.to == Endpoint::User)
        .expect("進行役がユーザーへ返すこと");

    assert!(
        summary.content.contains("作業しました"),
        "生きている側の答えは束ねに入ること: {}",
        summary.content
    );
    assert!(
        summary.content.contains("## agent_w2（二号）"),
        "停止側も見出しごと残ること（黙って消さない）: {}",
        summary.content
    );
    assert!(
        summary.content.contains("尋ねられませんでした"),
        "停止側は理由が文字列で入ること: {}",
        summary.content
    );
}

/// hop の上限は**波全体で一様**なので、1 つの結果文字列で返すこと。
///
/// タスク数ぶん同じ文字列を並べない。1 回の plan の中で hop は変わらず、
/// 全タスクが同じ `incoming.hop` を共有する。
#[tokio::test]
async fn an_exhausted_hop_refuses_the_whole_wave_once() {
    let dir = TempDir::new("plan-hop");
    let backend = Arc::new(PlanningBackend::new());
    // ユーザー発の hop は 0。進行役の配送は hop 1 になるので、上限 1 で塞がる。
    let config = OrchestratorConfig {
        max_hops: 1,
        ..OrchestratorConfig::default()
    };
    let (orchestrator, lead, workers) = setup_facilitator(
        &dir,
        backend,
        &[("agent_w1", "一号"), ("agent_w2", "二号")],
        config,
    )
    .await;

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&lead, "頼んで").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(500)).await;
    let log = messages(&events);

    for worker in &workers {
        assert!(
            !log.iter().any(|m| m.to == Endpoint::Agent { id: worker.clone() }),
            "hop 切れなら何も配送しないこと"
        );
    }
    let summary = log.iter().find(|m| m.to == Endpoint::User).unwrap();
    assert_eq!(
        summary.content.matches("転送の上限").count(),
        1,
        "同じ文字列をタスク数ぶん並べないこと: {}",
        summary.content
    );
    // hop 上限も配送ゼロなので、波として記録しない（Spec 08）。
    assert!(
        orchestrator.list_plan_waves().await.is_empty(),
        "hop 切れの plan は波として記録しないこと"
    );
}

/// 相手が答えずに転送したとき、依頼主に**その事実**が返ること。
///
/// 以前は `reply_to` が `Finish` 分岐でしか使われず、`Handoff` では送られない
/// まま drop されていたため、依頼主は「相手から答えが返りませんでした。」を
/// 読んでいた。**これは嘘** — 答えは返っており、宛先が違うだけ。
/// `ask` の既存バグで、`plan` は同じ経路を N 倍踏みやすくする。
#[tokio::test]
async fn a_worker_that_transfers_reports_the_fact_not_silence() {
    let dir = TempDir::new("plan-transfer");
    let orchestrator = setup_with(
        &dir,
        Arc::new(TransferringWorkerBackend),
        OrchestratorConfig::default(),
    )
    .await;

    let (a, b, c) = (
        AgentId::from("agent_a"),
        AgentId::from("agent_b"),
        AgentId::from("agent_c"),
    );
    for (id, name) in [(&a, "依頼主"), (&b, "ワーカー"), (&c, "第三者")] {
        orchestrator
            .create_agent(AgentSpec::new(id.clone(), name, "tpl"))
            .await
            .unwrap();
    }
    orchestrator.set_connections(&a, vec![b.clone()]).await.unwrap();
    orchestrator.set_connections(&b, vec![c.clone()]).await.unwrap();
    for id in [&a, &b, &c] {
        orchestrator.start_agent(id).await.unwrap();
    }

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&a, "ワーカーに聞いて").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(700)).await;

    let reply = messages(&events)
        .into_iter()
        .find(|m| m.from == Endpoint::Agent { id: a.clone() } && m.to == Endpoint::User)
        .expect("依頼主がユーザーへ返すこと");

    assert!(
        reply.content.contains("会話を渡しました"),
        "転送した事実が依頼主へ返ること: {}",
        reply.content
    );
    assert!(
        !reply.content.contains("答えが返りませんでした"),
        "「答えが返らなかった」は嘘なので使わないこと: {}",
        reply.content
    );
}

// ---------------------------------------------------------------------------
// 失敗したターンの記憶（2026-07-31 実機観測）
// ---------------------------------------------------------------------------

/// 1 回目だけ失敗し、以後は受け取ったメッセージを記録して答えるバックエンド。
struct FailFirstThenRecordingBackend {
    calls: std::sync::Mutex<usize>,
    seen: std::sync::Mutex<Vec<Vec<ChatMessage>>>,
}

impl FailFirstThenRecordingBackend {
    fn new() -> Self {
        Self {
            calls: std::sync::Mutex::new(0),
            seen: std::sync::Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl LlmBackend for FailFirstThenRecordingBackend {
    fn name(&self) -> &str {
        "fail-first"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        {
            let mut calls = self.calls.lock().unwrap();
            *calls += 1;
            if *calls == 1 {
                // 出力上限で落ちたときと同じ形（一過性なので稼働は続く）。
                return Err(LlmError::EmptyResponse);
            }
        }
        self.seen.lock().unwrap().push(req.messages.clone());
        Ok(ChatResponse {
            text: Some("承知".into()),
            tool_calls: Vec::new(),
            finish: Finish::Stop,
            usage: Usage { prompt: 1, completion: 1, cache_read: 0 },
            grounding: Default::default(),
        })
    }
}

/// **失敗したターンでも「何を頼まれたか」は履歴に残ること。**
///
/// 履歴の書き込みは handle_message の終盤にあり、途中で落ちると受け取った依頼ごと
/// 消える。一方で広場ログはユーザー発を対象外にし、自分宛も is_mine で除外する
/// （履歴にある前提で組まれている）。両者の前提が噛み合わず、失敗したターンの
/// 依頼が**どのプロンプト経路にも載らない**状態になっていた。
///
/// 実機（2026-07-31）: 出力上限で 1 ターン落ちた直後、進行役が直前の依頼を完全に
/// 失い、他のエージェントへ聞いて回った。会話ログには残っていて画面には見えるので、
/// 利用者からは「なぜ忘れたのか」が分からない。
#[tokio::test]
async fn a_failed_turn_still_remembers_what_was_asked() {
    let dir = TempDir::new("failed-turn-memory");
    let backend = Arc::new(FailFirstThenRecordingBackend::new());
    let orchestrator = setup_with(&dir, backend.clone(), OrchestratorConfig::default()).await;

    let id = AgentId::from("agent_solo");
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "ひとり", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    // 1 ターン目: 失敗する。
    orchestrator
        .send_user_message(&id, "README を英訳して README_en.md に書いて")
        .await
        .unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    // 一過性の失敗では稼働を降ろさない（実機と同じ状態）。
    assert_eq!(
        orchestrator.snapshot(&id).await.unwrap().status,
        AgentStatus::Running,
        "一過性の失敗で止まらないこと"
    );

    // 2 ターン目: ユーザーが「続き」を頼む。
    orchestrator.send_user_message(&id, "先ほどの続きをお願い").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let seen = backend.seen.lock().unwrap();
    let latest = seen.last().expect("2 ターン目のリクエストが記録されること");
    let joined = latest
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        joined.contains("README を英訳して"),
        "失敗したターンの依頼が次のプロンプトに載ること: {joined}"
    );
    // 送り手の封筒は成功時と同じ形で積む（履歴だけ出所不明にしない）。
    assert!(
        joined.contains("【送り手: ユーザー】"),
        "封筒つきで積むこと: {joined}"
    );
    // 応答側は「失敗した」と分かる形で埋める（往復の対を崩さない）。
    assert!(
        joined.contains("このターンは失敗し"),
        "失敗した事実も残すこと: {joined}"
    );
}

// ---------------------------------------------------------------------------
// 波の記録と event（Spec 08 — 波ペイン）
// ---------------------------------------------------------------------------

/// 波が型付きで記録され、event が per planId の順序で流れること。
#[tokio::test]
async fn a_wave_is_recorded_with_typed_states_and_ordered_events() {
    let dir = TempDir::new("plan-record");
    let backend = Arc::new(PlanningBackend::new());
    let (orchestrator, lead, _) = setup_facilitator(
        &dir,
        backend,
        &[("agent_w1", "一号"), ("agent_w2", "二号")],
        OrchestratorConfig::default(),
    )
    .await;

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&lead, "手分けして").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(600)).await;

    let waves = orchestrator.list_plan_waves().await;
    assert_eq!(waves.len(), 1, "1 波だけ記録されること");
    let wave = &waves[0];
    assert_eq!(wave.plan_id, 1, "planId は 1 始まり（0 は予約）");
    assert_eq!(wave.agent_id, lead);
    assert_eq!(wave.tasks.len(), 2);
    assert!(
        wave.tasks.iter().all(|t| t.state == PlanTaskState::Answered),
        "全員が答えた波は全タスク answered であること: {:#?}",
        wave.tasks
    );
    assert!(wave.tasks.iter().all(|t| t.elapsed_ms.is_some()));
    assert!(wave.tasks.iter().all(|t| t.msg_chars > 0));
    assert!(wave.bundle_chars.is_some(), "完了した波は束ねの大きさを持つこと");
    assert!(wave.elapsed_ms.is_some(), "完了した波は所要を持つこと");

    // event の順序保証は per planId: Started → Resolved* → Finished。
    let started = events
        .iter()
        .position(|e| matches!(e, CoreEvent::PlanWaveStarted { .. }))
        .expect("PlanWaveStarted が流れること");
    let finished = events
        .iter()
        .position(|e| matches!(e, CoreEvent::PlanWaveFinished { .. }))
        .expect("PlanWaveFinished が流れること");
    let resolved: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e, CoreEvent::PlanTaskResolved { .. }))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(resolved.len(), 2, "タスクごとに 1 回ずつ解決が流れること");
    assert!(
        resolved.iter().all(|&i| started < i && i < finished),
        "Started → Resolved* → Finished の順であること \
         (started={started} resolved={resolved:?} finished={finished})"
    );
}

/// 停止中のワーカーは `undeliverable` として**型で**残ること。
///
/// 文言 parse ではないことがこのテストの本体 — 文言は束ねの中にしか無く、
/// 記録は分類だけを持つ。
#[tokio::test]
async fn a_stopped_worker_is_recorded_as_undeliverable() {
    let dir = TempDir::new("plan-record-stopped");
    let backend = Arc::new(PlanningBackend::new());
    let (orchestrator, lead, workers) = setup_facilitator(
        &dir,
        backend,
        &[("agent_w1", "一号"), ("agent_w2", "二号")],
        OrchestratorConfig::default(),
    )
    .await;
    orchestrator.stop_agent(&workers[1]).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&lead, "手分けして").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(600)).await;

    let waves = orchestrator.list_plan_waves().await;
    assert_eq!(waves.len(), 1);
    let tasks = &waves[0].tasks;
    // 入力順（= 提示 enum の順）を保つ。
    assert_eq!(tasks[0].to, workers[0]);
    assert_eq!(tasks[0].state, PlanTaskState::Answered);
    assert_eq!(tasks[1].to, workers[1]);
    assert_eq!(
        tasks[1].state,
        PlanTaskState::Undeliverable,
        "停止中は undeliverable と型で残ること"
    );
    assert!(
        tasks.iter().all(|t| t.state != PlanTaskState::Running),
        "完了した波に「実行中」を残さないこと"
    );
}

/// 同一ターン内で 2 波を撒く進行役（波 1 = 全員 → 束ね → 波 2 = 1 体 → 最終出力）。
///
/// Spec 04 の「plan → 結果 → plan」往復の再現。周回はツール結果の件数で見分ける。
struct TwoWaveBackend;

#[async_trait::async_trait]
impl LlmBackend for TwoWaveBackend {
    fn name(&self) -> &str {
        "two-wave"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let usage = Usage {
            prompt: 1,
            completion: 1,
            cache_read: 0,
        };

        if let Some(plan) = req.tools.iter().find(|t| t.name == "plan") {
            let bundles = req.messages.iter().filter(|m| m.role == Role::Tool).count();
            let ids = plan.parameters["properties"]["tasks"]["items"]["properties"]["to"]["enum"]
                .as_array()
                .cloned()
                .unwrap_or_default();

            let tasks: Vec<serde_json::Value> = match bundles {
                // 波 1: 全接続先へ。
                0 => ids
                    .iter()
                    .map(|id| serde_json::json!({ "to": id, "message": "調べて" }))
                    .collect(),
                // 波 2: 1 体にだけ追加調査（前の波の結果を見てから、の形）。
                1 => vec![serde_json::json!({ "to": ids[0], "message": "追加で調べて" })],
                // 波 2 の束ねを受け取ったら最終出力。
                _ => {
                    return Ok(ChatResponse {
                        text: Some("2 波の結果をまとめました".into()),
                        tool_calls: Vec::new(),
                        finish: Finish::Stop,
                        usage,
                        grounding: Default::default(),
                    });
                }
            };
            return Ok(ChatResponse {
                text: Some(String::new()),
                tool_calls: vec![ToolCall {
                    id: format!("call_wave_{}", bundles + 1),
                    name: "plan".into(),
                    args: serde_json::json!({ "tasks": tasks }),
                    extra: None,
                }],
                finish: Finish::ToolUse,
                usage,
                grounding: Default::default(),
            });
        }

        Ok(ChatResponse {
            text: Some("作業しました".into()),
            tool_calls: Vec::new(),
            finish: Finish::Stop,
            usage,
            grounding: Default::default(),
        })
    }
}

/// 同一ターン内の 2 波が**両方**記録されること（実機で第二波が出ない報告の再現）。
#[tokio::test]
async fn consecutive_waves_in_one_turn_are_both_recorded() {
    let dir = TempDir::new("plan-two-waves");
    let (orchestrator, lead, _) = setup_facilitator(
        &dir,
        Arc::new(TwoWaveBackend),
        &[("agent_w1", "一号"), ("agent_w2", "二号")],
        OrchestratorConfig::default(),
    )
    .await;

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&lead, "手分けして、足りなければ追加で").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(800)).await;

    let waves = orchestrator.list_plan_waves().await;
    assert_eq!(waves.len(), 2, "2 波とも記録されること: {waves:#?}");
    assert_eq!((waves[0].plan_id, waves[0].wave), (1, 1));
    assert_eq!((waves[1].plan_id, waves[1].wave), (2, 2));
    assert_eq!(waves[0].tasks.len(), 2);
    assert_eq!(waves[1].tasks.len(), 1, "波 2 は 1 体だけ");
    assert!(
        waves.iter().all(|w| w.bundle_chars.is_some()),
        "両方の波が完了していること"
    );

    let started = events
        .iter()
        .filter(|e| matches!(e, CoreEvent::PlanWaveStarted { .. }))
        .count();
    assert_eq!(started, 2, "PlanWaveStarted が波ごとに流れること");
}

/// plan の片方だけが転送で応じる進行役 + ワーカー構成。
///
/// 進行役だけが `plan` を持つ（接続 2 体以上）。転送ツールを持つワーカーは
/// 答えず渡し、それ以外は素直に答える — 役の判別にツールの顔ぶれを使えるのは、
/// この構成ではワーカーの接続が 1 本以下で plan が提示されないから。
struct HalfTransferringBackend;

#[async_trait::async_trait]
impl LlmBackend for HalfTransferringBackend {
    fn name(&self) -> &str {
        "half-transfer"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let usage = Usage {
            prompt: 1,
            completion: 1,
            cache_read: 0,
        };

        // 進行役: 1 周目は全接続先へ 1 波、2 周目は束ねを最終出力へ。
        if let Some(plan) = req.tools.iter().find(|t| t.name == "plan") {
            if let Some(result) = req.messages.iter().find(|m| m.role == Role::Tool) {
                return Ok(ChatResponse {
                    text: Some(format!("まとめ\n{}", result.content)),
                    tool_calls: Vec::new(),
                    finish: Finish::Stop,
                    usage,
                    grounding: Default::default(),
                });
            }
            let ids = plan.parameters["properties"]["tasks"]["items"]["properties"]["to"]["enum"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let tasks: Vec<serde_json::Value> = ids
                .iter()
                .map(|id| {
                    serde_json::json!({
                        "to": id,
                        "message": format!("{} への依頼", id.as_str().unwrap_or(""))
                    })
                })
                .collect();
            return Ok(ChatResponse {
                text: Some(String::new()),
                tool_calls: vec![ToolCall {
                    id: "call_plan".into(),
                    name: "plan".into(),
                    args: serde_json::json!({ "tasks": tasks }),
                    extra: None,
                }],
                finish: Finish::ToolUse,
                usage,
                grounding: Default::default(),
            });
        }

        // 転送ツールを持つワーカー: 自分で答えず、接続先へ会話を渡す。
        if let Some(transfer) = req.tools.iter().find(|t| t.name.starts_with("transfer_to_")) {
            return Ok(ChatResponse {
                text: Some("私では分かりません".into()),
                tool_calls: vec![ToolCall {
                    id: "call_transfer".into(),
                    name: transfer.name.clone(),
                    args: serde_json::json!({ "message": "代わりに答えて" }),
                    extra: None,
                }],
                finish: Finish::ToolUse,
                usage,
                grounding: Default::default(),
            });
        }

        Ok(ChatResponse {
            text: Some("作業しました".into()),
            tool_calls: Vec::new(),
            finish: Finish::Stop,
            usage,
            grounding: Default::default(),
        })
    }
}

/// 転送で応じたタスクは `handed_off` として**型で**残ること。
///
/// 転送の答えは文字列としては普通の答えと同じ経路（`reply_to`）で返る —
/// 型（`Reply.kind`）で刻まないと区別できない、が Spec 08 P1 の核。
#[tokio::test]
async fn a_transferring_task_is_recorded_as_handed_off() {
    let dir = TempDir::new("plan-record-handoff");
    let orchestrator = setup_with(
        &dir,
        Arc::new(HalfTransferringBackend),
        OrchestratorConfig::default(),
    )
    .await;

    let (lead, w1, w2, w3) = (
        AgentId::from("agent_lead"),
        AgentId::from("agent_w1"),
        AgentId::from("agent_w2"),
        AgentId::from("agent_w3"),
    );
    for (id, name) in [(&lead, "進行役"), (&w1, "一号"), (&w2, "二号"), (&w3, "第三者")] {
        orchestrator
            .create_agent(AgentSpec::new(id.clone(), name, "tpl"))
            .await
            .unwrap();
    }
    orchestrator.set_connections(&lead, vec![w1.clone(), w2.clone()]).await.unwrap();
    orchestrator.set_connections(&w2, vec![w3.clone()]).await.unwrap();
    for id in [&lead, &w1, &w2, &w3] {
        orchestrator.start_agent(id).await.unwrap();
    }

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&lead, "手分けして").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(800)).await;

    let waves = orchestrator.list_plan_waves().await;
    assert_eq!(waves.len(), 1);
    let tasks = &waves[0].tasks;
    let answered = tasks.iter().find(|t| t.to == w1).expect("一号のタスク");
    let transferred = tasks.iter().find(|t| t.to == w2).expect("二号のタスク");
    assert_eq!(answered.state, PlanTaskState::Answered);
    assert_eq!(
        transferred.state,
        PlanTaskState::HandedOff,
        "転送は handed_off と型で残ること（文言 parse ではない）"
    );
    assert!(transferred.elapsed_ms.is_some());
}

// ---------------------------------------------------------------------------
// 顔ぶれと入退室（Spec 06）
// ---------------------------------------------------------------------------

/// 常に致命的エラーを返すバックエンド。Running → Failed の遷移を再現する。
struct FatalBackend;

#[async_trait::async_trait]
impl LlmBackend for FatalBackend {
    fn name(&self) -> &str {
        "fatal"
    }
    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, LlmError> {
        // Config は再試行で回復しない = fatal。エージェントは Failed へ落ちる。
        Err(LlmError::Config("テスト用の致命エラー".into()))
    }
}

/// 稼働中のエージェントを止めると、System 発・User 宛の通知が 1 件だけ記録されること。
///
/// Running → Stopping で 1 件、Stopping → Idle は同じ側なので 0 件。
#[tokio::test]
async fn stopping_an_agent_records_one_presence_notice() {
    let dir = TempDir::new("presence-stop");
    let orchestrator = setup(&dir, OrchestratorConfig::default()).await;
    let id = AgentId::from("agent_b");
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "ブラボー", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.stop_agent(&id).await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(300)).await;

    let notices: Vec<_> = messages(&events)
        .into_iter()
        .filter(|m| m.from == Endpoint::System)
        .collect();
    assert_eq!(notices.len(), 1, "境界をまたぐ遷移 1 回で通知 1 件: {notices:#?}");
    assert_eq!(notices[0].to, Endpoint::User, "宛先は User（配送なし・全員の広場に載る）");
    assert_eq!(notices[0].content, "agent_b（ブラボー）が停止しました");
}

/// 起動は Starting → Running の境界で 1 件だけ通知されること。
#[tokio::test]
async fn starting_an_agent_records_one_presence_notice() {
    let dir = TempDir::new("presence-start");
    let orchestrator = setup(&dir, OrchestratorConfig::default()).await;
    let id = AgentId::from("agent_b");
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "ブラボー", "tpl"))
        .await
        .unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.start_agent(&id).await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(300)).await;

    let notices: Vec<_> = messages(&events)
        .into_iter()
        .filter(|m| m.from == Endpoint::System)
        .collect();
    assert_eq!(notices.len(), 1, "Idle → Starting では出さない: {notices:#?}");
    assert_eq!(notices[0].content, "agent_b（ブラボー）が稼働を開始しました");
}

/// 致命的な失敗で落ちたときは「失敗により停止」と種別が伝わること。
///
/// 理由（last_error の中身）は流さない — 失敗と正常停止の区別だけが
/// 進行役の次の一手を変える。
#[tokio::test]
async fn a_fatal_failure_reports_its_kind_without_the_reason() {
    let dir = TempDir::new("presence-failed");
    let orchestrator = setup_with(
        &dir,
        Arc::new(FatalBackend),
        OrchestratorConfig::default(),
    )
    .await;
    let id = AgentId::from("agent_b");
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "ブラボー", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "何か話して").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let notice = messages(&events)
        .into_iter()
        .find(|m| m.from == Endpoint::System)
        .expect("失敗の通知が出ること");
    assert_eq!(notice.content, "agent_b（ブラボー）が失敗により停止しました");
    assert!(
        !notice.content.contains("致命エラー"),
        "エラーの理由は流さない: {}",
        notice.content
    );
}

/// 通知は `hearsRoomLog: false` の個体にも届き、顔ぶれは接続順で状態を示すこと。
///
/// 広場ログのオプトアウトは固定費の削減で、入退室は配送先の正しさ —
/// コストの設定が経路の正しさを黙って壊してはいけない（Spec 06 rev3 指摘 A）。
#[tokio::test]
async fn presence_reaches_optout_agents_and_roster_lists_by_connection_order() {
    let dir = TempDir::new("presence-prompt");
    let backend = Arc::new(RecordingBackend::default());
    let orchestrator = setup_with(&dir, backend.clone(), OrchestratorConfig::default()).await;

    let (a, b, c) = (
        AgentId::from("agent_a"),
        AgentId::from("agent_b"),
        AgentId::from("agent_c"),
    );
    let mut spec_a = AgentSpec::new(a.clone(), "アルファ", "tpl");
    spec_a.hears_room_log = false;
    orchestrator.create_agent(spec_a).await.unwrap();
    orchestrator
        .create_agent(AgentSpec::new(b.clone(), "ブラボー", "tpl"))
        .await
        .unwrap();
    orchestrator
        .create_agent(AgentSpec::new(c.clone(), "チャーリー", "tpl"))
        .await
        .unwrap();
    orchestrator.set_connections(&a, vec![b.clone(), c.clone()]).await.unwrap();
    for id in [&a, &b, &c] {
        orchestrator.start_agent(id).await.unwrap();
    }
    // チャーリーだけ止める。停止の通知が 1 件ログへ入り、顔ぶれは「停止中」になる。
    orchestrator.stop_agent(&c).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&a, "状況は？").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let seen = backend.seen.lock().unwrap();
    let request = seen.last().expect("アルファのリクエストが記録されること");
    // **System だけを見ない。** 毎ターン変わるもの（入退室・広場ログ・参照資料）は
    // System では積まず最終発話へ畳む — System で積むと adapter が先頭へ畳んで
    // 前方一致を切るため（failures.md #45）。「届くこと」の検査なので全文で見る。
    let joined = request
        .iter()
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n---\n");

    // 顔ぶれ: システムプロンプト内・接続順（b, c）・状態は UI と同じ語彙。
    assert!(
        joined.contains(
            "## 今の顔ぶれ\nagent_b（ブラボー）: 稼働中 / agent_c（チャーリー）: 停止中"
        ),
        "顔ぶれが接続順・状態つきで載ること: {joined}"
    );

    // 入退室: オプトアウトしていても届く。
    assert!(
        joined.contains("## 入退室"),
        "通知は hearsRoomLog と独立に届くこと: {joined}"
    );
    assert!(
        joined.contains("agent_c（チャーリー）が停止しました"),
        "停止の通知が入ること: {joined}"
    );
    assert!(
        joined.contains("「今の顔ぶれ」が正です"),
        "顔ぶれが権威である案内が付くこと（接続ありの個体）: {joined}"
    );

    // 広場ログ本体は従来どおり届かない（オプトアウトの効果は保つ）。
    assert!(
        !joined.contains("## この場で交わされていた会話"),
        "広場ログのオプトアウトは壊さないこと: {joined}"
    );

    // 周知（P2）: 失敗が理由つきで返ることが手順の説明に書かれている。
    assert!(
        joined.contains("理由"),
        "protocol_note に失敗の語彙の周知が入ること: {joined}"
    );
}

// ---- Spec 10: 割り込み停止（Phase 1 — ターン局所の純機構） ----------------

/// テスト用の即答ツール。割り込みテストでターンを回し続けるための燃料。
struct BusyTool;

#[async_trait::async_trait]
impl AgentTool for BusyTool {
    fn name(&self) -> &str {
        "busy_probe"
    }
    fn description(&self) -> String {
        "テスト用の即答ツール".into()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    async fn call(
        &self,
        _ctx: &ToolContext,
        _args: &serde_json::Value,
    ) -> agent_core::CoreResult<String> {
        Ok("ok".into())
    }
}

/// ツールを呼び続け、テストが許可するまで応答を返さないバックエンド。
///
/// 割り込みの述語は「LLM が呼ばれた回数」— ターンの終了を述語にすると、
/// 完走した場合でも通ってしまう（Spec 10 検証の設計 / #45 の一般化 4）。
/// ゲートで 1 呼び出しずつ進めるのは、壁時計に依存せず「割り込みがどの周回の
/// 前に届いたか」を決定的にするため。
struct GatedLoopingBackend {
    /// chat が呼ばれた回数。
    calls: Arc<std::sync::atomic::AtomicU32>,
    /// 各呼び出しの開始通知（テスト側はこれを見てから割り込む）。
    started: tokio::sync::mpsc::UnboundedSender<u32>,
    /// 応答を返してよい数。`add_permits` で進める。
    gate: Arc<tokio::sync::Semaphore>,
    /// 真なら本文だけ返してターンを終える（打ち切り後の「次のターン」用）。
    plain: Arc<std::sync::atomic::AtomicBool>,
    /// 直近の chat が受け取った messages（履歴の検証に使う）。
    last_request: Arc<std::sync::Mutex<Vec<ChatMessage>>>,
}

impl GatedLoopingBackend {
    fn new() -> (
        Arc<Self>,
        tokio::sync::mpsc::UnboundedReceiver<u32>,
        Arc<tokio::sync::Semaphore>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let backend = Arc::new(Self {
            calls: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            started: tx,
            gate: Arc::clone(&gate),
            plain: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            last_request: Arc::new(std::sync::Mutex::new(Vec::new())),
        });
        (backend, rx, gate)
    }
}

#[async_trait::async_trait]
impl LlmBackend for GatedLoopingBackend {
    fn name(&self) -> &str {
        "gated-looping"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        *self.last_request.lock().unwrap() = req.messages.clone();
        let _ = self.started.send(n);
        self.gate.acquire().await.expect("gate は閉じない").forget();

        if self.plain.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(ChatResponse {
                text: Some("済みました".into()),
                tool_calls: Vec::new(),
                finish: Finish::Stop,
                usage: Usage { prompt: 1, completion: 1, cache_read: 0 },
                grounding: Default::default(),
            });
        }

        // 毎回違う引数で呼ぶ。RepeatGuard（#41）に掛けない — ここで測るのは
        // 割り込みで、繰り返し検出が先に止めるとテストの述語が濁る。
        Ok(ChatResponse {
            text: Some(String::new()),
            tool_calls: vec![ToolCall {
                id: format!("call_{n}"),
                name: "busy_probe".into(),
                args: serde_json::json!({ "round": n }),
                extra: None,
            }],
            finish: Finish::ToolUse,
            usage: Usage { prompt: 1, completion: 1, cache_read: 0 },
            grounding: Default::default(),
        })
    }
}

/// 割り込みイベントの数を数える。
fn interruptions(events: &[CoreEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, CoreEvent::TurnInterrupted { .. }))
        .count()
}

/// 打ち切りの System 行を集める。
fn interrupt_notices<'a>(events: &'a [CoreEvent]) -> Vec<&'a agent_core::AgentMessage> {
    messages(events)
        .into_iter()
        .filter(|m| {
            m.from == Endpoint::System
                && m.content.contains("ターンをユーザーの指示で打ち切りました")
        })
        .collect()
}

/// **割り込みは周回境界でターンを切り、エージェントは稼働したまま残る。**
///
/// 述語は LLM の呼び出し回数（1 回で止まること）。出口 2a の 3 点のうち
/// (a) System 行と、打ち切りが失敗扱いにならないこと（AgentFailed ゼロ・
/// Running のまま）も同時に固定する。続けて次の依頼が普通に処理されること、
/// その履歴に「送った形のままの受信側 + 打ち切り注記」が載っていて
/// ツールの断片が残っていないこと（#29 / #45 の不変条件）まで見る。
#[tokio::test]
async fn an_interrupt_cuts_the_turn_at_the_round_boundary() {
    let dir = TempDir::new("interrupt-cut");
    let (backend, mut started, gate) = GatedLoopingBackend::new();
    let orchestrator = setup_with(&dir, backend.clone(), OrchestratorConfig::default()).await;
    orchestrator.register_tool(Arc::new(BusyTool)).await;
    let id = AgentId::from("agent_01");
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "ザリ", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "ずっと調べ続けて").await.unwrap();

    // 1 周目の LLM 呼び出しが始まった = ターンは確実に飛行中。
    started.recv().await.expect("1 周目が始まること");
    orchestrator.interrupt_turn(&id).await;
    // 飛行中の呼び出しは完走させる（rev1 の判断）— ここで初めて応答を許す。
    gate.add_permits(1);

    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    assert_eq!(
        backend.calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "割り込み後は次の周回の LLM を呼ばないこと"
    );
    assert_eq!(interruptions(&events), 1, "TurnInterrupted は 1 本: {events:?}");
    let notices = interrupt_notices(&events);
    assert_eq!(notices.len(), 1, "System 行は 1 本: {notices:#?}");
    assert!(
        notices[0].content.contains("agent_01（ザリ）"),
        "誰のターンかを名指しする: {}",
        notices[0].content
    );
    assert!(
        notices[0].content.contains("秒"),
        "要求から検知までの elapsed を含む（Notes 2 の判断材料）: {}",
        notices[0].content
    );
    assert!(
        !events.iter().any(|e| matches!(e, CoreEvent::AgentFailed { .. })),
        "打ち切りは失敗ではない（不変条件 4）"
    );
    assert_eq!(
        orchestrator.snapshot(&id).await.unwrap().status,
        AgentStatus::Running,
        "稼働は降ろさない"
    );

    // 次の依頼は普通に処理される（割り込みがターン B へ漏れない — 不変条件 6）。
    backend.plain.store(true, std::sync::atomic::Ordering::SeqCst);
    gate.add_permits(10);
    let mut rx2 = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "元気ですか").await.unwrap();
    let events2 = drain_until_quiet(&mut rx2, Duration::from_millis(400)).await;

    assert_eq!(interruptions(&events2), 0, "新しいターンは切られない");
    assert!(
        messages(&events2)
            .iter()
            .any(|m| matches!(m.from, Endpoint::Agent { .. }) && m.content.contains("済みました")),
        "次のターンは完走して答えが返ること: {events2:?}"
    );

    // 打ち切られたターンの履歴（次のターンのリクエストに載っている形で検証）。
    let request = backend.last_request.lock().unwrap().clone();
    assert!(
        request
            .iter()
            .any(|m| m.role == Role::Assistant
                && m.content.contains("このターンはユーザーの指示で打ち切られました")),
        "履歴に打ち切り注記が残ること（失敗の文言ではなく）"
    );
    assert!(
        request
            .iter()
            .any(|m| m.role == Role::User
                && m.content.contains("【送り手: ユーザー】")
                && m.content.contains("ずっと調べ続けて")),
        "受信側は送った形のまま積まれること（#45 — attributed へ縮めない）"
    );
    assert!(
        !request.iter().any(|m| m.role == Role::Tool),
        "打ち切られたターンのツールの断片は履歴に残らないこと（#29）"
    );
}

/// **飛行中のターンが無い割り込みは no-op（出口 2c）。**
///
/// 何も出さず成功し、あとから始まるターンへ漏れない（不変条件 6 の
/// 「エージェントに紐づくフラグを置かない」の外形）。
#[tokio::test]
async fn an_interrupt_while_idle_is_a_noop_and_does_not_leak() {
    let dir = TempDir::new("interrupt-noop");
    let orchestrator = setup(&dir, OrchestratorConfig::default()).await;
    let id = AgentId::from("agent_01");
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "ザリ", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    // 飛行中のターンが無い状態で割り込む。エラーにも通知にもならない。
    orchestrator.interrupt_turn(&id).await;

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "こんにちは").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    assert_eq!(interruptions(&events), 0, "あとから始まるターンへ漏れない");
    assert!(interrupt_notices(&events).is_empty(), "System 行も出ない");
    assert!(
        messages(&events)
            .iter()
            .any(|m| matches!(m.from, Endpoint::Agent { .. })),
        "ターンは普通に完走する: {events:?}"
    );
}

/// **二重割り込みでも出口の行は 1 本（interrupt_all と同じ形の冪等性）。**
///
/// 出口の行は切られたターン自身が検知時に書く — 割り込んだ側が書かないから、
/// 何回要求が重なっても検知は 1 回で行は 1 本になる。
#[tokio::test]
async fn a_double_interrupt_writes_one_notice() {
    let dir = TempDir::new("interrupt-double");
    let (backend, mut started, gate) = GatedLoopingBackend::new();
    let orchestrator = setup_with(&dir, backend.clone(), OrchestratorConfig::default()).await;
    orchestrator.register_tool(Arc::new(BusyTool)).await;
    let id = AgentId::from("agent_01");
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "ザリ", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "調べ続けて").await.unwrap();
    started.recv().await.expect("1 周目が始まること");

    orchestrator.interrupt_turn(&id).await;
    orchestrator.interrupt_turn(&id).await;
    gate.add_permits(1);

    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    assert_eq!(interruptions(&events), 1, "イベントは 1 本: {events:?}");
    assert_eq!(interrupt_notices(&events).len(), 1, "System 行も 1 本");
}

/// 進行役は委譲し、ワーカー側はゲートつきでツールを回し続けるバックエンド。
///
/// 役の判別は提示ツールで行う（`ask_*` を持つ側が進行役）。委譲の結果が
/// 届いたら、その本文を引用して会話を終える。
struct GatedAskBackend {
    calls_worker: Arc<std::sync::atomic::AtomicU32>,
    started: tokio::sync::mpsc::UnboundedSender<u32>,
    gate: Arc<tokio::sync::Semaphore>,
}

#[async_trait::async_trait]
impl LlmBackend for GatedAskBackend {
    fn name(&self) -> &str {
        "gated-ask"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        if let Some(ask) = req.tools.iter().find(|t| t.name.starts_with("ask_")) {
            // 進行役。結果が届いていればそれを引用して終える。
            if let Some(result) = req.messages.iter().rev().find(|m| m.role == Role::Tool) {
                return Ok(ChatResponse {
                    text: Some(format!("結果: {}", result.content)),
                    tool_calls: Vec::new(),
                    finish: Finish::Stop,
                    usage: Usage { prompt: 1, completion: 1, cache_read: 0 },
                    grounding: Default::default(),
                });
            }
            return Ok(ChatResponse {
                text: Some(String::new()),
                tool_calls: vec![ToolCall {
                    id: "ask_1".into(),
                    name: ask.name.clone(),
                    args: serde_json::json!({ "message": "調査して" }),
                    extra: None,
                }],
                finish: Finish::ToolUse,
                usage: Usage { prompt: 1, completion: 1, cache_read: 0 },
                grounding: Default::default(),
            });
        }

        // ワーカー。ゲートが開くまで応答を返さず、開いたらツールを 1 本呼ぶ。
        let n = self
            .calls_worker
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        let _ = self.started.send(n);
        self.gate.acquire().await.expect("gate は閉じない").forget();
        Ok(ChatResponse {
            text: Some(String::new()),
            tool_calls: vec![ToolCall {
                id: format!("w_{n}"),
                name: "busy_probe".into(),
                args: serde_json::json!({ "round": n }),
                extra: None,
            }],
            finish: Finish::ToolUse,
            usage: Usage { prompt: 1, completion: 1, cache_read: 0 },
            grounding: Default::default(),
        })
    }
}

/// **打ち切られたワーカーは、依頼主に「打ち切られた」と伝わる（出口 2a の (c)）。**
///
/// 依頼主が読むのは契約 P3 の固定文。oneshot が黙って drop されると依頼主は
/// 「相手から答えが返りませんでした」（NoAnswer の文言）を読むことになる —
/// それは嘘なので、Reply が届くことを文言で固定する。
#[tokio::test]
async fn an_interrupted_worker_reports_the_cut_to_its_asker() {
    let dir = TempDir::new("interrupt-ask");
    let (tx, mut started) = tokio::sync::mpsc::unbounded_channel();
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let backend = Arc::new(GatedAskBackend {
        calls_worker: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        started: tx,
        gate: Arc::clone(&gate),
    });
    let (orchestrator, lead, workers) = setup_facilitator(
        &dir,
        backend.clone(),
        &[("agent_w1", "ワーカー")],
        OrchestratorConfig::default(),
    )
    .await;
    orchestrator.register_tool(Arc::new(BusyTool)).await;
    let worker = workers[0].clone();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&lead, "調査依頼").await.unwrap();

    // ワーカーのターンが始まってから切る。飛行中の呼び出しは完走させる。
    started.recv().await.expect("ワーカーの 1 周目が始まること");
    orchestrator.interrupt_turn(&worker).await;
    gate.add_permits(1);

    let events = drain_until_quiet(&mut rx, Duration::from_millis(500)).await;

    assert_eq!(
        backend.calls_worker.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "ワーカーは割り込み後に LLM を呼ばない"
    );
    let final_reply = messages(&events)
        .into_iter()
        .find(|m| {
            matches!(&m.from, Endpoint::Agent { id } if *id == lead) && m.to == Endpoint::User
        })
        .expect("進行役は束ねてユーザーへ返す")
        .content
        .clone();
    assert!(
        final_reply.contains("この依頼はユーザーの指示で打ち切られました"),
        "依頼主は打ち切りの事実を固定文で読む（NoAnswer の文言ではなく）: {final_reply}"
    );
}

// ---- Spec 10 Phase 2: 波への伝播 ------------------------------------------

/// 進行役は plan を 1 回だけ撒き、ワーカーはゲートつきで振る舞うバックエンド。
///
/// 役の判別は提示ツール（`plan` を持つ側が進行役）。ワーカーの振る舞いは
/// 依頼文で分岐する — 「直接依頼」を含めば本文だけ返して終わり、それ以外は
/// `busy_probe` を呼び続ける（周回境界に到達させるため）。
struct GatedPlanBackend {
    /// 撒くタスク（テストが宛先を知っているので固定で渡す）。
    tasks: serde_json::Value,
    /// ワーカー側の chat が呼ばれた回数。**畳まれた封筒はここに現れない**のが
    /// 出口 2b の述語（LLM を 1 回も呼ばずに畳む）。
    calls_worker: Arc<std::sync::atomic::AtomicU32>,
    /// ワーカー呼び出しの開始通知。
    started: tokio::sync::mpsc::UnboundedSender<u32>,
    /// ワーカーの応答ゲート。
    gate: Arc<tokio::sync::Semaphore>,
}

#[async_trait::async_trait]
impl LlmBackend for GatedPlanBackend {
    fn name(&self) -> &str {
        "gated-plan"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        if req.tools.iter().any(|t| t.name == "plan") {
            // 進行役。束ねが返っていれば終える（割り込み経路では到達しない）。
            if req.messages.iter().any(|m| m.role == Role::Tool) {
                return Ok(ChatResponse {
                    text: Some("まとめました".into()),
                    tool_calls: Vec::new(),
                    finish: Finish::Stop,
                    usage: Usage { prompt: 1, completion: 1, cache_read: 0 },
                    grounding: Default::default(),
                });
            }
            return Ok(ChatResponse {
                text: Some(String::new()),
                tool_calls: vec![ToolCall {
                    id: "plan_1".into(),
                    name: "plan".into(),
                    args: serde_json::json!({ "tasks": self.tasks }),
                    extra: None,
                }],
                finish: Finish::ToolUse,
                usage: Usage { prompt: 1, completion: 1, cache_read: 0 },
                grounding: Default::default(),
            });
        }

        // ワーカー。開始を通知し、ゲートが開くまで応答を返さない。
        let n = self
            .calls_worker
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        let _ = self.started.send(n);
        self.gate.acquire().await.expect("gate は閉じない").forget();

        let direct = req
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .is_some_and(|m| m.content.contains("直接依頼"));
        if direct {
            return Ok(ChatResponse {
                text: Some("済みました".into()),
                tool_calls: Vec::new(),
                finish: Finish::Stop,
                usage: Usage { prompt: 1, completion: 1, cache_read: 0 },
                grounding: Default::default(),
            });
        }
        Ok(ChatResponse {
            text: Some(String::new()),
            tool_calls: vec![ToolCall {
                id: format!("w_{n}"),
                name: "busy_probe".into(),
                args: serde_json::json!({ "round": n }),
                extra: None,
            }],
            finish: Finish::ToolUse,
            usage: Usage { prompt: 1, completion: 1, cache_read: 0 },
            grounding: Default::default(),
        })
    }
}

fn gated_plan_backend(
    tasks: serde_json::Value,
) -> (
    Arc<GatedPlanBackend>,
    tokio::sync::mpsc::UnboundedReceiver<u32>,
    Arc<tokio::sync::Semaphore>,
) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let backend = Arc::new(GatedPlanBackend {
        tasks,
        calls_worker: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        started: tx,
        gate: Arc::clone(&gate),
    });
    (backend, rx, gate)
}

/// **進行役を切ると、波の待ちが畳まれ、ワーカーの仕事も連鎖して止まる。**
///
/// run_plan の join 待ちは select で抜ける（周回境界だけでは最悪 ask_timeout の
/// 180 秒が割り込み不能のまま残る — U2）。ワーカーは封筒の子トークン経由で
/// 自分の周回境界に止まる。波は interrupted で確定して閉じ、running を残さない。
#[tokio::test]
async fn interrupting_the_leader_folds_the_wave_and_stops_its_workers() {
    let dir = TempDir::new("interrupt-wave");
    let (backend, mut started, gate) = gated_plan_backend(serde_json::json!([
        { "to": "agent_w1", "message": "調査して" },
        { "to": "agent_w2", "message": "調査して" }
    ]));
    let (orchestrator, lead, _workers) = setup_facilitator(
        &dir,
        backend.clone(),
        &[("agent_w1", "ワン"), ("agent_w2", "ツー")],
        OrchestratorConfig::default(),
    )
    .await;
    orchestrator.register_tool(Arc::new(BusyTool)).await;

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&lead, "全員で調査").await.unwrap();

    // 両ワーカーの 1 周目が始まった = 波は配送済みで飛行中。
    started.recv().await.expect("ワーカー 1 体目");
    started.recv().await.expect("ワーカー 2 体目");
    orchestrator.interrupt_turn(&lead).await;
    // 飛行中の呼び出しは完走させる（rev1 の判断）。
    gate.add_permits(2);

    let events = drain_until_quiet(&mut rx, Duration::from_millis(600)).await;

    assert_eq!(
        backend
            .calls_worker
            .load(std::sync::atomic::Ordering::SeqCst),
        2,
        "ワーカーは割り込み後に次の周回の LLM を呼ばない"
    );
    assert_eq!(
        interruptions(&events),
        3,
        "進行役 + 飛行中ワーカー 2 体がそれぞれ 1 回ずつ: {events:?}"
    );
    assert_eq!(interrupt_notices(&events).len(), 3, "System 行も 3 本");

    let waves = orchestrator.list_plan_waves().await;
    assert_eq!(waves.len(), 1, "波は記録されている");
    let wave = &waves[0];
    assert!(
        wave.tasks
            .iter()
            .all(|t| t.state == PlanTaskState::Interrupted),
        "全タスクが interrupted で確定（no_answer ではない）: {:?}",
        wave.tasks
    );
    assert!(
        wave.elapsed_ms.is_some(),
        "波は閉じている（永遠の running を残さない）"
    );
    assert_eq!(wave.bundle_chars, Some(0), "束ねは作られていない");
}

/// **未着手の波タスクは LLM を呼ばずに畳まれ、同じワーカーの別の依頼は生き残る。**
///
/// 伝播の単位は「エージェント」ではなく「ターンの因果」— 進行役を切って
/// 止まってよいのは、その波が生んだ仕事だけ。ワーカーが並行して受けている
/// ユーザー直の依頼は完走する（巻き添え禁止）。畳まれた封筒はワーカーの
/// LLM 呼び出し回数に現れず、TurnInterrupted も出さない（出口 2b）。
#[tokio::test]
async fn a_queued_wave_task_folds_without_starting_and_direct_work_survives() {
    let dir = TempDir::new("interrupt-fold");
    let (backend, mut started, gate) = gated_plan_backend(serde_json::json!([
        { "to": "agent_w1", "message": "調査して" },
        { "to": "agent_w2", "message": "調査して" }
    ]));
    let (orchestrator, lead, workers) = setup_facilitator(
        &dir,
        backend.clone(),
        &[("agent_w1", "ワン"), ("agent_w2", "ツー")],
        OrchestratorConfig::default(),
    )
    .await;
    orchestrator.register_tool(Arc::new(BusyTool)).await;
    let w1 = workers[0].clone();

    let mut rx = orchestrator.subscribe();

    // W1 を先にユーザー直の依頼で塞ぐ。波の W1 宛タスクは受信箱で待つことになる。
    orchestrator.send_user_message(&w1, "直接依頼です").await.unwrap();
    started.recv().await.expect("W1 の直接ターンが始まること");

    orchestrator.send_user_message(&lead, "全員で調査").await.unwrap();
    // W2 は空いているので波のタスクが始まる。W1 宛は queued のまま。
    started.recv().await.expect("W2 の波ターンが始まること");

    orchestrator.interrupt_turn(&lead).await;
    gate.add_permits(4);

    let events = drain_until_quiet(&mut rx, Duration::from_millis(600)).await;

    assert_eq!(
        backend
            .calls_worker
            .load(std::sync::atomic::Ordering::SeqCst),
        2,
        "W1 の直接ターンと W2 の波ターンだけ。畳まれた W1 宛タスクは LLM を呼ばない"
    );
    assert_eq!(
        interruptions(&events),
        2,
        "切られたのは進行役と飛行中の W2 だけ。W1 は切られていない: {events:?}"
    );
    assert!(
        messages(&events).iter().any(|m| {
            matches!(&m.from, Endpoint::Agent { id } if *id == w1)
                && m.to == Endpoint::User
                && m.content.contains("済みました")
        }),
        "W1 のユーザー直の依頼は完走する（巻き添え禁止）: {events:?}"
    );

    let waves = orchestrator.list_plan_waves().await;
    assert_eq!(waves.len(), 1);
    assert!(
        waves[0]
            .tasks
            .iter()
            .all(|t| t.state == PlanTaskState::Interrupted),
        "波の両タスクは interrupted で確定: {:?}",
        waves[0].tasks
    );
}

// ---- Spec 10 Phase 3: interrupt_all と stop_agent の高速化 ------------------

/// **interrupt_all は飛行中の全ターンを切り、飛んでいなければ何もしない（冪等）。**
///
/// P1 の for 文であること自体が仕様 — 独自の機構を持たないので、固定するのは
/// 外形だけ: 飛行中 N 体なら TurnInterrupted が N 本、飛行中 0 なら 0 本。
#[tokio::test]
async fn interrupt_all_cuts_every_flying_turn_and_is_idempotent() {
    let dir = TempDir::new("interrupt-all");
    let (backend, mut started, gate) = GatedLoopingBackend::new();
    let orchestrator = setup_with(&dir, backend.clone(), OrchestratorConfig::default()).await;
    orchestrator.register_tool(Arc::new(BusyTool)).await;
    for (id, name) in [("agent_01", "ワン"), ("agent_02", "ツー")] {
        let aid = AgentId::from(id);
        orchestrator
            .create_agent(AgentSpec::new(aid.clone(), name, "tpl"))
            .await
            .unwrap();
        orchestrator.start_agent(&aid).await.unwrap();
    }

    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&AgentId::from("agent_01"), "調べて")
        .await
        .unwrap();
    orchestrator
        .send_user_message(&AgentId::from("agent_02"), "調べて")
        .await
        .unwrap();
    started.recv().await.expect("1 体目が飛ぶこと");
    started.recv().await.expect("2 体目が飛ぶこと");

    orchestrator.interrupt_all().await;
    gate.add_permits(2);

    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    assert_eq!(
        backend.calls.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "どちらのターンも次の周回の LLM を呼ばない"
    );
    assert_eq!(interruptions(&events), 2, "飛行中 2 体で 2 本: {events:?}");

    // 飛行中が居なくなった状態でもう一度。何も出さず成功する。
    let mut rx2 = orchestrator.subscribe();
    orchestrator.interrupt_all().await;
    let events2 = drain_until_quiet(&mut rx2, Duration::from_millis(200)).await;
    assert_eq!(interruptions(&events2), 0, "冪等: {events2:?}");
    assert!(interrupt_notices(&events2).is_empty(), "System 行も出ない");
}

/// **stop_agent は飛行中ターンへ先に割り込み、完走を待たずに停止する（P5）。**
///
/// 述語は LLM の呼び出し回数 — 高速化が効かなければ、解放したゲートで
/// ターンは max_tool_iterations（既定 12）周まで回ってから止まり、calls が
/// 12 になる。割り込みは Stopping の通知より前に立つ（stop_agent の順序保証）
/// ので、Stopping を見てからゲートを開ければ競合しない。
#[tokio::test]
async fn stop_agent_interrupts_the_flying_turn_instead_of_waiting() {
    let dir = TempDir::new("interrupt-stop-fast");
    let (backend, mut started, gate) = GatedLoopingBackend::new();
    let orchestrator = Arc::new(
        setup_with(&dir, backend.clone(), OrchestratorConfig::default()).await,
    );
    orchestrator.register_tool(Arc::new(BusyTool)).await;
    let id = AgentId::from("agent_01");
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "ザリ", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "調べ続けて").await.unwrap();
    started.recv().await.expect("ターンが飛ぶこと");

    // stop_agent は join でターンの終了を待つのでタスクに逃がす。
    let stopper = {
        let orchestrator = Arc::clone(&orchestrator);
        let id = id.clone();
        tokio::spawn(async move { orchestrator.stop_agent(&id).await })
    };

    // Stopping の通知 = 割り込みは既に立っている（stop_agent の順序保証）。
    loop {
        let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("Stopping が 5 秒以内に流れること")
            .expect("イベントチャネルは生きていること");
        if matches!(
            &event,
            CoreEvent::AgentStatusChanged {
                status: AgentStatus::Stopping,
                ..
            }
        ) {
            break;
        }
    }
    // 潤沢に開ける。高速化が無ければここで 12 周まで回れてしまう。
    gate.add_permits(20);

    stopper
        .await
        .expect("stop タスクが落ちないこと")
        .expect("stop_agent が成功すること");

    assert_eq!(
        backend.calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "飛行中だった 1 周だけで止まる（完走の 12 周を待たない）"
    );
    assert_eq!(
        orchestrator.snapshot(&id).await.unwrap().status,
        AgentStatus::Idle,
        "stop_agent 経由は Running へ戻らず Idle へ（不変条件 4 の但し書き）"
    );
}

// ---- トークン予算（Spec 11） -------------------------------------------------

/// 予算つきの村を組む。`world.json` を bootstrap の**前**に書く —
/// 起動時の天井は world の読み込みで入る（起動後の変更は Spec 13 の
/// `set_token_budget` が担う。settings_contract）。
async fn setup_with_budget(
    dir: &TempDir,
    backend: Arc<dyn LlmBackend>,
    ceiling: u64,
) -> Orchestrator {
    std::fs::write(
        dir.0.join("world.json"),
        format!(r#"{{ "tokenBudget": {ceiling} }}"#),
    )
    .unwrap();
    setup_with(dir, backend, OrchestratorConfig::default()).await
}

/// S1: 予算が尽きたターンは周回境界で打ち切られ、System 行が 1 本出て、
/// 稼働は残る（token_budget の exhaustion）。
///
/// SilentToolBackend の usage は 1 周あたり prompt 1 + completion 1 =
/// 実効 5（1×1000 + 1×4000 milli の切り上げ）。天井 5 なので 1 周目で
/// 使い切り、2 周目の周回境界で止まる。
#[tokio::test]
async fn a_turn_is_cut_at_the_round_boundary_when_the_budget_runs_out() {
    let dir = TempDir::new("budget-cut");
    let orchestrator = setup_with_budget(
        &dir,
        Arc::new(SilentToolBackend {
            calls: std::sync::Mutex::new(0),
        }),
        5,
    )
    .await;
    register_all_tools(&orchestrator, &dir).await;

    let id = AgentId::from("agent_01");
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "ザリ", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "調べて").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(600)).await;

    let system_lines: Vec<_> = messages(&events)
        .into_iter()
        .filter(|m| m.from == Endpoint::System && m.content.contains("予算"))
        .cloned()
        .collect();
    assert_eq!(system_lines.len(), 1, "System 行はちょうど 1 本: {events:?}");
    assert!(
        system_lines[0].content.contains("実効 5 トークン"),
        "天井の値が事実として載る: {}",
        system_lines[0].content
    );
    assert!(
        system_lines[0].content.contains("改めて依頼"),
        "次の道を書く（#44 の規律）: {}",
        system_lines[0].content
    );

    assert_eq!(
        orchestrator.snapshot(&id).await.unwrap().status,
        AgentStatus::Running,
        "予算切れは稼働を降ろさない（閉じるのはターンだけ）"
    );
}

/// 予算切れの後でも、次の依頼は**新しい予算**で普通に走る（因果ごとに独立）。
/// EchoBackend は usage を返さないので、バイト見積もり（usage_fallback）で
/// 数えられる経路の確認を兼ねる。
#[tokio::test]
async fn the_next_request_gets_a_fresh_budget_after_exhaustion() {
    let dir = TempDir::new("budget-fresh");
    // 天井 100,000 は echo の 1 ターン（見積もり数百実効）では尽きない。
    let orchestrator = setup_with_budget(
        &dir,
        Arc::new(agent_core::EchoBackend::new("[echo]")),
        100_000,
    )
    .await;

    let id = AgentId::from("agent_01");
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "ザリ", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "一度目").await.unwrap();
    orchestrator.send_user_message(&id, "二度目").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(600)).await;

    let replies = messages(&events)
        .into_iter()
        .filter(|m| m.from == (Endpoint::Agent { id: id.clone() }))
        .count();
    assert_eq!(replies, 2, "健全な依頼は天井の下で普通に完走する: {events:?}");
}

// ---- 村の設定（Spec 13 P1） --------------------------------------------------

/// Spec 13 S1: 画面から保存した天井は**次の依頼から**効く。
///
/// `set_token_budget` はメモリの `World` を変えてから `world.json` へ書き戻す
/// （settings_contract の即時反映 — `world.json` は所有者ではなく投影）。
/// 再起動もホットリロード用の別機構も要らないことを、保存 → 依頼 → 打ち切りの
/// 連鎖で確かめる。
#[tokio::test]
async fn a_ceiling_saved_via_settings_applies_from_the_next_request() {
    let dir = TempDir::new("budget-settings-apply");
    // 起動時は余裕のある天井。SilentToolBackend の 1 周 = 実効 5
    // （prompt 1 ×1 + completion 1 ×4）なので 100,000 では尽きない。
    let orchestrator = setup_with_budget(
        &dir,
        Arc::new(SilentToolBackend {
            calls: std::sync::Mutex::new(0),
        }),
        100_000,
    )
    .await;
    register_all_tools(&orchestrator, &dir).await;

    let id = AgentId::from("agent_01");
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "ザリ", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    // 保存 = メモリと world.json の両方が変わる。
    orchestrator.set_token_budget(Some(5)).await.unwrap();
    assert_eq!(orchestrator.token_budget().await, Some(5));
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.0.join("world.json")).unwrap()).unwrap();
    assert_eq!(
        json["tokenBudget"].as_u64(),
        Some(5),
        "world.json へ書き戻される（投影）"
    );

    // 次の依頼は新しい天井 5 の下で走り、2 周目の周回境界で打ち切られる。
    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "調べて").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(600)).await;
    let cut = messages(&events)
        .into_iter()
        .any(|m| m.from == Endpoint::System && m.content.contains("実効 5 トークン"));
    assert!(
        cut,
        "保存した天井が再起動なしで次の依頼の打ち切りに使われる: {events:?}"
    );
}

/// Spec 13: 天井を「なし」へ戻すと、次の依頼は打ち切られずに完走する。
/// `None` の投影は `world.json` から `tokenBudget` キーごと消えること。
#[tokio::test]
async fn clearing_the_ceiling_removes_the_cut_and_the_projection() {
    let dir = TempDir::new("budget-settings-clear");
    // 天井 5 のままなら 2 周目で打ち切られる編成（上のテストと同じ）。
    let orchestrator = setup_with_budget(
        &dir,
        Arc::new(SilentToolBackend {
            calls: std::sync::Mutex::new(0),
        }),
        5,
    )
    .await;
    register_all_tools(&orchestrator, &dir).await;

    let id = AgentId::from("agent_01");
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "ザリ", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    orchestrator.set_token_budget(None).await.unwrap();
    assert_eq!(orchestrator.token_budget().await, None);
    let kept = std::fs::read_to_string(dir.0.join("world.json")).unwrap();
    assert!(
        !kept.contains("tokenBudget"),
        "None はキーごと消える（0 のマジック値を作らない）: {kept}"
    );

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "調べて").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(600)).await;
    let cut = messages(&events)
        .into_iter()
        .any(|m| m.from == Endpoint::System && m.content.contains("予算"));
    assert!(!cut, "天井なしへ戻した依頼は打ち切られない: {events:?}");
}

/// システムプロンプト（`Role::System` の本文）を呼び出しごとに記録する
/// バックエンド（言語切り替えの不変検証用）。
struct PromptProbeBackend {
    /// 呼び出しごとの、system ブロックを連結した文字列。
    systems: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl LlmBackend for PromptProbeBackend {
    fn name(&self) -> &str {
        "prompt-probe"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let system: String = req
            .messages
            .iter()
            .filter(|m| m.role == Role::System)
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n---\n");
        self.systems.lock().unwrap().push(system);

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

/// Spec 13: 起動時に言語が OS から確定され、`world.json` へ保存される
/// （settings_contract — 「自動」の選択肢は無く、初回に確定する）。
/// テスト機の OS 言語には依存しない — 確定先が ja / en のどちらかであることと、
/// **再起動で再判定されない**（確定済みの値が残る）ことを見る。
#[tokio::test]
async fn the_language_is_determined_once_and_persisted() {
    let dir = TempDir::new("language-determine");
    let _ = setup(&dir, OrchestratorConfig::default()).await;

    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.0.join("world.json")).unwrap()).unwrap();
    let determined = json["language"].as_str().expect("起動で確定される").to_string();
    assert!(
        determined == "ja" || determined == "en",
        "確定先は 2 択のどちらか: {determined}"
    );

    // 確定済みの村を反対の言語へ書き換えてから再起動 — OS で上書きされないこと。
    let flipped = if determined == "ja" { "en" } else { "ja" };
    let mut edited: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.0.join("world.json")).unwrap()).unwrap();
    edited["language"] = serde_json::Value::String(flipped.into());
    std::fs::write(dir.0.join("world.json"), edited.to_string()).unwrap();

    let _ = setup(&dir, OrchestratorConfig::default()).await;
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.0.join("world.json")).unwrap()).unwrap();
    assert_eq!(
        json["language"].as_str(),
        Some(flipped),
        "確定済みの言語は起動で再判定されない（利用者の選択が勝つ）"
    );
}

/// Spec 13: 言語の切り替えはシステムプロンプトに触らない（多言語化 3 層の (3) —
/// 「system_digest が切り替えの前後で同一」の結合版。訳すのは UI 文言だけで、
/// モデルへの入力は 1 字も変わらない）。
#[tokio::test]
async fn switching_the_language_leaves_the_system_prompt_untouched() {
    let dir = TempDir::new("language-prompt");
    let backend = Arc::new(PromptProbeBackend {
        systems: std::sync::Mutex::new(Vec::new()),
    });
    let orchestrator = setup_with(&dir, backend.clone(), OrchestratorConfig::default()).await;

    let id = AgentId::from("agent_01");
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "ザリ", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "一度目").await.unwrap();
    let _ = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    orchestrator
        .set_language(agent_core::world::Language::En)
        .await
        .unwrap();

    orchestrator.send_user_message(&id, "二度目").await.unwrap();
    let _ = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let systems = backend.systems.lock().unwrap();
    assert_eq!(systems.len(), 2, "2 ターン分の記録があること");
    assert_eq!(
        systems[0], systems[1],
        "言語切り替えの前後で system ブロックは 1 字も変わらない"
    );

    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.0.join("world.json")).unwrap()).unwrap();
    assert_eq!(json["language"].as_str(), Some("en"), "投影は切り替わる");
}

/// Spec 13: 0 は設定経路で拒否される（INVALID_TOKEN_BUDGET）。
///
/// 読み込み時の `Some(0) → None` 正規化に任せて受け付けると
/// 「保存したのに黙って別の値になる」— メモリもファイルも変えずに弾く。
#[tokio::test]
async fn setting_a_zero_ceiling_is_rejected_without_touching_anything() {
    let dir = TempDir::new("budget-settings-zero");
    let orchestrator = setup_with_budget(
        &dir,
        Arc::new(agent_core::EchoBackend::new("[echo]")),
        7,
    )
    .await;

    let err = orchestrator.set_token_budget(Some(0)).await.unwrap_err();
    assert_eq!(err.code(), "INVALID_TOKEN_BUDGET");
    assert_eq!(
        orchestrator.token_budget().await,
        Some(7),
        "拒否はメモリを変えない"
    );
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.0.join("world.json")).unwrap()).unwrap();
    assert_eq!(
        json["tokenBudget"].as_u64(),
        Some(7),
        "拒否はファイルも変えない"
    );
}

/// S3: 波の配送前に予算が尽きていたら、配送そのものを始めない。
/// セルは budget_exhausted で確定し、System 行は因果全体で 1 本だけ
/// （CAS の初回観測が記録を 1 系統に保つ）。
///
/// PlanningBackend の進行役 1 周目（実効 5）で天井 5 を使い切るので、
/// 波の 2 タスクは deliver_and_wait の事前検査で止まる。
#[tokio::test]
async fn an_exhausted_budget_stops_wave_deliveries_before_they_start() {
    let dir = TempDir::new("budget-wave");
    std::fs::write(dir.0.join("world.json"), r#"{ "tokenBudget": 5 }"#).unwrap();
    let (orchestrator, lead, _workers) = setup_facilitator(
        &dir,
        Arc::new(PlanningBackend::new()),
        &[("agent_w1", "1 号"), ("agent_w2", "2 号")],
        OrchestratorConfig::default(),
    )
    .await;

    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&lead, "調べてまとめて")
        .await
        .unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(600)).await;

    let resolved: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            CoreEvent::PlanTaskResolved { state, .. } => Some(*state),
            _ => None,
        })
        .collect();
    assert_eq!(resolved.len(), 2, "2 タスクとも確定する: {events:?}");
    assert!(
        resolved.iter().all(|s| *s == PlanTaskState::BudgetExhausted),
        "配送前の予算切れは budget_exhausted で確定: {resolved:?}"
    );

    let system_lines = messages(&events)
        .into_iter()
        .filter(|m| m.from == Endpoint::System && m.content.contains("予算"))
        .count();
    assert_eq!(system_lines, 1, "System 行は因果全体で 1 本だけ");
}

/// 転送は予算を**同一の Arc のまま**引き継ぐ。引き継がれなければ転送先は
/// 新品の予算（または天井なし）で普通に応答してしまう — delegation-fanout
/// race（token_budget の pool、arXiv:2606.04056 の 63 件中 11 件）を
/// 再現させない側の固定。
#[tokio::test]
async fn a_handoff_inherits_the_same_budget_pool() {
    let dir = TempDir::new("budget-handoff");
    std::fs::write(dir.0.join("world.json"), r#"{ "tokenBudget": 5 }"#).unwrap();
    let orchestrator = setup_with(
        &dir,
        Arc::new(AlwaysHandoffBackend),
        OrchestratorConfig::default(),
    )
    .await;

    let a = AgentId::from("agent_01");
    let b = AgentId::from("agent_02");
    orchestrator
        .create_agent(AgentSpec::new(a.clone(), "取次", "tpl"))
        .await
        .unwrap();
    orchestrator
        .create_agent(AgentSpec::new(b.clone(), "受け手", "tpl"))
        .await
        .unwrap();
    orchestrator
        .set_connections(&a, vec![b.clone()])
        .await
        .unwrap();
    orchestrator.start_agent(&a).await.unwrap();
    orchestrator.start_agent(&b).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&a, "よろしく").await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(600)).await;

    // 転送先 B のターンは、引き継いだ予算の残 0 を周回境界で観測して止まる。
    let system_lines: Vec<_> = messages(&events)
        .into_iter()
        .filter(|m| m.from == Endpoint::System && m.content.contains("予算"))
        .cloned()
        .collect();
    assert_eq!(system_lines.len(), 1, "System 行が 1 本出る: {events:?}");
    assert!(
        system_lines[0].content.contains("agent_02"),
        "止まったのは転送**先**のターン（= 予算が引き継がれた証拠）: {}",
        system_lines[0].content
    );
    let b_replies = messages(&events)
        .into_iter()
        .filter(|m| m.from == (Endpoint::Agent { id: b.clone() }))
        .count();
    assert_eq!(
        b_replies, 0,
        "転送先は LLM を呼ばずに止まる（新品の予算を作らない）"
    );
}

/// ceiling 契約の後方互換: 既定 1,000,000 を書くのは**新規 world.json だけ**。
/// 既存の村（tokenBudget なし）は None のまま触らない。
#[tokio::test]
async fn the_default_ceiling_is_written_only_to_a_fresh_world_file() {
    // 新規の村: bootstrap が world.json を作り、既定値が入る。
    let fresh = TempDir::new("budget-fresh-world");
    let _ = setup(&fresh, OrchestratorConfig::default()).await;
    let written = std::fs::read_to_string(fresh.0.join("world.json")).unwrap();
    let json: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert_eq!(
        json["tokenBudget"].as_u64(),
        Some(1_000_000),
        "新規の村には既定の天井が入る: {written}"
    );

    // 既存の村（天井なし）: 黙って書き足さない。
    let legacy = TempDir::new("budget-legacy-world");
    std::fs::write(legacy.0.join("world.json"), r#"{ "agents": [] }"#).unwrap();
    let _ = setup(&legacy, OrchestratorConfig::default()).await;
    let kept = std::fs::read_to_string(legacy.0.join("world.json")).unwrap();
    assert!(
        !kept.contains("tokenBudget"),
        "既存の村へ天井を黙って足さない（後方互換）: {kept}"
    );
}

// ---- 漏れたツール呼び出しの計器（L0） ---------------------------------------

/// ツール呼び出しを**本文テキストとして**返すバックエンド。
///
/// 本文は 2026-08-02 の実機ログから採った実物 — 先頭の `<invoke name="` が
/// 無い形（削ったのは API 側）。合成した理想形で試すと、実機で発火しない
/// 検出器を「動く」と誤認する（failures.md #47 の一般化 3）。
struct LeakedToolCallBackend;

const LEAKED_TEXT: &str = "MCP_DOCKER__fetch\">\n\
     <parameter name=\"url\">https://news.yahoo.co.jp/topics/top-picks</parameter>\n\
     <parameter name=\"max_length\">4000</parameter>\n\
     </invoke>";

#[async_trait::async_trait]
impl LlmBackend for LeakedToolCallBackend {
    fn name(&self) -> &str {
        "leaked-tool-call"
    }

    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, LlmError> {
        Ok(ChatResponse {
            text: Some(LEAKED_TEXT.to_owned()),
            // ネイティブの呼び出しは無い — ハーネスから見れば「最終出力」。
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

/// 計器が**実機の経路で**発火すること（failures.md #47 の L0）。
///
/// 単体テストは文字列判定しか見ない。この企画は「単体は緑なのに実機で
/// 一度も発火しない計器」を既に踏んでいる（RepeatGuard 初版、#41 の一般化 4）。
/// ここでは実際にターンを走らせ、ログファイルへ 1 行出ることまで確かめる。
///
/// 併せて **L0 が挙動を変えないこと**も固定する（漏れた本文はそのまま
/// 答えとして配信される）。ここを変えるのは L1（自動修復）の仕事で、
/// そのとき壊れるべきテストとして残す。
#[tokio::test]
async fn the_leak_instrument_fires_on_a_real_turn() {
    // ログはテスト用の一時ディレクトリの外へ置く。SINK はプロセスで 1 つ
    // （OnceLock）なので、掴んだままのファイルを消しに行かせない。
    let log_path = std::env::temp_dir().join(format!(
        "concordia-leak-probe-{}.log",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    agent_core::open_log(&log_path).expect("ログを開けること");

    let dir = TempDir::new("leak-instrument");
    let orchestrator = setup_with(
        &dir,
        Arc::new(LeakedToolCallBackend),
        OrchestratorConfig::default(),
    )
    .await;

    let id = AgentId::from("agent_01");
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "ザリ", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&id, "ニュースを取ってきて")
        .await
        .unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let written = std::fs::read_to_string(&log_path).expect("ログを読めること");
    assert!(
        written.contains("text tool call leaked"),
        "漏れた本文で計器が発火すること: {written}"
    );

    // 挙動は変えない（L0 は計器）。漏れた本文はそのまま答えになる。
    let reply = messages(&events)
        .into_iter()
        .find(|m| m.from == (Endpoint::Agent { id: id.clone() }))
        .expect("答えが 1 通あること")
        .clone();
    assert_eq!(reply.content, LEAKED_TEXT, "L0 は本文へ手を入れない");
}

// ---- 役職（Spec 14 P2） -----------------------------------------------------

/// 役職を 1 件作る補助。
fn a_role(id: &str, name: &str, defaults: agent_core::AgentRoleDefaults) -> agent_core::AgentRole {
    agent_core::AgentRole {
        id: id.into(),
        name: name.into(),
        description: "テスト用".into(),
        // 色はプロンプトに載らないので、コア側のテストでは常に None で足りる。
        color: None,
        defaults,
    }
}

/// S1: 役職を選んで作ると**設定が入った状態で始まる**。
///
/// 触るのは 4 欄と `Construct.md` だけで、**入れない 5 欄は既定のまま**。
#[tokio::test]
async fn creating_with_a_role_fills_the_settings_and_writes_construct() {
    let dir = TempDir::new("role-create");
    let orchestrator = setup(&dir, OrchestratorConfig::default()).await;

    orchestrator
        .upsert_template(ModelTemplate::new("fast", "速い", "mock-fast"))
        .await
        .unwrap();
    orchestrator
        .upsert_role(a_role(
            "researcher",
            "調査役",
            agent_core::AgentRoleDefaults {
                construct: "あなたは調査役です。".into(),
                model_template_id: Some("fast".into()),
                rag_sources: vec!["docs".into()],
                enabled_tools: Some(vec!["grep".into()]),
                max_tool_iterations: Some(24),
            },
        ))
        .await
        .unwrap();

    let mut spec = AgentSpec::new("agent_1", "ザリ", "tpl");
    spec.role_id = Some("researcher".into());
    let created = orchestrator.create_agent(spec).await.unwrap();

    // 入る 4 欄。
    assert_eq!(created.model_template_id, "fast".into());
    assert_eq!(created.rag_sources, vec!["docs".to_string()]);
    assert_eq!(created.enabled_tools, Some(vec!["grep".to_string()]));
    assert_eq!(created.max_tool_iterations, Some(24));
    // ラベルは残る（バッジの足場）。
    assert_eq!(created.role_id, Some("researcher".into()));

    // 入れない 5 欄は既定のまま。**線と場所が雛形から入らないこと**が主眼。
    let baseline = AgentSpec::new("agent_1", "ザリ", "tpl");
    assert_eq!(created.connected_agents, baseline.connected_agents);
    assert_eq!(created.work_dir, baseline.work_dir);
    assert_eq!(created.order, baseline.order);
    assert_eq!(created.batch_start, baseline.batch_start);
    assert_eq!(created.hears_room_log, baseline.hears_room_log);

    // Construct.md は AgentSpec の欄ではないので、ここでしか書けない。
    let construct = orchestrator
        .read_config(&"agent_1".into(), ConfigFileKind::Construct)
        .await
        .unwrap();
    assert_eq!(construct, "あなたは調査役です。");
}

/// 凍結 4: **コピーの発火点は新規作成ただ 1 つ。**
///
/// 役職の中身を後から直しても、既に居る個体は 1 欄も変わらない。
#[tokio::test]
async fn editing_a_role_never_touches_existing_agents() {
    let dir = TempDir::new("role-copy");
    let orchestrator = setup(&dir, OrchestratorConfig::default()).await;
    orchestrator
        .upsert_template(ModelTemplate::new("fast", "速い", "mock-fast"))
        .await
        .unwrap();
    orchestrator
        .upsert_role(a_role(
            "researcher",
            "調査役",
            agent_core::AgentRoleDefaults {
                construct: "初版".into(),
                max_tool_iterations: Some(24),
                ..Default::default()
            },
        ))
        .await
        .unwrap();

    let mut spec = AgentSpec::new("agent_1", "ザリ", "tpl");
    spec.role_id = Some("researcher".into());
    orchestrator.create_agent(spec).await.unwrap();

    // 役職の中身を丸ごと差し替える。
    orchestrator
        .upsert_role(a_role(
            "researcher",
            "調査役",
            agent_core::AgentRoleDefaults {
                construct: "二版".into(),
                model_template_id: Some("fast".into()),
                max_tool_iterations: Some(99),
                ..Default::default()
            },
        ))
        .await
        .unwrap();

    let after = orchestrator.snapshot(&"agent_1".into()).await.unwrap();
    assert_eq!(after.max_tool_iterations, Some(24), "既存の個体は変わらない");
    assert_eq!(after.model_template_id, "tpl".into());
    let construct = orchestrator
        .read_config(&"agent_1".into(), ConfigFileKind::Construct)
        .await
        .unwrap();
    assert_eq!(construct, "初版", "Construct.md も書き換わらない");
}

/// 凍結 5: **役職を削除しても個体の動作は変わらない。**
///
/// `remove_template` は参照中を拒むが、役職は拒まない。コピー済みだから。
#[tokio::test]
async fn deleting_a_role_in_use_leaves_the_agent_working() {
    let dir = TempDir::new("role-delete");
    let orchestrator = setup(&dir, OrchestratorConfig::default()).await;
    orchestrator
        .upsert_role(a_role(
            "researcher",
            "調査役",
            agent_core::AgentRoleDefaults {
                construct: "本文".into(),
                max_tool_iterations: Some(24),
                ..Default::default()
            },
        ))
        .await
        .unwrap();
    let mut spec = AgentSpec::new("agent_1", "ザリ", "tpl");
    spec.role_id = Some("researcher".into());
    orchestrator.create_agent(spec).await.unwrap();

    // 参照中でも通る（テンプレートならここで InvalidTopology になる）。
    orchestrator.remove_role(&"researcher".into()).await.unwrap();

    let after = orchestrator.snapshot(&"agent_1".into()).await.unwrap();
    assert_eq!(after.max_tool_iterations, Some(24), "設定は無傷");
    assert_eq!(after.role_id, Some("researcher".into()), "id は残る");
    assert!(orchestrator.list_roles().await.is_empty());
    let construct = orchestrator
        .read_config(&"agent_1".into(), ConfigFileKind::Construct)
        .await
        .unwrap();
    assert_eq!(construct, "本文", "Construct.md も残る");
}

/// 参照先の欠落は**その欄だけ落として作成は通す**。
///
/// `rag_sources` には検査を掛けない（索引は実行時に育つ器で、宣言された
/// 登録簿が無い）。ここを検査すると「作ってから資料を食わせる」が壊れる。
#[tokio::test]
async fn a_missing_template_drops_one_field_but_the_agent_is_still_created() {
    let dir = TempDir::new("role-dangling");
    let orchestrator = setup(&dir, OrchestratorConfig::default()).await;
    orchestrator
        .upsert_role(a_role(
            "researcher",
            "調査役",
            agent_core::AgentRoleDefaults {
                model_template_id: Some("存在しない".into()),
                rag_sources: vec!["まだ索引していない資料".into()],
                max_tool_iterations: Some(24),
                ..Default::default()
            },
        ))
        .await
        .unwrap();

    let mut spec = AgentSpec::new("agent_1", "ザリ", "tpl");
    spec.role_id = Some("researcher".into());
    let created = orchestrator.create_agent(spec).await.unwrap();

    assert_eq!(
        created.model_template_id,
        "tpl".into(),
        "落ちるのはこの欄だけ"
    );
    assert_eq!(
        created.rag_sources,
        vec!["まだ索引していない資料".to_string()],
        "索引が空でも rag_sources は落とさない"
    );
    assert_eq!(created.max_tool_iterations, Some(24));
}

/// 存在しない役職を指していても**作成そのものは通る**。
///
/// 村を配った先で役職が欠けているだけで新規作成ができなくなるのは罰が重い。
#[tokio::test]
async fn creating_with_an_unknown_role_still_works() {
    let dir = TempDir::new("role-unknown");
    let orchestrator = setup(&dir, OrchestratorConfig::default()).await;

    let mut spec = AgentSpec::new("agent_1", "ザリ", "tpl");
    spec.role_id = Some("居ない役職".into());
    let created = orchestrator.create_agent(spec).await.unwrap();

    assert_eq!(created.role_id, Some("居ない役職".into()));
    assert_eq!(created.model_template_id, "tpl".into());
}

/// 役職なしの作成経路は**今までどおり**（`Option` なので後方互換）。
#[tokio::test]
async fn creating_without_a_role_is_unchanged() {
    let dir = TempDir::new("role-none");
    let orchestrator = setup(&dir, OrchestratorConfig::default()).await;

    let created = orchestrator
        .create_agent(AgentSpec::new("agent_1", "ザリ", "tpl"))
        .await
        .unwrap();

    assert_eq!(created.role_id, None);
    let construct = orchestrator
        .read_config(&"agent_1".into(), ConfigFileKind::Construct)
        .await
        .unwrap();
    assert!(construct.is_empty(), "役職が無ければ Construct.md は書かない");
}

/// 役職は `world.json` を往復する（村の共有物）。
#[tokio::test]
async fn roles_survive_a_restart() {
    let dir = TempDir::new("role-persist");
    {
        let orchestrator = setup(&dir, OrchestratorConfig::default()).await;
        orchestrator
            .upsert_role(a_role(
                "researcher",
                "調査役",
                agent_core::AgentRoleDefaults::default(),
            ))
            .await
            .unwrap();
    }

    let reopened = setup(&dir, OrchestratorConfig::default()).await;
    let roles = reopened.list_roles().await;
    assert_eq!(roles.len(), 1);
    assert_eq!(roles[0].name, "調査役");
}

// ---- 役職と顔ぶれ（Spec 14 P4） ----------------------------------------------

/// 顔ぶれの行に**役職名だけ**が載る（`role_contract` 凍結 6）。
///
/// 説明は載らない — 顔ぶれは毎ターン・全員ぶんを素の値段で払うので、
/// 名前 3〜5 トークンに対し説明 50〜200 は以後の全ターンに乗る固定費になる。
#[tokio::test]
async fn the_roster_carries_the_role_name_but_never_the_description() {
    let dir = TempDir::new("roster-role");
    let backend = Arc::new(PromptProbeBackend {
        systems: std::sync::Mutex::new(Vec::new()),
    });
    let orchestrator = setup_with(&dir, backend.clone(), OrchestratorConfig::default()).await;

    orchestrator
        .upsert_role(agent_core::AgentRole {
            id: "researcher".into(),
            name: "調査役".into(),
            description: "この文字列はプロンプトに出てはいけない".into(),
            color: None,
            defaults: agent_core::AgentRoleDefaults::default(),
        })
        .await
        .unwrap();

    let planner = AgentId::from("agent_01");
    let worker = AgentId::from("agent_02");
    orchestrator
        .create_agent(AgentSpec::new(planner.clone(), "ザリ", "tpl"))
        .await
        .unwrap();
    let mut worker_spec = AgentSpec::new(worker.clone(), "ジェミー", "tpl");
    worker_spec.role_id = Some("researcher".into());
    orchestrator.create_agent(worker_spec).await.unwrap();
    orchestrator
        .set_connections(&planner, vec![worker.clone()])
        .await
        .unwrap();
    orchestrator.start_agent(&planner).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&planner, "点呼").await.unwrap();
    let _ = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let systems = backend.systems.lock().unwrap();
    let system = systems.first().expect("1 ターン分の記録があること");
    assert!(system.contains("［調査役］"), "顔ぶれに役職名が載る: {system}");
    assert!(
        !system.contains("この文字列はプロンプトに出てはいけない"),
        "説明はプロンプトに載らない（凍結 6）"
    );
}

/// **役職が引けない個体は `［...］` ごと省く**（`role_contract` 凍結 5）。
///
/// `［不明］` とは書かない — 存在しない役は判断材料にならず、顔ぶれでは
/// 毎ターンぶんのトークンを払うだけになる。バッジ側と同じ規則。
#[tokio::test]
async fn an_unresolvable_role_is_omitted_from_the_roster_entirely() {
    let dir = TempDir::new("roster-role-gone");
    let backend = Arc::new(PromptProbeBackend {
        systems: std::sync::Mutex::new(Vec::new()),
    });
    let orchestrator = setup_with(&dir, backend.clone(), OrchestratorConfig::default()).await;

    let planner = AgentId::from("agent_01");
    let worker = AgentId::from("agent_02");
    orchestrator
        .create_agent(AgentSpec::new(planner.clone(), "ザリ", "tpl"))
        .await
        .unwrap();
    let mut worker_spec = AgentSpec::new(worker.clone(), "ジェミー", "tpl");
    worker_spec.role_id = Some("居ない役職".into());
    orchestrator.create_agent(worker_spec).await.unwrap();
    orchestrator
        .set_connections(&planner, vec![worker.clone()])
        .await
        .unwrap();
    orchestrator.start_agent(&planner).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&planner, "点呼").await.unwrap();
    let _ = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let systems = backend.systems.lock().unwrap();
    let system = systems.first().expect("1 ターン分の記録があること");
    assert!(!system.contains("不明］"), "［不明］を作らない: {system}");
    assert!(!system.contains("［］"), "空の括弧も残さない: {system}");
    assert!(
        system.contains("agent_02（ジェミー）: 停止中"),
        "役職の表示だけが落ちて、行そのものは今までどおり: {system}"
    );
}

/// **役職名を足しても `stable_len` は動かない**（`role_contract` 凍結 6）。
///
/// 顔ぶれは `stable_len` の**後ろ**（可変部）にあるので、キャッシュの安定境界は
/// 動かない。ここが破れると、役職を 1 つ足しただけで全エージェントの
/// プロンプトキャッシュが割れる。
#[tokio::test]
async fn adding_a_role_does_not_move_the_stable_prefix() {
    let dir = TempDir::new("roster-stable");
    let backend = Arc::new(PromptProbeBackend {
        systems: std::sync::Mutex::new(Vec::new()),
    });
    let orchestrator = setup_with(&dir, backend.clone(), OrchestratorConfig::default()).await;

    let planner = AgentId::from("agent_01");
    let worker = AgentId::from("agent_02");
    orchestrator
        .create_agent(AgentSpec::new(planner.clone(), "ザリ", "tpl"))
        .await
        .unwrap();
    orchestrator
        .create_agent(AgentSpec::new(worker.clone(), "ジェミー", "tpl"))
        .await
        .unwrap();
    orchestrator
        .set_connections(&planner, vec![worker.clone()])
        .await
        .unwrap();
    orchestrator.start_agent(&planner).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&planner, "一度目").await.unwrap();
    let _ = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    // 役職を作り、相手に付ける（**設定は流し込まれない** — update_agent は
    // ラベルだけを差し替える。凍結 4）。
    orchestrator
        .upsert_role(agent_core::AgentRole {
            id: "researcher".into(),
            name: "調査役".into(),
            description: String::new(),
            color: None,
            defaults: agent_core::AgentRoleDefaults {
                construct: "この本文は既存の個体へ入ってはいけない".into(),
                max_tool_iterations: Some(99),
                ..Default::default()
            },
        })
        .await
        .unwrap();
    let mut worker_spec = AgentSpec::new(worker.clone(), "ジェミー", "tpl");
    worker_spec.role_id = Some("researcher".into());
    orchestrator.update_agent(worker_spec).await.unwrap();

    orchestrator.send_user_message(&planner, "二度目").await.unwrap();
    let _ = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    // 既存の個体には流し込まれていない（凍結 4 の結合版）。
    let after = orchestrator.snapshot(&worker).await.unwrap();
    assert_eq!(after.max_tool_iterations, None, "既存の個体は設定が変わらない");
    let construct = orchestrator
        .read_config(&worker, ConfigFileKind::Construct)
        .await
        .unwrap();
    assert!(construct.is_empty(), "Construct.md も書かれない");

    let systems = backend.systems.lock().unwrap();
    assert_eq!(systems.len(), 2, "2 ターン分の記録があること");
    assert_ne!(systems[0], systems[1], "顔ぶれの行は変わっている");

    // **安定部（顔ぶれの直前まで）は 1 字も変わらない。**
    let stable = |s: &String| s.split("## 今の顔ぶれ").next().unwrap().to_owned();
    assert_eq!(
        stable(&systems[0]),
        stable(&systems[1]),
        "役職を足しても安定プレフィックスは動かない（キャッシュが割れない）"
    );
    assert!(systems[1].contains("［調査役］"), "可変部にだけ現れる");
}

/// 機構 7: 役職表示が変わると System 行が 1 本出る。
/// **付与・改名・削除の 3 経路とも**（判定は「表示名が変わったか」の 1 点）。
#[tokio::test]
async fn every_role_display_change_leaves_one_system_line() {
    let dir = TempDir::new("role-system-line");
    let orchestrator = setup(&dir, OrchestratorConfig::default()).await;
    let id = AgentId::from("agent_01");
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "ザリ", "tpl"))
        .await
        .unwrap();
    orchestrator
        .upsert_role(agent_core::AgentRole {
            id: "researcher".into(),
            name: "調査役".into(),
            description: String::new(),
            color: None,
            defaults: agent_core::AgentRoleDefaults::default(),
        })
        .await
        .unwrap();

    let lines = |messages: Vec<agent_core::model::AgentMessage>| -> Vec<String> {
        messages
            .into_iter()
            .filter(|m| m.from == Endpoint::System && m.content.contains("役職"))
            .map(|m| m.content)
            .collect()
    };

    // (1) 付与。
    let mut spec = AgentSpec::new(id.clone(), "ザリ", "tpl");
    spec.role_id = Some("researcher".into());
    orchestrator.update_agent(spec).await.unwrap();
    let after_assign = lines(orchestrator.message_log(None).await);
    assert_eq!(after_assign.len(), 1, "付与で 1 本");
    assert!(after_assign[0].contains("調査役"));

    // (2) 改名（役職側を直す。個体は触っていない）。
    orchestrator
        .upsert_role(agent_core::AgentRole {
            id: "researcher".into(),
            name: "コード調査役".into(),
            description: String::new(),
            color: None,
            defaults: agent_core::AgentRoleDefaults::default(),
        })
        .await
        .unwrap();
    let after_rename = lines(orchestrator.message_log(None).await);
    assert_eq!(after_rename.len(), 2, "改名でも 1 本増える");
    assert!(after_rename[1].contains("コード調査役"));

    // (3) 削除。
    orchestrator.remove_role(&"researcher".into()).await.unwrap();
    let after_delete = lines(orchestrator.message_log(None).await);
    assert_eq!(after_delete.len(), 3, "削除でも 1 本増える");
    assert!(after_delete[2].contains("外れました"), "{}", after_delete[2]);
}

/// 表示が変わらない更新では System 行を出さない（同じ役職のまま設定だけ直す）。
#[tokio::test]
async fn an_update_that_keeps_the_role_stays_silent() {
    let dir = TempDir::new("role-silent");
    let orchestrator = setup(&dir, OrchestratorConfig::default()).await;
    let id = AgentId::from("agent_01");
    orchestrator
        .upsert_role(agent_core::AgentRole {
            id: "researcher".into(),
            name: "調査役".into(),
            description: String::new(),
            color: None,
            defaults: agent_core::AgentRoleDefaults::default(),
        })
        .await
        .unwrap();
    let mut spec = AgentSpec::new(id.clone(), "ザリ", "tpl");
    spec.role_id = Some("researcher".into());
    orchestrator.create_agent(spec.clone()).await.unwrap();

    let before = orchestrator.message_log(None).await.len();
    spec.hears_room_log = false; // 役職以外を変える
    orchestrator.update_agent(spec).await.unwrap();
    assert_eq!(
        orchestrator.message_log(None).await.len(),
        before,
        "役職表示が動かない更新では 1 行も増えない"
    );
}

/// **自分の役職名はプロンプトに入らない**（Spec 14 の D6・2026-08-04 利用者裁定）。
///
/// 顔ぶれに載るのは**他人**の役職だけで、自分の役職名はどこにも現れない。
///
/// # なぜ入れないか
///
/// **ペルソナが役職名に引きずられる**ため。人格は `Construct.md` / `SKILL.md` に
/// 書かれた文章が担っており、そこへ「あなたは助役です」と label を差し込むと、
/// モデルは語のもつ含意（従属的・補佐的）へ寄る。**役職は人が読む飾りであり、
/// 雛形の名前**であって、本人の自己認識の材料ではない。
///
/// 実機で観測した形（2026-08-04）: バッジが「助役」のザリに自分の役職を訊くと
/// 「進行役（オーケストレーター）」と答えた。**`SKILL.md` の役割定義から答えて
/// おり、ラベルは読んでいない** — これが意図した状態。
///
/// このテストは**親切心で足されるのを防ぐ**ためにある。顔ぶれに自分を含める、
/// 「# あなたについて」へ役職を書く、のどちらをやってもここが落ちる。
#[tokio::test]
async fn an_agent_never_sees_its_own_role_name_only_the_roles_of_others() {
    let dir = TempDir::new("role-self-blind");
    let backend = Arc::new(PromptProbeBackend {
        systems: std::sync::Mutex::new(Vec::new()),
    });
    let orchestrator = setup_with(&dir, backend.clone(), OrchestratorConfig::default()).await;

    for (id, name) in [("deputy", "助役"), ("librarian", "司書")] {
        orchestrator
            .upsert_role(agent_core::AgentRole {
                id: id.into(),
                name: name.into(),
                description: String::new(),
                color: None,
                defaults: agent_core::AgentRoleDefaults::default(),
            })
            .await
            .unwrap();
    }

    let me = AgentId::from("agent_01");
    let peer = AgentId::from("agent_02");

    let mut my_spec = AgentSpec::new(me.clone(), "ザリ", "tpl");
    my_spec.role_id = Some("deputy".into());
    orchestrator.create_agent(my_spec).await.unwrap();

    let mut peer_spec = AgentSpec::new(peer.clone(), "ジェミー", "tpl");
    peer_spec.role_id = Some("librarian".into());
    orchestrator.create_agent(peer_spec).await.unwrap();

    orchestrator
        .set_connections(&me, vec![peer.clone()])
        .await
        .unwrap();
    orchestrator.start_agent(&me).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&me, "自分の役職は？").await.unwrap();
    let _ = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let systems = backend.systems.lock().unwrap();
    let system = systems.first().expect("1 ターン分の記録があること");

    assert!(
        system.contains("司書"),
        "他人の役職は顔ぶれに載る（S3 の材料）: {system}"
    );
    assert!(
        !system.contains("助役"),
        "**自分の役職名はプロンプトのどこにも現れない**（D6）。\
         顔ぶれに自分を含めたか、「# あなたについて」へ役職を書いたのでは: {system}"
    );
}

// ---- 登録済みコマンド（Spec 15 P3）------------------------------------------

/// **提示されたツール定義**を記録する背骨。`RecordingBackend` は本文しか
/// 見ておらず、提示の検証には `req.tools` が要る。
#[derive(Default)]
struct ToolSpecBackend {
    tools: std::sync::Mutex<Vec<Vec<agent_core::llm::ToolSpec>>>,
}

#[async_trait::async_trait]
impl LlmBackend for ToolSpecBackend {
    fn name(&self) -> &str {
        "toolspec"
    }
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.tools.lock().unwrap().push(req.tools.clone());
        Ok(ChatResponse {
            text: Some("了解".into()),
            tool_calls: Vec::new(),
            finish: Finish::Stop,
            usage: Usage { prompt: 1, completion: 1, cache_read: 0 },
            grounding: Default::default(),
        })
    }
}

/// `run` を登録した村を 1 つ組む。ポリシーは `agents/{id}/command.json` に書く。
async fn setup_with_run(dir: &TempDir, backend: Arc<dyn LlmBackend>) -> Orchestrator {
    let orchestrator = setup_with(dir, backend, OrchestratorConfig::default()).await;
    orchestrator
        .register_tool(Arc::new(agent_core::RunTool::new(ConfigStore::new(&dir.0))))
        .await;
    // 既定集合の変更が**他の同梱ツールに波及していない**ことを見るための比較対象。
    // `setup_with` は同梱ツールを 1 本も登録しない（GUI 側の仕事）。
    orchestrator.register_tool(Arc::new(agent_core::GrepTool)).await;
    orchestrator
}

/// `agents/{id}/command.json` を書く。
async fn write_policy(orchestrator: &Orchestrator, id: &AgentId, allow: &[&str]) {
    let policy = serde_json::json!({
        "version": 1,
        "allow": allow,
        "deny": [],
        "pending": [],
        "timeoutSecs": 60,
    });
    orchestrator
        .write_config(id, ConfigFileKind::Command, &policy.to_string())
        .await
        .unwrap();
}

/// **既定では `run` を提示しない**（Spec 15 の破壊的変更）。
///
/// `enabledTools: null` の意味を「全同梱ツール」から「既定集合」へ変えた。
/// これが守られていないと、**アプリを更新した瞬間に全個体がコマンド実行能力を
/// 得る** — `batch_start_invariant`（開いただけで課金が始まる作りにしない）と
/// 同じ形で、更新しただけで実行能力が増える作りにしない。
#[tokio::test]
async fn run_is_not_presented_to_agents_that_did_not_ask_for_it() {
    let backend = Arc::new(ToolSpecBackend::default());
    let dir = TempDir::new("run-default-off");
    let orchestrator = setup_with_run(&dir, Arc::clone(&backend) as Arc<dyn LlmBackend>).await;

    let id = AgentId::from("agent_a");
    let mut spec = AgentSpec::new(id.clone(), "アルファ", "tpl");
    spec.work_dir = Some(dir.0.display().to_string());
    assert!(spec.enabled_tools.is_none(), "既定に従う個体であること");
    orchestrator.create_agent(spec).await.unwrap();
    write_policy(&orchestrator, &id, &["ruff *"]).await;
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "やあ").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let tools = backend.tools.lock().unwrap().clone();
    let names: Vec<&str> = tools[0].iter().map(|t| t.name.as_str()).collect();
    assert!(
        !names.contains(&"run"),
        "既定集合の外に居ること（更新しただけで実行能力が増えない）: {names:?}"
    );
    assert!(names.contains(&"grep"), "他の同梱ツールは既定のまま: {names:?}");
}

/// **オプトインしていても `allow` が空なら提示しない**（fail closed = D10）。
///
/// 提示は 2 段ゲート（`enabledTools` に `run` があるか × `allow` が 1 件以上あるか）。
/// `command.json` を持たない個体は何も実行できず、`run` は提示すらされない。
#[tokio::test]
async fn run_needs_both_the_opt_in_and_a_non_empty_allow_list() {
    let backend = Arc::new(ToolSpecBackend::default());
    let dir = TempDir::new("run-gate");
    let orchestrator = setup_with_run(&dir, Arc::clone(&backend) as Arc<dyn LlmBackend>).await;

    let id = AgentId::from("agent_a");
    let mut spec = AgentSpec::new(id.clone(), "アルファ", "tpl");
    spec.work_dir = Some(dir.0.display().to_string());
    spec.enabled_tools = Some(vec!["run".into(), "grep".into()]);
    orchestrator.create_agent(spec).await.unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    // command.json 無し = allow が空。
    orchestrator.send_user_message(&id, "1 回目").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    // 利用者が許可を書く。**次のターンから効く**（呼び出しの瞬間に読むため）。
    write_policy(&orchestrator, &id, &["ruff check *"]).await;
    orchestrator.send_user_message(&id, "2 回目").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    orchestrator.send_user_message(&id, "3 回目").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let tools = backend.tools.lock().unwrap().clone();
    let first: Vec<&str> = tools[0].iter().map(|t| t.name.as_str()).collect();
    assert!(
        !first.contains(&"run"),
        "allow が空なら提示しないこと（fail closed）: {first:?}"
    );

    let last = tools.last().unwrap();
    let described = last
        .iter()
        .find(|t| t.name == "run")
        .map(|t| t.description.clone())
        .expect("allow を書いたら提示されること");
    assert!(described.contains("`ruff check *`"), "allow を列挙すること: {described}");
    assert!(
        described.contains("引数なしの呼び出しにしか一致しない"),
        "`*` の有無の差を書くこと（利用者の指摘）: {described}"
    );
}

/// **`deny` はモデルへ見せない。**
///
/// 見せると「やってはいけないこと」の一覧を毎ターン積むことになり、
/// **トークンを払って禁止の方法を教える**形になる。
#[tokio::test]
async fn the_deny_list_is_never_shown_to_the_model() {
    let backend = Arc::new(ToolSpecBackend::default());
    let dir = TempDir::new("run-deny-hidden");
    let orchestrator = setup_with_run(&dir, Arc::clone(&backend) as Arc<dyn LlmBackend>).await;

    let id = AgentId::from("agent_a");
    let mut spec = AgentSpec::new(id.clone(), "アルファ", "tpl");
    spec.work_dir = Some(dir.0.display().to_string());
    spec.enabled_tools = Some(vec!["run".into()]);
    orchestrator.create_agent(spec).await.unwrap();

    let policy = serde_json::json!({
        "version": 1,
        "allow": ["ruff *"],
        "deny": ["shutdown *", "format-disk *"],
        "pending": [],
        "timeoutSecs": 60,
    });
    orchestrator
        .write_config(&id, ConfigFileKind::Command, &policy.to_string())
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&id, "1 回目").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    orchestrator.send_user_message(&id, "2 回目").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let tools = backend.tools.lock().unwrap().clone();
    let described = tools
        .last()
        .unwrap()
        .iter()
        .find(|t| t.name == "run")
        .map(|t| t.description.clone())
        .expect("allow があるので提示される");
    assert!(described.contains("`ruff *`"), "{described}");
    assert!(!described.contains("shutdown"), "deny を見せないこと: {described}");
    assert!(!described.contains("format-disk"), "deny を見せないこと: {described}");
}
