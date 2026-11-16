//! 提供元が判定で止めた応答を、止めたと読む（2026-11-16）。
//!
//! # 何が起きていたか
//!
//! 画像を添えた依頼に Gemini が何も返さず（出力 0 トークン）、会話には「モデルから本文が返りませんでした」とだけ
//! 出た。**同じ画像と文面が 1 分後には通った**ので、入力の審査で止まったのか一時的な空なのかを確かめたかったが、
//! Gemini の受け取り側は `promptFeedback.blockReason`（入力で止めた理由）も安全系の `finishReason` も読まずに
//! 捨てており、ログにも何も残っていなかった（`failures.md` #72 と同じ — 握り潰した事実を数えない）。
//!
//! # 留めること（2 層）
//!
//! 1. **本物の受け取り側**: ループバックの HTTP スタブが Gemini の形で「入力で止めた」応答（候補 0 件 +
//!    `blockReason`）を返す → `Finish::Blocked(Input)` と、ログの `gemini empty:` 行に生の理由の語
//! 2. **会話に出る文**: 止められた個体と、理由の無い空を返す個体の**対** — 前者は「提供元が入力を止めた」、
//!    後者は今までどおり「本文が返りませんでした」。片方だけでは「常に止めたと書く実装」と区別が付かない
//!
//! **診断の出口はプロセスで 1 つ**（`OnceLock`）なので、ログを開くのはこのファイルの 1 本だけ。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fuseforks_core::llm::{
    BlockStage, ChatMessage, ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, Usage,
};
use fuseforks_core::model::{AgentId, AgentSpec, CredentialSource, ModelTemplate};
use fuseforks_core::{
    ConfigStore, CoreEvent, FixedBackendFactory, HttpLlmBackend, InMemorySecretStore, LlmConfig,
    Orchestrator, OrchestratorConfig, Provider,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "fuseforks-geminiblocked-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
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

/// 入力の段で止めた Gemini の応答（候補 0 件・理由は `promptFeedback` だけ。払いは入力ぶん）。
const BLOCKED: &str = r#"{"promptFeedback":{"blockReason":"PROHIBITED_CONTENT"},"usageMetadata":{"promptTokenCount":1290,"totalTokenCount":1290},"modelVersion":"stub"}"#;

/// 200 と `BLOCKED` を返すだけの最小の HTTP/1.1 スタブ（`tests/error_body_truncation.rs` と同じ形）。
async fn spawn_stub() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut raw = Vec::new();
                let mut buf = [0u8; 8192];
                loop {
                    let Ok(n) = socket.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&raw);
                    let Some(head_end) = text.find("\r\n\r\n") else {
                        continue;
                    };
                    let len: usize = text[..head_end]
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse().ok())?
                        })
                        .unwrap_or(0);
                    if raw.len() >= head_end + 4 + len {
                        break;
                    }
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{BLOCKED}",
                    BLOCKED.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });

    format!("http://127.0.0.1:{port}/v1beta")
}

/// 1. 本物の受け取り側が入力で止めた形を読み、理由の語をログに残す。
#[tokio::test]
async fn a_blocked_prompt_from_gemini_is_read_and_logged() {
    let dir = TempDir::new("log");
    let log_path = dir.0.join("fuseforks.log");
    fuseforks_core::open_log(&log_path).expect("ログを開けること");

    let mut template = ModelTemplate::new("tpl_stub", "スタブ", "stub-model");
    template.base_url = spawn_stub().await;
    template.credential = CredentialSource::NotRequired;
    template.provider = Some(Provider::Gemini);
    template.max_retries = 1;
    let config = LlmConfig::from_template(&template, &InMemorySecretStore::new()).unwrap();
    let backend = HttpLlmBackend::new(config).unwrap();

    let response = backend
        .chat(ChatRequest::plain("stub-model", vec![ChatMessage::user("この画像を")], 16))
        .await
        .expect("200 は応答として返るはず（失敗ではない）");
    assert_eq!(response.finish, Finish::Blocked(BlockStage::Input));
    assert_eq!(response.text, None);
    assert_eq!(response.usage.prompt, 1290, "止められても入力の払いは記録する");

    let body = std::fs::read_to_string(&log_path).expect("ログが読めること");
    let line = body
        .lines()
        .find(|line| line.contains("gemini empty: "))
        .unwrap_or_else(|| panic!("gemini empty: の行があること:\n{body}"));
    assert!(
        line.ends_with("gemini empty: candidates=0 block_reason=PROHIBITED_CONTENT finish=- output_tokens=0"),
        "生の理由の語を書く:\n{line}"
    );
}

/// 依頼文に「止めて」があれば入力で止めた形、それ以外は理由の無い空を返す。
struct BlockedOnCue;

#[async_trait::async_trait]
impl LlmBackend for BlockedOnCue {
    fn name(&self) -> &str {
        "blocked-on-cue"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let blocked = req.messages.iter().any(|m| m.content.contains("止めて"));
        Ok(ChatResponse {
            text: None,
            tool_calls: Vec::new(),
            finish: if blocked { Finish::Blocked(BlockStage::Input) } else { Finish::Other },
            usage: Usage {
                prompt: 1,
                completion: 0,
                cache_read: 0,
                cache_write: 0,
                cache_write_1h: 0,
                reasoning: 0,
            },
            grounding: Default::default(),
            reasoning_summary: Vec::new(),
            replay: None,
        })
    }
}

/// 2. 止められた個体と理由の無い空の個体の対で、会話に出る文が書き分けられる。
#[tokio::test]
async fn a_blocked_turn_says_the_provider_stopped_it() {
    let dir = TempDir::new("reply");
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(Arc::new(BlockedOnCue))),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig {
            schedule_interval: Duration::from_secs(3600),
            ..OrchestratorConfig::default()
        },
    )
    .await
    .expect("bootstrap できること");
    orchestrator.set_language(fuseforks_core::world::Language::Ja).await.unwrap();
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();

    let blocked = AgentId::from("agent_01");
    let empty = AgentId::from("agent_02");
    for (id, name) in [(&blocked, "止まる"), (&empty, "空")] {
        orchestrator
            .create_agent(AgentSpec::new(id.clone(), name, "tpl"))
            .await
            .unwrap();
        orchestrator.start_agent(id).await.unwrap();
    }

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&blocked, "止めてほしい依頼").await.unwrap();
    orchestrator.send_user_message(&empty, "ふつうの依頼").await.unwrap();

    let mut replies = std::collections::HashMap::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while replies.len() < 2 {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!remaining.is_zero(), "2 体の返事が 20 秒で揃わない: {replies:?}");
        if let Ok(Ok(CoreEvent::MessageSent { message })) = tokio::time::timeout(remaining, rx.recv()).await
            && let fuseforks_core::model::Endpoint::Agent { id } = &message.from
        {
            replies.insert(id.clone(), message.content.clone());
        }
    }

    assert_eq!(
        replies[&blocked],
        "（モデルの提供元が、入力（依頼の文面や添付）を安全や利用方針の判定で止めたため、応答が返りませんでした。添付や頼み方を変えてもう一度頼んでみてください。）"
    );
    assert_eq!(replies[&empty], "（モデルから本文が返りませんでした。もう一度頼んでみてください。）");
}
