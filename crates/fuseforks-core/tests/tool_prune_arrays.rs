//! 配列型（Spec 60）の配線を偽の採点器で丸ごと通す（P2）。
//!
//! `tool_prune.rs` が留めているのは段落型の経路。配列型で**新しく増えた配線**は 2 つで、
//! どちらも 2 周またがないと出ない:
//!
//! 1. **落とした配列ごとに `P{n}` が振られ、`omitted` の `id` がその配列を指す**（D3 / D5）。
//!    `PrunedRaw` は中身が段落か要素かを知らないので、`turn.rs` が `Pruned.entries` を
//!    そのまま積んでいるかはここでしか見えない
//! 2. **`omitted` の `from` / `to` は配列の中で 0 始まり**（平坦化した index ではない）。
//!    2 つ目の配列の要素 1 を `P2, 1` で引いて、1 つ目の配列の要素が返らないことを見る
//!
//! モデルへ届く本文が **JSON として読めるまま**であることも、経路を通した実物で確かめる。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use fuseforks_core::llm::{
    ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, Role, ToolCall, Usage,
};
use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::prune::{ParagraphScorer, ScoreError, ScoreReport, PRUNED_KEY};
use fuseforks_core::tool::{AgentTool, ToolContext};
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};

struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-prune-arrays-{tag}-{}",
            std::process::id()
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

fn elem(tag: &str, i: usize) -> String {
    format!(r#"{{"tag":"{tag}","i":{i},"text":"{}"}}"#, "あ".repeat(500))
}

/// `{"title":"t","posts":[8 要素],"tail":1}`（整形あり）。
fn one_array_doc() -> String {
    let posts: Vec<String> = (0..8).map(|i| elem("p", i)).collect();
    format!(
        "{{\n  \"title\": \"t\",\n  \"posts\": [\n    {}\n  ],\n  \"tail\": 1\n}}",
        posts.join(",\n    ")
    )
}

/// `{"x":[4 要素],"y":[4 要素]}`（素）。
fn two_arrays_doc() -> String {
    let x: Vec<String> = (0..4).map(|i| elem("x", i)).collect();
    let y: Vec<String> = (0..4).map(|i| elem("y", i)).collect();
    format!(r#"{{"x":[{}],"y":[{}]}}"#, x.join(","), y.join(","))
}

/// 配列型の本文を返すツール（`prunable` が真）。
struct ArrayTool {
    body: String,
}

#[async_trait::async_trait]
impl AgentTool for ArrayTool {
    fn name(&self) -> &str {
        "array_probe"
    }
    fn description(&self, _l: fuseforks_core::world::Language) -> String {
        "テスト用。配列型の JSON を返す".into()
    }
    fn parameters(&self, _l: fuseforks_core::world::Language) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    fn prunable(&self) -> bool {
        true
    }
    async fn call(
        &self,
        _ctx: &ToolContext,
        _args: &serde_json::Value,
    ) -> fuseforks_core::CoreResult<String> {
        Ok(self.body.clone())
    }
}

/// 偽の採点器。**偶数番目だけ残す**（平坦化した並びで数える）。
struct KeepEven {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl ParagraphScorer for KeepEven {
    async fn score(&self, _basis: &str, paragraphs: &[&str]) -> Result<ScoreReport, ScoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ScoreReport {
            scores: (0..paragraphs.len())
                .map(|i| Some(if i % 2 == 0 { 0.9 } else { 0.01 }))
                .collect(),
            calls: 1,
            tokens: 1,
        })
    }
    fn threshold(&self) -> f32 {
        0.2
    }
}

/// 1 周目で `array_probe` を呼び、2 周目で `omitted` を呼び、3 周目で答える。
struct CallThenOmit {
    omit: serde_json::Value,
    seen: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl LlmBackend for CallThenOmit {
    fn name(&self) -> &str {
        "call-then-omit"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        if let Some(m) = req.messages.iter().rev().find(|m| m.role == Role::Tool) {
            self.seen.lock().unwrap().push(m.content.clone());
        }
        let usage = Usage {
            prompt: 10,
            completion: 5,
            cache_read: 0,
            cache_write: 0,
            cache_write_1h: 0,
            reasoning: 0,
        };
        let results = req.messages.iter().filter(|m| m.role == Role::Tool).count();
        let call = |id: &str, name: &str, args: serde_json::Value| ChatResponse {
            text: Some(String::new()),
            tool_calls: vec![ToolCall {
                id: id.into(),
                name: name.into(),
                args,
                extra: None,
            }],
            finish: Finish::ToolUse,
            usage,
            grounding: Default::default(),
            reasoning_summary: Vec::new(),
        };
        match results {
            0 => Ok(call("c1", "array_probe", serde_json::json!({}))),
            1 => Ok(call("c2", "omitted", self.omit.clone())),
            _ => Ok(ChatResponse {
                text: Some("終わりました".into()),
                tool_calls: Vec::new(),
                finish: Finish::Stop,
                usage,
                grounding: Default::default(),
                reasoning_summary: Vec::new(),
            }),
        }
    }
}

async fn run(tag: &str, body: String, omit: serde_json::Value) -> (Vec<String>, usize) {
    let dir = TempDir::new(tag);
    let backend = Arc::new(CallThenOmit {
        omit,
        seen: std::sync::Mutex::new(Vec::new()),
    });
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(backend.clone())),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig {
            schedule_interval: Duration::from_secs(3600),
            ..OrchestratorConfig::default()
        },
    )
    .await
    .expect("bootstrap できること");
    orchestrator
        .set_language(fuseforks_core::world::Language::Ja)
        .await
        .unwrap();
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();
    let id = AgentId::from("agent_1");
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "調べ役", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();
    orchestrator.register_tool(Arc::new(ArrayTool { body })).await;
    let scorer = Arc::new(KeepEven { calls: AtomicUsize::new(0) });
    orchestrator.set_paragraph_scorer(Some(scorer.clone())).await;

    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&id, "この依頼は 20 字以上あるので基準として通ります")
        .await
        .unwrap();
    drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    let seen = backend.seen.lock().unwrap().clone();
    (seen, scorer.calls.load(Ordering::SeqCst))
}

/// 配列型が圧縮され、本文は JSON のまま。`omitted` は**次の周**で要素を逐語で返す。
#[tokio::test]
async fn an_array_body_is_pruned_and_omitted_reads_an_element_back_next_round() {
    let (seen, calls) = run(
        "one",
        one_array_doc(),
        serde_json::json!({ "id": "P1", "from": 3, "to": 3 }),
    )
    .await;
    assert_eq!(calls, 1, "採点は 1 回（8 要素は 1 束ね）");
    assert!(seen.len() >= 2, "ツール結果が 2 回モデルへ渡ること: {}", seen.len());

    // 1 通目 = 圧縮された本文。JSON として読めるまま。
    let v: serde_json::Value =
        serde_json::from_str(&seen[0]).unwrap_or_else(|e| panic!("JSON のまま: {e}\n{}", seen[0]));
    let posts = v["posts"].as_array().expect("posts が配列のまま");
    let kept: Vec<u64> = posts.iter().map(|p| p["i"].as_u64().unwrap()).collect();
    assert_eq!(kept, vec![0, 2, 4, 6], "偶数番目が残る");
    assert_eq!(v["title"], "t");
    assert_eq!(v["tail"], 1);
    assert_eq!(v[PRUNED_KEY]["arrays"]["posts"]["id"], "P1");
    assert_eq!(v[PRUNED_KEY]["arrays"]["posts"]["total"], 8);
    assert_eq!(v[PRUNED_KEY]["arrays"]["posts"]["dropped"], serde_json::json!([1, 3, 5, 7]));
    assert!(seen[0].contains(&elem("p", 2)), "残った要素は逐語");

    // 2 通目 = `omitted` が返した要素 3 の逐語。**次の周で読めることが要点。**
    let back = &seen[1];
    assert!(back.contains(&elem("p", 3)), "落とした要素 3 が逐語で返る: {back}");
    assert!(!back.contains(&elem("p", 2)), "保持した要素は返さない");
}

/// **落とした配列ごとに別の id。`omitted` の index はその配列の中で数える。**
#[tokio::test]
async fn each_pruned_array_has_its_own_id_and_omitted_indexes_within_the_array() {
    let (seen, _) = run(
        "two",
        two_arrays_doc(),
        serde_json::json!({ "id": "P2", "from": 1, "to": 1 }),
    )
    .await;
    assert!(seen.len() >= 2, "{}", seen.len());

    let v: serde_json::Value = serde_json::from_str(&seen[0]).unwrap();
    // 平坦化で x = 0..4、y = 4..8。偶数番目が残るので両方の配列で 2 つずつ落ちる。
    assert_eq!(v["x"].as_array().unwrap().len(), 2);
    assert_eq!(v["y"].as_array().unwrap().len(), 2);
    assert_eq!(v[PRUNED_KEY]["arrays"]["x"]["id"], "P1");
    assert_eq!(v[PRUNED_KEY]["arrays"]["y"]["id"], "P2");
    assert_eq!(
        v[PRUNED_KEY]["arrays"]["y"]["dropped"],
        serde_json::json!([1, 3]),
        "y の落とした index は y の中で 0 始まり（平坦化の 5, 7 ではない）"
    );

    // `P2, 1` は **y の要素 1**。x の要素 1 でも、平坦化の 1 番目（= x の要素 1）でもない。
    let back = &seen[1];
    assert!(back.contains(&elem("y", 1)), "y の要素 1 が返る: {back}");
    assert!(!back.contains(&elem("x", 1)), "x の要素 1 は返らない");
}

/// 静穏窓まで待つ（`tool_prune.rs` と同じ。統計イベントの周期 1 秒より短く）。
async fn drain_until_quiet(
    rx: &mut tokio::sync::broadcast::Receiver<fuseforks_core::CoreEvent>,
    quiet: Duration,
) {
    loop {
        match tokio::time::timeout(quiet, rx.recv()).await {
            Ok(Ok(_)) => continue,
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            _ => return,
        }
    }
}
