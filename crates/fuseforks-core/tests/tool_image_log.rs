//! **ツール結果の画像の計器**を機械で留める（Spec 70 P1 / `mcp_image_result_contract` 凍結 6・7）。
//!
//! - `tool:` 行の `images=N` は**画像を返したときだけ**（返さないツールの行は 1 バイトも変わらない）
//! - 落とした画像は `tool image dropped:` の 1 行（理由は計器の語。画像の中身は書かない）
//! - 画像を外して送り直したら `tool image fallback:` の 1 行（結末つき）
//!
//! **負の対照を同じログで取る**（`failures.md` #90）— 本文だけのツールの `tool:` 行に
//! `images=` が無いこと。これが無いと「常に `images=` を書く実装」でも緑になる。
//!
//! **診断の出口はプロセスで 1 つ**（`OnceLock`）なので、ログを読むテストはこのファイルに 1 つだけ。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use fuseforks_core::event::CoreEvent;
use fuseforks_core::llm::{
    ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, Role, ToolCall, Usage,
};
use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::tool::{AgentTool, ToolContext, ToolOutput};
use fuseforks_core::tool_image::DropReason;
use fuseforks_core::world::Language;
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("fuseforks-tool-image-log-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 画像 1 枚を返し、1 枚を落とした（GIF だった）ことにするツール。
struct CameraTool;

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
        let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            8,
            6,
            image::Rgb([0, 200, 0]),
        ));
        let mut jpeg = std::io::Cursor::new(Vec::new());
        image
            .write_to(&mut jpeg, image::ImageFormat::Jpeg)
            .expect("書けること");
        let data = base64::engine::general_purpose::STANDARD.encode(jpeg.into_inner());
        Ok(ToolOutput {
            text: "撮りました".to_owned(),
            images: vec![fuseforks_core::tool_image::normalize(&data).expect("通ること")],
            dropped: vec![DropReason::Format],
        })
    }
}

/// 本文だけを返すツール（負の対照）。
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

/// `camera` → `plain` の順に呼んで答える。ツール結果に画像が載った要求は 400（画像を読めないモデル）。
struct RejectsImages;

#[async_trait::async_trait]
impl LlmBackend for RejectsImages {
    fn name(&self) -> &str {
        "rejects-images"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        if req
            .messages
            .iter()
            .any(|m| m.role == Role::Tool && !m.attachments.is_empty())
        {
            return Err(LlmError::api(400, "image input is not supported"));
        }
        let usage = Usage {
            prompt: 10,
            completion: 5,
            cache_read: 0,
            cache_write: 0,
            cache_write_1h: 0,
            reasoning: 0,
        };
        let done_calls = req.messages.iter().filter(|m| m.role == Role::Tool).count();
        let name = match done_calls {
            0 => "camera",
            1 => "plain",
            _ => {
                return Ok(ChatResponse {
                    text: Some("見ました".to_owned()),
                    tool_calls: Vec::new(),
                    finish: Finish::Stop,
                    usage,
                    grounding: Default::default(),
                    reasoning_summary: Vec::new(),
                });
            }
        };
        Ok(ChatResponse {
            text: Some(String::new()),
            tool_calls: vec![ToolCall {
                id: format!("call_{done_calls}"),
                name: name.to_owned(),
                args: serde_json::json!({}),
                extra: None,
            }],
            finish: Finish::ToolUse,
            usage,
            grounding: Default::default(),
            reasoning_summary: Vec::new(),
        })
    }
}

#[tokio::test]
async fn image_counters_appear_only_when_images_do() {
    let dir = TempDir::new();
    let log_path = dir.0.join("fuseforks.log");
    fuseforks_core::open_log(&log_path).expect("ログを開けること");

    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(Arc::new(RejectsImages))),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig {
            schedule_interval: Duration::from_secs(3600),
            ..OrchestratorConfig::default()
        },
    )
    .await
    .expect("bootstrap できること");
    orchestrator.set_language(Language::Ja).await.unwrap();
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();
    orchestrator.register_tool(Arc::new(CameraTool)).await;
    orchestrator.register_tool(Arc::new(PlainTool)).await;

    let agent = AgentId::from("agent_log");
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
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;

    let body = std::fs::read_to_string(&log_path).expect("ログが読めること");
    let line = |needle: &str| {
        body.lines()
            .find(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("`{needle}` の行が出ること:\n{body}"))
            .to_owned()
    };

    // 1. 画像を返したツールの行にだけ `images=`。
    let camera = line("tool: agent=agent_log round=1 name=camera ");
    assert!(camera.ends_with(" images=1"), "実際: {camera}");
    // 2. 負の対照 — 本文だけのツールの行は Spec 70 の前と同じ（`images=` が無い）。
    let plain = line("tool: agent=agent_log round=2 name=plain ");
    assert!(!plain.contains("images="), "実際: {plain}");

    // 3. 落とした画像は理由の語と枚数だけ。
    let dropped = line("tool image dropped: agent=agent_log ");
    assert!(
        dropped.ends_with("round=1 name=camera count=1 reason=format:1"),
        "実際: {dropped}"
    );

    // 4. 送り直しは 1 回だけで、結末つき。
    let fallback = line("tool image fallback: agent=agent_log ");
    assert!(
        fallback.ends_with("model=mock-model removed=1 outcome=ok"),
        "実際: {fallback}"
    );
    assert_eq!(
        body.matches("tool image fallback: agent=agent_log ").count(),
        1,
        "次の周の画像は最初から載せない = 2 回目の送り直しは無い"
    );
    assert!(!body.contains("tool image capped:"), "20 枚に届いていない");
}

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
