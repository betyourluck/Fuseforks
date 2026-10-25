//! **MCP のツール結果の画像が、ターンの層を通ってモデルへの要求に載る**ことを機械で留める
//! （Spec 70 P1 / `mcp_image_result_contract`）。
//!
//! 画像を返すツールは `call_with_media` を上書きした偽物で、本物の MCP サーバーは立てない
//! （`McpTool` の本文と画像の組み立ては `mcp.rs` の単体が見る）。ここで見るのは、その結果を
//! 受けたターンの層の振る舞い — 画像を載せる場所（凍結 2）・1 要求 20 枚（3b）・
//! RepeatGuard（5）・400 の送り直し（6）・表示の 1 行（7）。
//!
//! **adapter はまだ画像を組み立てない**（P2）ので、見るのは canonical の要求（`ChatRequest`）。

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use fuseforks_core::event::CoreEvent;
use fuseforks_core::llm::{
    ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, Role, ToolCall, ToolChoice, Usage,
};
use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::tool::{AgentTool, ToolContext, ToolOutput};
use fuseforks_core::world::Language;
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};

/// テストごとに別のフォルダ（同じバイナリの中で並行に走るので、プロセス番号だけでは衝突する）。
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "fuseforks-tool-image-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
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

/// 1 色の小さな JPEG（base64）。`shade` を変えると要約値が変わる。
fn jpeg_b64(shade: u8) -> String {
    let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        8,
        6,
        image::Rgb([shade, 200, 0]),
    ));
    let mut out = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut out, image::ImageFormat::Jpeg)
        .expect("書けること");
    base64::engine::general_purpose::STANDARD.encode(out.into_inner())
}

/// 画像を返す偽のツール（`McpTool` の代わり）。
///
/// - `per_call`: 1 回に返す画像の枚数
/// - `vary`: 呼ぶたびに画像を変えるか（偽なら毎回同じバイト列）
struct CameraTool {
    runs: AtomicUsize,
    per_call: usize,
    vary: bool,
}

impl CameraTool {
    fn new(per_call: usize, vary: bool) -> Self {
        Self {
            runs: AtomicUsize::new(0),
            per_call,
            vary,
        }
    }
}

#[async_trait::async_trait]
impl AgentTool for CameraTool {
    fn name(&self) -> &str {
        "camera"
    }
    fn description(&self, _language: Language) -> String {
        "テスト用。画像を返す".into()
    }
    fn parameters(&self, _language: Language) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    async fn call(
        &self,
        ctx: &ToolContext,
        args: &serde_json::Value,
    ) -> fuseforks_core::CoreResult<String> {
        self.call_with_media(ctx, args).await.map(|output| output.text)
    }
    async fn call_with_media(
        &self,
        _ctx: &ToolContext,
        _args: &serde_json::Value,
    ) -> fuseforks_core::CoreResult<ToolOutput> {
        let run = self.runs.fetch_add(1, Ordering::SeqCst);
        let images = (0..self.per_call)
            .map(|i| {
                let shade = if self.vary {
                    u8::try_from((run * self.per_call + i) % 250).unwrap()
                } else {
                    0
                };
                fuseforks_core::tool_image::normalize(&jpeg_b64(shade)).expect("通ること")
            })
            .collect();
        Ok(ToolOutput {
            // **本文は毎回同じ** — 画像だけが違う結果（ライブビューの撮り直し）の形。
            text: "{\"width\":8}".to_owned(),
            images,
            dropped: Vec::new(),
        })
    }
}

/// 本文だけを返すツール（画像を返さないツールが変わらないことの対照）。
struct PlainTool;

#[async_trait::async_trait]
impl AgentTool for PlainTool {
    fn name(&self) -> &str {
        "plain"
    }
    fn description(&self, _language: Language) -> String {
        "テスト用。本文だけを返す".into()
    }
    fn parameters(&self, _language: Language) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    async fn call(
        &self,
        _ctx: &ToolContext,
        _args: &serde_json::Value,
    ) -> fuseforks_core::CoreResult<String> {
        Ok("文字だけ".to_owned())
    }
}

/// 筋書き: ツールの名前を順に呼び、尽きたら答える。要求はすべて記録する。
///
/// `reject_images` なら、ツール結果に画像が載った要求を 400 で返す（画像を読めないモデル）。
/// `vary_args` なら、呼ぶたびに引数を変える（RepeatGuard に掛けないため）。
struct Script {
    calls: Vec<&'static str>,
    reject_images: bool,
    vary_args: bool,
    requests: Mutex<Vec<ChatRequest>>,
}

impl Script {
    fn new(calls: Vec<&'static str>) -> Self {
        Self {
            calls,
            reject_images: false,
            vary_args: false,
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<ChatRequest> {
        self.requests.lock().unwrap().clone()
    }
}

fn usage() -> Usage {
    Usage {
        prompt: 10,
        completion: 5,
        cache_read: 0,
        cache_write: 0,
        cache_write_1h: 0,
        reasoning: 0,
    }
}

fn done(text: &str) -> ChatResponse {
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

#[async_trait::async_trait]
impl LlmBackend for Script {
    fn name(&self) -> &str {
        "tool-image-script"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.requests.lock().unwrap().push(req.clone());
        let carries_images = req
            .messages
            .iter()
            .any(|m| m.role == Role::Tool && !m.attachments.is_empty());
        if self.reject_images && carries_images {
            return Err(LlmError::api(400, "image input is not supported"));
        }
        // まとめ呼び出し（ツールの使用を禁じた 1 回）には答える。
        if matches!(req.tool_choice, ToolChoice::None) {
            return Ok(done("まとめました"));
        }
        let done_calls = req.messages.iter().filter(|m| m.role == Role::Tool).count();
        let Some(name) = self.calls.get(done_calls) else {
            return Ok(done("見ました"));
        };
        let args = if self.vary_args {
            serde_json::json!({ "n": done_calls })
        } else {
            serde_json::json!({})
        };
        Ok(ChatResponse {
            text: Some(String::new()),
            tool_calls: vec![ToolCall {
                id: format!("call_{done_calls}"),
                name: (*name).to_owned(),
                args,
                extra: None,
            }],
            finish: Finish::ToolUse,
            usage: usage(),
            grounding: Default::default(),
            reasoning_summary: Vec::new(),
            replay: None,
        })
    }
}

/// 村を 1 つ建て、ツールを登録し、1 体へ依頼して、イベントが止むまで待つ。
async fn run(
    backend: Arc<Script>,
    tools: Vec<Arc<dyn AgentTool>>,
    language: Language,
) -> (Orchestrator, Vec<CoreEvent>, TempDir) {
    let dir = TempDir::new();
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(backend)),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig {
            schedule_interval: Duration::from_secs(3600),
            ..OrchestratorConfig::default()
        },
    )
    .await
    .expect("bootstrap できること");
    // ホストの OS ロケールに依存させない（#101）。
    orchestrator.set_language(language).await.unwrap();
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();
    for tool in tools {
        orchestrator.register_tool(tool).await;
    }
    let agent = AgentId::from("agent_cam");
    orchestrator
        .create_agent(AgentSpec::new(agent.clone(), "カメラ係", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&agent).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&agent, "カメラを見て")
        .await
        .unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    (orchestrator, events, dir)
}

fn tool_messages(req: &ChatRequest) -> Vec<&fuseforks_core::llm::ChatMessage> {
    req.messages.iter().filter(|m| m.role == Role::Tool).collect()
}

/// 凍結 2 — 画像はツール結果の `ChatMessage.attachments` に載り、代替の本文（村の言語）が
/// 一緒に渡る。本文はテキストのブロックだけで、表示の 1 行（凍結 7）はモデルへ送らない。
#[tokio::test]
async fn an_image_rides_on_the_tool_result_message() {
    let backend = Arc::new(Script::new(vec!["camera"]));
    let (orchestrator, events, _dir) = run(
        backend.clone(),
        vec![Arc::new(CameraTool::new(1, true))],
        Language::Ja,
    )
    .await;

    let requests = backend.requests();
    assert_eq!(requests.len(), 2, "呼ぶ 1 回 + 結果を受けて答える 1 回");
    let results = tool_messages(&requests[1]);
    assert_eq!(results.len(), 1);
    let result = results[0];
    assert_eq!(result.attachments.len(), 1, "画像がツール結果に載ること");
    assert_eq!(
        result.attachments[0].media_type,
        fuseforks_core::llm::PromptMediaType::Jpeg
    );
    assert_eq!(
        result.attachment_caption.as_deref(),
        Some("（camera の結果の画像 1 枚）"),
        "代替の本文はターンの層が村の言語で組むこと"
    );
    assert_eq!(result.content, "{\"width\":8}", "モデルへ送る本文に表示の 1 行を足さない");

    // 表示（Spec 57 のリング）には画像の枚数・形式・大きさを 1 行足す。画像そのものは持たない。
    let call_id = events
        .iter()
        .find_map(|e| match e {
            CoreEvent::ToolInvoked { tool, call_id, .. } if tool == "camera" => Some(*call_id),
            _ => None,
        })
        .expect("ToolInvoked が出ること");
    let detail = orchestrator.tool_call(call_id).expect("リングに在ること");
    assert!(
        detail.body.starts_with("{\"width\":8}\n（画像 1 枚: JPEG "),
        "実際: {}",
        detail.body
    );
}

/// 凍結 1 / Goal 2 — 画像を返さないツールの結果は Spec 70 の前と同じ形（欄が増えない）。
#[tokio::test]
async fn a_text_only_tool_result_is_unchanged() {
    let backend = Arc::new(Script::new(vec!["plain"]));
    let (_orchestrator, _events, _dir) =
        run(backend.clone(), vec![Arc::new(PlainTool)], Language::Ja).await;

    let requests = backend.requests();
    let result = tool_messages(&requests[1])[0];
    let wire = serde_json::to_value(result).unwrap();
    assert_eq!(
        wire,
        serde_json::json!({
            "role": "tool",
            "content": "文字だけ",
            "tool_call_id": "call_0",
            "tool_name": "plain",
        }),
        "画像の欄が生えないこと"
    );
}

/// 凍結 6 — 画像を読めないモデルの 400 で、画像を外して 1 回だけ送り直す。
/// **以後の周の画像は載せない**（同じ 400 を毎周踏まない）。足す文は村の言語。
#[tokio::test]
async fn a_rejected_image_is_stripped_and_the_turn_goes_on() {
    let mut script = Script::new(vec!["camera", "camera"]);
    script.reject_images = true;
    script.vary_args = true;
    let backend = Arc::new(script);
    let (_orchestrator, events, _dir) = run(
        backend.clone(),
        vec![Arc::new(CameraTool::new(1, true))],
        Language::En,
    )
    .await;

    let requests = backend.requests();
    let carried: Vec<usize> = requests
        .iter()
        .map(|r| tool_messages(r).iter().map(|m| m.attachments.len()).sum())
        .collect();
    assert_eq!(
        carried,
        vec![0, 1, 0, 0],
        "画像つきの 1 回が 400 → 外して送り直し → 次の周は最初から載せない"
    );
    let last = requests.last().unwrap();
    let results = tool_messages(last);
    assert_eq!(results.len(), 2);
    for result in results {
        assert!(
            result
                .content
                .ends_with("(The images could not be sent to this model.)"),
            "英語の村の注記: {}",
            result.content
        );
        assert!(result.attachment_caption.is_none());
    }
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, CoreEvent::AgentFailed { .. })),
        "ターンが落ちないこと（Goal 4）: {events:?}"
    );
}

/// 凍結 5 — 本文が同じで画像だけが違う結果は、繰り返しではない（撮り直し）。
#[tokio::test]
async fn the_same_text_with_a_new_image_is_not_a_repeat() {
    let backend = Arc::new(Script::new(vec!["camera"; 4]));
    let tool = Arc::new(CameraTool::new(1, true));
    let (_orchestrator, events, _dir) = run(backend, vec![tool.clone()], Language::Ja).await;

    assert_eq!(tool.runs.load(Ordering::SeqCst), 4, "4 回とも走ること");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, CoreEvent::ToolRepeatBlocked { .. })),
        "画像が違えば止めないこと: {events:?}"
    );
}

/// 凍結 5 の対照 — 本文も画像も同じなら、今までどおり 3 回目を止める。
/// これが無いと「画像があれば常に止めない」実装でも上の 1 本は緑になる。
#[tokio::test]
async fn the_same_text_and_the_same_image_is_still_a_repeat() {
    let backend = Arc::new(Script::new(vec!["camera"; 4]));
    let tool = Arc::new(CameraTool::new(1, false));
    let (_orchestrator, events, _dir) = run(backend, vec![tool.clone()], Language::Ja).await;

    assert_eq!(tool.runs.load(Ordering::SeqCst), 2, "3 回目は実行しないこと");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, CoreEvent::ToolRepeatBlocked { .. })),
        "同じ結果の 3 回目を止めること: {events:?}"
    );
}

/// 凍結 3b — 1 要求に載せるツール結果の画像は新しいものから 20 枚まで。
/// 4 枚 × 6 回 = 24 枚 → 最も古い結果の 4 枚を外し、その結果の本文に一文。
#[tokio::test]
async fn images_beyond_twenty_drop_the_oldest() {
    let mut script = Script::new(vec!["camera"; 6]);
    script.vary_args = true;
    let backend = Arc::new(script);
    let (_orchestrator, _events, _dir) = run(
        backend.clone(),
        vec![Arc::new(CameraTool::new(4, true))],
        Language::Ja,
    )
    .await;

    let requests = backend.requests();
    let last = requests.last().unwrap();
    let per_result: Vec<usize> = tool_messages(last)
        .iter()
        .map(|m| m.attachments.len())
        .collect();
    assert_eq!(per_result, vec![0, 4, 4, 4, 4, 4]);
    let oldest = tool_messages(last)[0];
    assert!(
        oldest.content.ends_with("（前の周の画像は省きました）"),
        "外した結果に一文: {}",
        oldest.content
    );
    assert!(oldest.attachment_caption.is_none());

    // 20 枚ちょうど（5 回ぶん）の要求では何も外さない。
    let fifth: Vec<usize> = tool_messages(&requests[5])
        .iter()
        .map(|m| m.attachments.len())
        .collect();
    assert_eq!(fifth, vec![4, 4, 4, 4, 4]);
}

/// イベントが止むまで集める（`tests/orchestrator.rs` と同じ作法）。
///
/// 窓は**この系で最も短い定期イベントの周期より短く**する（`failures.md` #86）。
async fn drain_until_quiet(
    rx: &mut tokio::sync::broadcast::Receiver<CoreEvent>,
    quiet: Duration,
) -> Vec<CoreEvent> {
    let mut out = Vec::new();
    loop {
        match tokio::time::timeout(quiet, rx.recv()).await {
            Ok(Ok(event)) => out.push(event),
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
            _ => break,
        }
    }
    out
}
