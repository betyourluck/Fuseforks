//! Spec 55 P3 — `blackboard/` の囲いと、個体の削除の掃除が**計器に 1 行ずつ残る**こと。
//!
//! 診断ログの宛先はプロセスで 1 つ（`OnceLock`）なので、**このファイルは 1 テストだけ**。
//!
//! - `blackboard fence:` — 断った書き込みだけが 1 行。通した読み取りでは増えない。
//!   **パスは出さない**（付箋のファイル名には仕事名が入る）
//! - `blackboard sweep:` — `delete_agent` が、消す個体の現在の作業フォルダから
//!   その id の付箋をごみ箱へ送る。**id は再利用される**ので、残すと同じ id の
//!   新しい個体が引き継いで書けてしまう

use std::path::PathBuf;
use std::sync::Arc;

use fuseforks_core::llm::{ChatRequest, ChatResponse, LlmBackend, LlmError};
use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::tool::{AgentTool, ToolContext};
use fuseforks_core::{
    ConfigStore, FileTool, FixedBackendFactory, InMemorySecretStore, Orchestrator,
    OrchestratorConfig, SdTool,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-bbfence-{tag}-{}",
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

/// 呼ばれないバックエンド（このテストはターンを走らせない）。
struct Unused;

#[async_trait::async_trait]
impl LlmBackend for Unused {
    fn name(&self) -> &str {
        "unused"
    }
    async fn chat(&self, _req: ChatRequest) -> Result<ChatResponse, LlmError> {
        unreachable!("ターンは走らない")
    }
}

const TASK: &str = "秘密の仕事名";

#[tokio::test]
async fn refused_writes_and_the_delete_sweep_each_leave_one_line() {
    let store_dir = TempDir::new("store");
    let work = TempDir::new("work");
    let log = store_dir.0.join("fuseforks.log");
    fuseforks_core::open_log(&log).expect("開けること");

    let board = work.0.join("blackboard");
    for (state, owner) in [("doing", "agent"), ("on-hold", "agent"), ("done", "agent"), ("doing", "agent_2")] {
        std::fs::create_dir_all(board.join(state)).unwrap();
        std::fs::write(board.join(state).join(format!("{owner} - {TASK}.md")), "x").unwrap();
    }

    // ---- 囲い ----
    let ctx = ToolContext {
        agent_id: AgentId::from("agent"),
        work_dir: Some(work.0.clone()),
        cancel: None,
        rag_roots: Vec::new(),
        agent_names: Vec::new(),
        uses_blackboard: true,
        language: fuseforks_core::world::Language::Ja,
    };
    let note = format!("blackboard/doing/agent - {TASK}.md");
    let refused = FileTool
        .call(&ctx, &serde_json::json!({ "op": "append", "path": note, "content": "横から" }))
        .await
        .unwrap();
    assert!(refused.contains("`blackboard` ツール"), "{refused}");
    let read = FileTool.call(&ctx, &serde_json::json!({ "op": "read", "path": note })).await.unwrap();
    assert!(read.ends_with('x'), "読み取りは通る: {read}");
    let sd = SdTool
        .call(&ctx, &serde_json::json!({ "path": note, "pattern": "x", "replacement": "y", "apply": true }))
        .await
        .unwrap();
    assert!(sd.contains("`blackboard` ツール"), "{sd}");

    // ---- 個体の削除の掃除 ----
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&store_dir.0),
        Arc::new(FixedBackendFactory::new(Arc::new(Unused))),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig::default(),
    )
    .await
    .expect("bootstrap できること");
    orchestrator.upsert_template(ModelTemplate::new("tpl", "既定", "mock-model")).await.unwrap();
    for (id, name) in [("agent", "ザリ"), ("agent_2", "ジェミー")] {
        let mut spec = AgentSpec::new(id, name, "tpl");
        spec.work_dir = Some(work.0.display().to_string());
        orchestrator.create_agent(spec).await.unwrap();
    }
    orchestrator.delete_agent(&AgentId::from("agent")).await.unwrap();

    let body = std::fs::read_to_string(&log).expect("読めること");
    let sweep: Vec<&str> = body.lines().filter(|l| l.contains("blackboard sweep:")).collect();
    assert_eq!(sweep.len(), 1, "{body}");
    if sweep[0].contains("failed=0") {
        assert!(sweep[0].contains("agent=agent removed=3 failed=0"), "{}", sweep[0]);
        for state in ["doing", "on-hold", "done"] {
            assert!(!board.join(state).join(format!("agent - {TASK}.md")).exists(), "{state}");
        }
    }
    assert!(
        board.join("doing").join(format!("agent_2 - {TASK}.md")).exists(),
        "他の個体の付箋は残る"
    );

    let fence: Vec<&str> = body.lines().filter(|l| l.contains("blackboard fence:")).collect();
    assert_eq!(fence.len(), 2, "断った 2 本だけ。読み取りでは増えない: {body}");
    assert!(fence[0].contains("agent=agent tool=file op=append"), "{}", fence[0]);
    assert!(fence[1].contains("agent=agent tool=sd op=apply"), "{}", fence[1]);
    assert!(
        body.lines().filter(|l| l.contains("blackboard ")).all(|l| !l.contains(TASK)),
        "仕事名（= パス）は計器へ出さない: {body}"
    );
}
