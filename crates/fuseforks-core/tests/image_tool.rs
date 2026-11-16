//! 同梱ツール `image`（Spec 76 / `image_gen_contract`）を、ループバックの HTTP スタブ越しに踏む。
//!
//! 見るのは 2 層 —
//!
//! - **ツール単体**: 4 社の口（URL・鍵の載せ方・本文の形）/ 参照番号（利用者の添付 → 生成した写しの続き）/
//!   生成を始めずに断るもの（設定が無効・参照番号の誤り・5 回目）はスタブに 1 本も届かない /
//!   打ち切りは要求ごと切り、何も書かない / 作業フォルダが無くても画像は返る
//! - **ターン**: 返事の発話に写しが付き、添付の置き場に書かれる / チェックが無い個体には提示しない /
//!   サーヴァントの発話の添付を「入力欄へ」で渡し直せる（S76-3）
//!
//! スタブは `attachment_fallback.rs` と同じ最小の HTTP/1.1。受け取った要求（パス・ヘッダ・本文）を
//! 全部記録するので、**何本届いたか**を後から数えられる。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use fuseforks_core::event::CoreEvent;
use fuseforks_core::image_gen::{ImageAspect, ImageGenConfig, ImageProvider};
use fuseforks_core::llm::{
    ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, PromptAttachment, PromptMediaType, Role,
    ToolCall, ToolChoice, Usage,
};
use fuseforks_core::model::{AgentId, AgentSpec, Endpoint, ModelTemplate};
use fuseforks_core::tool::{AgentTool, ToolContext};
use fuseforks_core::tools::image::{ActiveImageGen, ImageGenSlot, ImageTool, TurnImages};
use fuseforks_core::world::Language;
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};

/// テストごとに別のフォルダ。
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "fuseforks-image-tool-{}-{}",
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

/// スタブの応答の決め方。
#[derive(Clone, Copy)]
enum Policy {
    /// 口ごとの成功の形で 1 枚返す。
    Ok,
    /// この状態コードを返す。
    Status(u16),
    /// これだけ待ってから成功の形で返す（打ち切りの検査）。
    Slow(u64),
}

/// 受け取った要求の記録。
#[derive(Debug, Clone)]
struct Seen {
    path: String,
    headers: String,
    body: Vec<u8>,
}

impl Seen {
    fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

type Log = Arc<Mutex<Vec<Seen>>>;

/// 返す画像（小さな PNG）。
fn png_bytes() -> Vec<u8> {
    let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(32, 24, image::Rgb([10, 200, 30])));
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

/// 口（パス）ごとの成功の形。
fn success_body(path: &str) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(png_bytes());
    if path.contains(":generateContent") {
        format!(
            r#"{{"candidates":[{{"content":{{"parts":[{{"inlineData":{{"mimeType":"image/png","data":"{b64}"}}}}]}},"finishReason":"STOP"}}],"usageMetadata":{{"promptTokenCount":29,"candidatesTokenCount":1526}}}}"#
        )
    } else if path.ends_with("/responses") {
        format!(
            r#"{{"status":"completed","output":[{{"type":"image_generation_call","result":"{b64}"}}],"usage":{{"input_tokens":4031,"output_tokens":919}}}}"#
        )
    } else {
        format!(
            r#"{{"data":[{{"b64_json":"{b64}"}}],"usage":{{"input_tokens":38,"output_tokens":272,"cost_in_usd_ticks":400000000}}}}"#
        )
    }
}

/// ループバックに最小の HTTP スタブを立て、`(base_url, 記録)` を返す。
async fn spawn_stub(policy: Policy) -> (String, Log) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&log);
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let recorded = Arc::clone(&recorded);
            tokio::spawn(async move {
                let mut raw = Vec::new();
                let mut buf = [0u8; 16384];
                let (head, body) = loop {
                    let Ok(n) = socket.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    let Some(head_end) = raw.windows(4).position(|w| w == b"\r\n\r\n") else {
                        continue;
                    };
                    let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
                    let len: usize = head
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse().ok())?
                        })
                        .unwrap_or(0);
                    if raw.len() >= head_end + 4 + len {
                        break (head, raw[head_end + 4..head_end + 4 + len].to_vec());
                    }
                };
                let path = head
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("")
                    .to_owned();
                recorded.lock().unwrap().push(Seen {
                    path: path.clone(),
                    headers: head.to_ascii_lowercase(),
                    body,
                });
                let (status, text) = match policy {
                    Policy::Ok => (200, success_body(&path)),
                    Policy::Status(code) => (code, r#"{"error":{"message":"nope"}}"#.to_owned()),
                    Policy::Slow(ms) => {
                        tokio::time::sleep(Duration::from_millis(ms)).await;
                        (200, success_body(&path))
                    }
                };
                let response = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
                    text.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    (format!("http://127.0.0.1:{port}"), log)
}

fn active(provider: ImageProvider, base_url: &str) -> ActiveImageGen {
    let mut config = ImageGenConfig::new(provider);
    config.base_url = base_url.to_owned();
    ActiveImageGen {
        config,
        key: "test-key".to_owned(),
        default_aspect: ImageAspect::Square,
    }
}

fn ctx(work_dir: Option<&Path>, ledger: &Arc<TurnImages>) -> ToolContext {
    ToolContext {
        agent_id: AgentId::from("agent_x"),
        work_dir: work_dir.map(Path::to_path_buf),
        cancel: None,
        rag_roots: Vec::new(),
        agent_names: Vec::new(),
        uses_blackboard: false,
        language: Language::Ja,
        run_approval: fuseforks_core::command::RunApproval::default(),
        judges_dir: None,
        turn_images: Some(Arc::clone(ledger)),
    }
}

fn tool_with(provider: ImageProvider, base_url: &str) -> (ImageTool, Arc<ImageGenSlot>) {
    let slot = Arc::new(ImageGenSlot::new());
    slot.set(Some(active(provider, base_url)));
    (ImageTool::new(Arc::clone(&slot)), slot)
}

fn webp_ref() -> PromptAttachment {
    PromptAttachment::new(PromptMediaType::Webp, base64::engine::general_purpose::STANDARD.encode(b"RIFF0000WEBPVP8 "))
}

fn files_in(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// 生成 → 元は作業フォルダへ・写しはモデルへ（JPEG に寄せる）・台帳へ。鍵は Bearer で載る。
#[tokio::test]
async fn generates_saves_the_original_and_returns_a_copy() {
    let (base, log) = spawn_stub(Policy::Ok).await;
    let (tool, _slot) = tool_with(ImageProvider::Xai, &base);
    let work = TempDir::new();
    let ledger = Arc::new(TurnImages::default());

    let output = tool
        .call_with_media(&ctx(Some(&work.0), &ledger), &serde_json::json!({"prompt": "苔の地面"}))
        .await
        .unwrap();

    assert!(output.text.contains("参照番号 1"), "{}", output.text);
    assert!(output.text.contains("generated_images/"), "{}", output.text);
    assert_eq!(output.images.len(), 1, "写しをモデルへ返す");
    assert_eq!(
        output.images[0].attachment.media_type,
        PromptMediaType::Jpeg,
        "PNG は JPEG へ寄せる（tool_image::normalize と同じ 1 実装）"
    );
    let saved = files_in(&work.0.join("generated_images"));
    assert_eq!(saved.len(), 1, "{saved:?}");
    assert!(saved[0].contains("-agent_x-1.png"), "元の形式のまま: {saved:?}");
    assert_eq!(ledger.take_generated().len(), 1, "写しは台帳へ置く（返事の発話に付けるため）");

    let seen = log.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].path, "/v1/images/generations");
    assert!(seen[0].headers.contains("authorization: bearer test-key"));
    let body: serde_json::Value = serde_json::from_slice(&seen[0].body).unwrap();
    assert_eq!(body["aspect_ratio"], "1:1");
    assert_eq!(body["resolution"], "1k", "解像度は設定の既定（モデルに選ばせない）");
}

/// 参照番号: 利用者の添付が 1 から、そのターンに生成した写しがその続き。参照ありは edits の口。
#[tokio::test]
async fn references_follow_the_turn_numbering() {
    let (base, log) = spawn_stub(Policy::Ok).await;
    let (tool, _slot) = tool_with(ImageProvider::Xai, &base);
    let ledger = Arc::new(TurnImages::new(vec![webp_ref()]));
    let context = ctx(None, &ledger);

    let first = tool
        .call_with_media(&context, &serde_json::json!({"prompt": "寄せて", "references": [1]}))
        .await
        .unwrap();
    assert!(first.text.contains("参照番号 2"), "利用者の添付の続き: {}", first.text);
    let second = tool
        .call_with_media(&context, &serde_json::json!({"prompt": "直して", "references": [2]}))
        .await
        .unwrap();
    assert!(second.text.contains("参照番号 3"), "{}", second.text);

    let seen = log.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].path, "/v1/images/edits");
    assert!(seen[0].body_text().contains("data:image/webp;base64,"), "利用者の添付は WebP のまま送る");
    assert!(
        seen[1].body_text().contains("data:image/jpeg;base64,"),
        "生成した写し（JPEG）を参照として送る"
    );
}

/// 生成を始めずに断るもの（設定が無効・参照番号の誤り・引数の誤り）はスタブに届かず、上限にも数えない。
/// 送った要求だけが 4 回まで。
#[tokio::test]
async fn refusals_never_reach_the_provider_and_do_not_count() {
    let (base, log) = spawn_stub(Policy::Ok).await;
    let (tool, slot) = tool_with(ImageProvider::Xai, &base);
    let ledger = Arc::new(TurnImages::default());
    let context = ctx(None, &ledger);
    let prompt = |n: usize| serde_json::json!({"prompt": format!("{n} 枚目")});

    let bad_ref = tool
        .call_with_media(&context, &serde_json::json!({"prompt": "x", "references": [5]}))
        .await
        .unwrap();
    assert!(bad_ref.text.contains("参照できる画像がありません"), "{}", bad_ref.text);
    let bad_args = tool.call_with_media(&context, &serde_json::json!({})).await.unwrap();
    assert!(bad_args.text.contains("prompt"), "{}", bad_args.text);
    assert_eq!(log.lock().unwrap().len(), 0, "断ったものは送らない");

    for n in 1..=4 {
        let output = tool.call_with_media(&context, &prompt(n)).await.unwrap();
        assert_eq!(output.images.len(), 1, "{n} 回目は通る: {}", output.text);
    }
    let fifth = tool.call_with_media(&context, &prompt(5)).await.unwrap();
    assert!(fifth.text.contains("4 回使いました"), "{}", fifth.text);
    assert!(fifth.images.is_empty());
    assert_eq!(log.lock().unwrap().len(), 4, "5 回目は送らない");

    slot.set(None);
    let inactive = tool.call_with_media(&ctx(None, &Arc::new(TurnImages::default())), &prompt(1)).await.unwrap();
    assert!(inactive.text.contains("今は使えません"), "{}", inactive.text);
    assert_eq!(log.lock().unwrap().len(), 4, "呼び出しの時にも門を見る");
}

/// 4 社がそれぞれの口・鍵の載せ方・本文の形で往復する。
#[tokio::test]
async fn each_provider_round_trips_through_its_own_mouth() {
    let (base, log) = spawn_stub(Policy::Ok).await;
    for provider in ImageProvider::ALL {
        let (tool, _slot) = tool_with(provider, &base);
        let ledger = Arc::new(TurnImages::new(vec![webp_ref()]));
        let context = ctx(None, &ledger);
        let plain = tool
            .call_with_media(&context, &serde_json::json!({"prompt": "cat"}))
            .await
            .unwrap();
        assert_eq!(plain.images.len(), 1, "{provider:?}: {}", plain.text);
        let with_ref = tool
            .call_with_media(&context, &serde_json::json!({"prompt": "cat", "references": [1]}))
            .await
            .unwrap();
        assert_eq!(with_ref.images.len(), 1, "{provider:?}: {}", with_ref.text);
    }
    let seen = log.lock().unwrap().clone();
    let paths: Vec<&str> = seen.iter().map(|s| s.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "/v1/images/generations",
            "/v1/images/edits",
            "/v1beta/models/gemini-3.1-flash-lite-image:generateContent",
            "/v1beta/models/gemini-3.1-flash-lite-image:generateContent",
            "/v1/responses",
            "/v1/responses",
            "/v1/images/generations",
            "/v1/images/edits",
        ]
    );
    // OpenAI の参照ありは multipart。
    assert!(seen[1].headers.contains("content-type: multipart/form-data; boundary="));
    assert!(seen[1].body_text().contains("name=\"image[]\"; filename=\"ref1.webp\""));
    // Gemini は x-goog-api-key、ほかは Bearer。
    assert!(seen[2].headers.contains("x-goog-api-key: test-key"));
    assert!(!seen[2].headers.contains("authorization:"));
    assert!(seen[4].headers.contains("authorization: bearer test-key"));
    // Meta は解像度を写さず store: false。
    let meta: serde_json::Value = serde_json::from_slice(&seen[4].body).unwrap();
    assert_eq!(meta["store"], false);
    assert!(meta["tools"][0].get("quality").is_none());
}

/// 401 は鍵の失敗として本文に書く（送った要求なので上限には数える）。
#[tokio::test]
async fn a_401_is_reported_as_the_key() {
    let (base, _log) = spawn_stub(Policy::Status(401)).await;
    let (tool, _slot) = tool_with(ImageProvider::Openai, &base);
    let ledger = Arc::new(TurnImages::default());
    let output = tool
        .call_with_media(&ctx(None, &ledger), &serde_json::json!({"prompt": "x"}))
        .await
        .unwrap();
    assert!(output.text.contains("401"), "{}", output.text);
    assert!(output.images.is_empty());
    assert_eq!(ledger.reserve(), Some(2), "失敗した要求も 1 回に数える");
}

/// 打ち切りは要求ごと切る（応答を待たない）。元も写しも書かない。
#[tokio::test]
async fn an_interrupt_drops_the_request_and_writes_nothing() {
    let (base, _log) = spawn_stub(Policy::Slow(10_000)).await;
    let (tool, _slot) = tool_with(ImageProvider::Xai, &base);
    let work = TempDir::new();
    let ledger = Arc::new(TurnImages::default());
    let token = tokio_util::sync::CancellationToken::new();
    let mut context = ctx(Some(&work.0), &ledger);
    context.cancel = Some(token.clone());
    let canceller = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        token.cancel();
    });
    let started = std::time::Instant::now();
    let output = tool
        .call_with_media(&context, &serde_json::json!({"prompt": "x"}))
        .await
        .unwrap();
    canceller.await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(5), "応答を待たない: {:?}", started.elapsed());
    assert!(output.text.contains("打ち切られた"), "{}", output.text);
    assert!(output.images.is_empty());
    assert!(files_in(&work.0.join("generated_images")).is_empty(), "元を書かない");
    assert!(ledger.take_generated().is_empty(), "写しを置かない");
}

/// 作業フォルダが無くても画像は返り、保存しなかったことを本文に書く。
#[tokio::test]
async fn without_a_work_dir_the_image_still_comes_back() {
    let (base, _log) = spawn_stub(Policy::Ok).await;
    let (tool, _slot) = tool_with(ImageProvider::Gemini, &base);
    let ledger = Arc::new(TurnImages::default());
    let output = tool
        .call_with_media(&ctx(None, &ledger), &serde_json::json!({"prompt": "x"}))
        .await
        .unwrap();
    assert!(output.text.contains("作業フォルダが設定されていない"), "{}", output.text);
    assert_eq!(output.images.len(), 1);
    let copies = ledger.take_generated();
    assert_eq!(copies.len(), 1);
    assert_eq!(copies[0].file_name, "generated-1.jpg");
}

/// 同じ秒・同じ番号でも上書きしない（`create_new` で `-2` を足す）。
#[tokio::test]
async fn an_existing_file_is_never_overwritten() {
    let (base, _log) = spawn_stub(Policy::Ok).await;
    let (tool, _slot) = tool_with(ImageProvider::Xai, &base);
    let work = TempDir::new();
    // 先に同じ名前を作っておく（日時は秒単位なので、前後 2 秒ぶんを埋める）。
    let dir = work.0.join("generated_images");
    std::fs::create_dir_all(&dir).unwrap();
    let now = chrono::Local::now();
    for offset in 0..3 {
        let stamp = (now + chrono::Duration::seconds(offset)).format("%Y%m%d-%H%M%S");
        std::fs::write(dir.join(format!("{stamp}-agent_x-1.png")), b"old").unwrap();
    }
    let ledger = Arc::new(TurnImages::default());
    let output = tool
        .call_with_media(&ctx(Some(&work.0), &ledger), &serde_json::json!({"prompt": "x"}))
        .await
        .unwrap();
    assert!(output.text.contains("-agent_x-1-2.png"), "{}", output.text);
    for name in files_in(&dir) {
        if !name.contains("-2.png") {
            assert_eq!(std::fs::read(dir.join(&name)).unwrap(), b"old", "{name} を上書きしない");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// ターン
// ---------------------------------------------------------------------------------------------

/// 筋書き: 1 回目は `image` を呼び、結果を受けたら答える。要求はすべて記録する。
struct Script {
    requests: Mutex<Vec<ChatRequest>>,
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

fn response(text: &str, calls: Vec<ToolCall>, finish: Finish) -> ChatResponse {
    ChatResponse {
        text: Some(text.to_owned()),
        tool_calls: calls,
        finish,
        usage: usage(),
        grounding: Default::default(),
        reasoning_summary: Vec::new(),
        replay: None,
    }
}

#[async_trait::async_trait]
impl LlmBackend for Script {
    fn name(&self) -> &str {
        "image-script"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        self.requests.lock().unwrap().push(req.clone());
        let offered = req.tools.iter().any(|t| t.name == "image");
        let tool_results = req.messages.iter().filter(|m| m.role == Role::Tool).count();
        if !offered || tool_results > 0 || matches!(req.tool_choice, ToolChoice::None) {
            return Ok(response("描きました", Vec::new(), Finish::Stop));
        }
        Ok(response(
            "",
            vec![ToolCall {
                id: "call_0".to_owned(),
                name: "image".to_owned(),
                args: serde_json::json!({"prompt": "苔の地面"}),
                extra: None,
            }],
            Finish::ToolUse,
        ))
    }
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

/// 村を建て、`image` を登録して 1 体へ頼む。`checked` は `enabledTools` に `image` を入れるか。
async fn run_turn(
    slot: Arc<ImageGenSlot>,
    checked: bool,
) -> (Orchestrator, Arc<Script>, AgentId, TempDir) {
    let dir = TempDir::new();
    let backend = Arc::new(Script {
        requests: Mutex::new(Vec::new()),
    });
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(dir.0.join("ws")),
        Arc::new(FixedBackendFactory::new(backend.clone())),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig {
            schedule_interval: Duration::from_secs(3600),
            ..OrchestratorConfig::default()
        },
    )
    .await
    .unwrap();
    orchestrator.set_language(Language::Ja).await.unwrap();
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();
    orchestrator.register_tool(Arc::new(ImageTool::new(slot))).await;
    let agent = AgentId::from("agent_painter");
    let work = dir.0.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let mut spec = AgentSpec::new(agent.clone(), "絵描き", "tpl");
    spec.work_dir = Some(work.display().to_string());
    if checked {
        spec.enabled_tools = Some(vec!["image".to_owned()]);
    }
    orchestrator.create_agent(spec).await.unwrap();
    orchestrator.start_agent(&agent).await.unwrap();
    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&agent, "絵を描いて").await.unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(500)).await;
    (orchestrator, backend, agent, dir)
}

/// 返事の発話に写しが付き、添付の置き場に書かれる。ツール結果にも写しが載る。
#[tokio::test]
async fn the_reply_carries_the_generated_copy() {
    let (base, log) = spawn_stub(Policy::Ok).await;
    let slot = Arc::new(ImageGenSlot::new());
    slot.set(Some(active(ImageProvider::Xai, &base)));
    let (orchestrator, backend, agent, dir) = run_turn(Arc::clone(&slot), true).await;

    assert_eq!(log.lock().unwrap().len(), 1, "接続先へ 1 本");
    let requests = backend.requests.lock().unwrap().clone();
    let results: Vec<_> = requests[1].messages.iter().filter(|m| m.role == Role::Tool).collect();
    assert_eq!(results[0].attachments.len(), 1, "写しがツール結果に載る");

    let messages = orchestrator.message_log(None).await;
    let reply = messages
        .iter()
        .rev()
        .find(|m| matches!(&m.from, Endpoint::Agent { id } if *id == agent))
        .expect("返事の発話");
    assert_eq!(reply.content, "描きました");
    assert_eq!(reply.attachments.len(), 1, "返事の発話に写しを付ける");
    let attachment = &reply.attachments[0];
    assert_eq!(attachment.format, fuseforks_core::attachment::AttachmentFormat::Jpeg);
    assert!(attachment.file_name.contains("-agent_painter-1.png"), "{}", attachment.file_name);
    let stored = dir.0.join("ws").join("attachments").join(format!("{}.jpg", attachment.id));
    assert!(stored.exists(), "写しを添付の置き場へ書く: {}", stored.display());

    // S76-3 — サーヴァントの発話の添付を「入力欄へ」で渡し直せる。
    let id = attachment.id.clone();
    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message_full(&agent, "この絵を見て", &[], Vec::new(), &[], &[id])
        .await
        .expect("サーヴァントの発話の添付を渡し直せる");
    drain_until_quiet(&mut rx, Duration::from_millis(500)).await;
    let requests = backend.requests.lock().unwrap().clone();
    let last_user = requests
        .last()
        .unwrap()
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .unwrap()
        .clone();
    assert_eq!(last_user.attachments.len(), 1);
    assert_eq!(last_user.attachments[0].media_type, PromptMediaType::Jpeg);
}

/// 提示の門は 2 段 — チェックが無い個体にも、設定が無効なら チェックがある個体にも出さない。
#[tokio::test]
async fn the_tool_is_offered_only_through_both_gates() {
    let (base, log) = spawn_stub(Policy::Ok).await;

    let slot = Arc::new(ImageGenSlot::new());
    slot.set(Some(active(ImageProvider::Xai, &base)));
    let (_o1, unchecked, _a1, _d1) = run_turn(Arc::clone(&slot), false).await;
    assert!(
        unchecked.requests.lock().unwrap()[0].tools.iter().all(|t| t.name != "image"),
        "既定集合に入れない（更新しただけで生やさない）"
    );

    let empty = Arc::new(ImageGenSlot::new());
    let (_o2, inactive, _a2, _d2) = run_turn(empty, true).await;
    assert!(
        inactive.requests.lock().unwrap()[0].tools.iter().all(|t| t.name != "image"),
        "設定が無効なら提示しない"
    );
    assert_eq!(log.lock().unwrap().len(), 0, "どちらも外へ 1 本も出ない");
}
