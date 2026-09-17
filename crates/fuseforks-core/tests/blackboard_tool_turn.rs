//! Spec 55 P2 — `blackboard` ツールが**実際のターンの中で**提示され、書き、計器を残すこと。
//!
//! 診断ログの宛先はプロセスで 1 つ（`OnceLock`）なので、`grep_include_log.rs` と同じく
//! **このファイルは 1 テストだけ**にする。
//!
//! 見るのは 3 つ:
//!
//! 1. **`enabled_tools` が明示配列（しかも空）の個体にも提示される。** 既存の村は全個体が
//!    明示配列で、`BUNDLED_TOOL_NAMES` へ入れると誰にも生えない（Spec 18 D13 の穴）。
//!    親切心で表へ足すとここが赤くなる
//! 2. オプトアウトした個体には提示されない（同じ村・同じ作業フォルダで対にして見る）
//! 3. `blackboard op:` の 1 行が残り、**仕事名も本文も載らない**

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fuseforks_core::event::CoreEvent;
use fuseforks_core::llm::{ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, ToolCall, Usage};
use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::{
    BlackboardTool, ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator,
    OrchestratorConfig,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-bbturn-{tag}-{}",
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

const TASK: &str = "秘密の仕事名";
const BODY: &str = "秘密の本文";

/// 提示されたツール名を書き留め、`blackboard` があれば同じ `write` を 2 回呼んでから本文で終える。
#[derive(Default)]
struct WritesTwice {
    offered: Mutex<Vec<Vec<String>>>,
}

#[async_trait::async_trait]
impl LlmBackend for WritesTwice {
    fn name(&self) -> &str {
        "writes-twice"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let names: Vec<String> = req.tools.iter().map(|t| t.name.clone()).collect();
        let has_board = names.iter().any(|n| n == "blackboard");
        let round = {
            let mut offered = self.offered.lock().unwrap();
            offered.push(names);
            offered.len()
        };
        // 2 体を順に走らせる。個体ごとの周は、ツール結果の数で数える。
        let results = req
            .messages
            .iter()
            .filter(|m| matches!(m.role, fuseforks_core::llm::Role::Tool))
            .count();
        let call_tool = has_board && results < 2;
        Ok(ChatResponse {
            text: Some(if call_tool { String::new() } else { format!("終わりました {round}") }),
            tool_calls: if call_tool {
                vec![ToolCall {
                    id: format!("call_{results}"),
                    name: "blackboard".into(),
                    args: serde_json::json!({ "op": "write", "name": TASK, "body": BODY }),
                    extra: None,
                }]
            } else {
                Vec::new()
            },
            finish: Finish::Stop,
            usage: Usage { prompt: 1, completion: 1, cache_read: 0, cache_write: 0, cache_write_1h: 0, reasoning: 0 },
            grounding: Default::default(),
            reasoning_summary: Vec::new(),
        })
    }
}

async fn one_turn(orchestrator: &Orchestrator, id: &str) {
    let mut rx = orchestrator.subscribe();
    let id = AgentId::from(id);
    orchestrator.start_agent(&id).await.unwrap();
    orchestrator.send_user_message(&id, "付箋を書いて").await.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let event = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("返信が出ること")
            .expect("受信できること");
        if let CoreEvent::MessageSent { message } = event
            && message.content.contains("終わりました")
        {
            break;
        }
    }
}

#[tokio::test]
async fn the_tool_is_offered_outside_enabled_tools_and_leaves_one_line_per_op() {
    let store_dir = TempDir::new("store");
    let work_dir = TempDir::new("work");
    let log = store_dir.0.join("fuseforks.log");
    fuseforks_core::open_log(&log).expect("開けること");

    let backend = Arc::new(WritesTwice::default());
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&store_dir.0),
        Arc::new(FixedBackendFactory::new(backend.clone())),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig::default(),
    )
    .await
    .expect("bootstrap できること");
    orchestrator.set_language(fuseforks_core::world::Language::Ja).await.unwrap();
    orchestrator.upsert_template(ModelTemplate::new("tpl", "既定", "mock-model")).await.unwrap();
    orchestrator.register_tool(Arc::new(BlackboardTool)).await;

    // 既存の村の形: `enabled_tools` は明示配列。ここでは空 = 同梱ツールを 1 本も選んでいない。
    let mut zari = AgentSpec::new("agent", "ザリ", "tpl");
    zari.enabled_tools = Some(Vec::new());
    zari.work_dir = Some(work_dir.0.display().to_string());
    orchestrator.create_agent(zari).await.unwrap();

    let mut opted_out = AgentSpec::new("agent_2", "ジェミー", "tpl");
    opted_out.enabled_tools = Some(Vec::new());
    opted_out.work_dir = Some(work_dir.0.display().to_string());
    opted_out.uses_blackboard = false;
    orchestrator.create_agent(opted_out).await.unwrap();

    one_turn(&orchestrator, "agent").await;
    let after_first = backend.offered.lock().unwrap().len();
    one_turn(&orchestrator, "agent_2").await;

    let offered = backend.offered.lock().unwrap().clone();
    assert!(
        offered[..after_first].iter().all(|names| names.iter().any(|n| n == "blackboard")),
        "明示配列の個体にも提示される: {offered:?}"
    );
    assert!(
        offered[after_first..].iter().all(|names| !names.iter().any(|n| n == "blackboard")),
        "オプトアウトした個体には提示されない: {offered:?}"
    );

    // ファイル名と置き場はツールが組んだ。2 回目の write は 1 バイトも変えていない。
    let note = work_dir.0.join("blackboard").join("doing").join(format!("agent - {TASK}.md"));
    assert_eq!(std::fs::read_to_string(&note).expect("付箋ができていること"), BODY);

    let body = std::fs::read_to_string(&log).expect("読めること");
    let lines: Vec<&str> = body.lines().filter(|l| l.contains("blackboard op:")).collect();
    assert_eq!(lines.len(), 2, "op 1 回につき 1 行: {body}");
    assert!(lines[0].contains("agent=agent op=write state=doing outcome=ok"), "{}", lines[0]);
    assert!(lines[1].contains("agent=agent op=write state=doing outcome=exists"), "{}", lines[1]);
    assert!(
        lines.iter().all(|l| !l.contains(TASK) && !l.contains(BODY)),
        "仕事名と本文は計器へ出さない: {lines:?}"
    );
}
