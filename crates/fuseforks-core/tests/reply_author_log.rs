//! `reply:` 行の `author=`（計器。2026-10-10）。
//!
//! **起点は `refusal=` の検証**（2026-10-10）— 返信 1,065 本を `sessions.redb` の
//! 本文と突き合わせたら、**コアが本人に代わって書いた失敗の文面が 12 本**、
//! 普通の返信と同じ `reply: … refusal=no` で配送されていた（ツール実行の上限 /
//! 本文が空）。完遂の軸で唯一、本文を読まずに数えられる未完遂なのに、
//! ログからは見分けが付かなかった。
//!
//! ここで留めるのは**対** — モデルが本文を返した回は `author=model`、
//! 本文が空でコアが置き換えた回は `author=core`。片方だけでは
//! 「常に model と書く実装」と区別が付かない。
//!
//! **診断の出口はプロセスで 1 つ**（`OnceLock`）なので、ログを読むテストは
//! このファイルに 1 つだけ（`tests/diag.rs` と同じ制約）。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fuseforks_core::llm::{ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, Usage};
use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-replyauthor-{tag}-{}",
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

/// 依頼文に「黙って」があれば本文を空で返し、それ以外は本文を返す。
struct SilentOnCue;

#[async_trait::async_trait]
impl LlmBackend for SilentOnCue {
    fn name(&self) -> &str {
        "silent-on-cue"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let silent = req.messages.iter().any(|m| m.content.contains("黙って"));
        Ok(ChatResponse {
            text: if silent { None } else { Some("答えです".into()) },
            tool_calls: Vec::new(),
            finish: Finish::Stop,
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
        })
    }
}

#[tokio::test]
async fn a_reply_written_by_the_core_is_marked_as_such() {
    let dir = TempDir::new("author");
    let log_path = dir.0.join("fuseforks.log");
    fuseforks_core::open_log(&log_path).expect("ログを開けること");

    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(Arc::new(SilentOnCue))),
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

    let speaker = AgentId::from("agent_01");
    let silent = AgentId::from("agent_02");
    for (id, name) in [(&speaker, "話す"), (&silent, "黙る")] {
        orchestrator
            .create_agent(AgentSpec::new(id.clone(), name, "tpl"))
            .await
            .unwrap();
        orchestrator.start_agent(id).await.unwrap();
    }

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&speaker, "調べて").await.unwrap();
    orchestrator.send_user_message(&silent, "黙っていて").await.unwrap();
    while tokio::time::timeout(Duration::from_millis(400), rx.recv())
        .await
        .is_ok()
    {}

    let body = std::fs::read_to_string(&log_path).expect("ログが読めること");
    let reply_of = |agent: &str| {
        body.lines()
            .find(|line| line.contains(&format!("reply: agent={agent} ")))
            .unwrap_or_else(|| panic!("{agent} の reply: 行があること:\n{body}"))
            .to_owned()
    };

    let spoken = reply_of("agent_01");
    assert!(
        spoken.ends_with(" author=model"),
        "モデルが本文を返した回は author=model:\n{spoken}"
    );
    let replaced = reply_of("agent_02");
    assert!(
        replaced.ends_with(" author=core"),
        "本文が空でコアが置き換えた回は author=core:\n{replaced}"
    );
}
