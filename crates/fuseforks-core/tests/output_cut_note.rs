//! 出力の上限で切れた本文に、切れたことを書く（2026-11-28）。
//!
//! # 何が起きていたか
//!
//! ウォーレン（`google/gemini-4-argon`・Perplexity 経由・最大出力 16,000）が、答えの終わりで止まらず同じ記号の
//! 並びを上限まで出した。受け取り側は `incomplete` を `Finish::Length` に読んでいたのに、ターンの側は本文が空の
//! ときしか終わり方を見ておらず、切れた本文が `stop=-` の普通の答えとして配られた。会話ペインにも `turn:` 行にも
//! 切れた印は無く、気づく手掛かりは `total − prompt` が最大出力と同じことだけだった。
//!
//! # 留めること
//!
//! 切れた個体と切れていない個体の**対** — 前者は本文の後ろに注記が付き、ログに `output cut:` が 1 行出る。
//! 後者は本文がそのままで、行も出ない。片方だけでは「常に付ける実装」と区別が付かない（`failures.md` #90）。
//!
//! **診断の出口はプロセスで 1 つ**（`OnceLock`）なので、ログを開くのはこのファイルの 1 本だけ。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fuseforks_core::llm::{ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, Usage};
use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::{
    ConfigStore, CoreEvent, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!("fuseforks-outputcut-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// テンプレートの最大出力。注記とログに出る数がここから来ることを見る。
const LIMIT: u32 = 1234;

/// 依頼文に「長く」があれば上限で切れた形（本文あり・`Length`）、それ以外は普通に終わる。
struct CutOnCue;

#[async_trait::async_trait]
impl LlmBackend for CutOnCue {
    fn name(&self) -> &str {
        "cut-on-cue"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let cut = req.messages.iter().any(|m| m.content.contains("長く"));
        Ok(ChatResponse {
            text: Some(if cut { "途中まで書いた答え".into() } else { "終わりまで書いた答え".into() }),
            tool_calls: Vec::new(),
            finish: if cut { Finish::Length } else { Finish::Stop },
            usage: Usage {
                prompt: 10,
                completion: if cut { u64::from(LIMIT) } else { 7 },
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

#[tokio::test]
async fn a_reply_cut_at_the_output_limit_says_so() {
    let dir = TempDir::new("reply");
    let log_path = dir.0.join("fuseforks.log");
    fuseforks_core::diag::open_log(&log_path).expect("ログを開けること");

    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(Arc::new(CutOnCue))),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig {
            schedule_interval: Duration::from_secs(3600),
            ..OrchestratorConfig::default()
        },
    )
    .await
    .expect("bootstrap できること");
    orchestrator.set_language(fuseforks_core::world::Language::Ja).await.unwrap();
    let mut template = ModelTemplate::new("tpl", "既定", "mock-model");
    template.max_output_tokens = LIMIT;
    orchestrator.upsert_template(template).await.unwrap();

    let cut = AgentId::from("agent_01");
    let whole = AgentId::from("agent_02");
    for (id, name) in [(&cut, "切れる"), (&whole, "終わる")] {
        orchestrator
            .create_agent(AgentSpec::new(id.clone(), name, "tpl"))
            .await
            .unwrap();
        orchestrator.start_agent(id).await.unwrap();
    }

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&cut, "長く書いてほしい依頼").await.unwrap();
    orchestrator.send_user_message(&whole, "ふつうの依頼").await.unwrap();

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
        replies[&cut],
        "途中まで書いた答え\n\n（この返事は出力の上限 1234 トークンで途中で切れました。\
         続きが要るなら、依頼を小さく分けて頼み直してください。\
         終わりのほうで同じ文字が繰り返されているなら、モデルが止まれなくなった形で、\
         上限を上げても直りません。）",
        "本文は残したまま、末尾に注記を足す"
    );
    assert_eq!(replies[&whole], "終わりまで書いた答え", "切れていない答えには何も足さない");

    let body = std::fs::read_to_string(&log_path).expect("ログが読めること");
    let lines: Vec<&str> = body.lines().filter(|line| line.contains("output cut: ")).collect();
    assert_eq!(lines.len(), 1, "切れた 1 ターンぶんだけ出る:\n{body}");
    assert!(
        lines[0].ends_with("output cut: agent=agent_01 limit=1234 completion=1234 chars=9"),
        "上限・出力トークン・注記を足す前の本文の字数を書く:\n{}",
        lines[0]
    );
}
