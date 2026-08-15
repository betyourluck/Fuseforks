//! **失敗したターンの払いが、成功と同じ 3 箇所へ入る**ことを機械で留める
//! （`failures.md` #103 の処方 — 飛行中台帳）。
//!
//! 出力上限で本文が空になったターン（`LLM_OUTPUT_TRUNCATED`）は、以前は
//! `turn.rs` の `backend.chat(...).await?` で即座に伝播し、**課金は起きているのに
//! カードの累計・予算・`turn:` 行のどれにも出なかった**。唯一の痕跡が
//! `reject_empty_reasoning` の `truncated:` 行で、それは `tests/truncated_note.rs`
//! が留めている。ここで見るのはその先 — **`Err` に乗った usage が台帳へ入るか**。
//!
//! 3 箇所を 1 本のテストで対にして見る:
//!
//! 1. **カードの累計**（`snapshot().total_tokens`）— 失敗した個体の数字が動く
//! 2. **`turn:` 行** — `stop=failed:LLM_OUTPUT_TRUNCATED` で成功と同じ欄が出る
//! 3. **予算** — 切れた応答の払いが財布から引かれ、**同じ因果の次の呼び出しが
//!    予約できずに止まる**（`budget stop:` + System 行「予算」）
//!
//! 3 は**予約が Drop で全額返金されていた旧実装では起きない** — 返金されれば
//! 進行役の 2 周目は通り、ユーザーへ「結果: 相手から答えが返りませんでした」が
//! 返る。だから**進行役がユーザーへ返さないこと**が負の対照になる（#90 の処方）。
//!
//! 数字の設計（実効 = 未キャッシュ×1 + 出力×4、床 1,000）:
//!
//! ```text
//! 天井 3,000
//! 進行役 1 周目: 予約 1,000（床）→ 実測 1+1 = 5 → 残 2,995
//! ワーカー:      予約 1,000（床）→ 切れた応答 prompt 2,000 + completion 125 = 2,500
//!                旧: Drop で返金 → 残 2,995 / 新: commit → 残 495
//! 進行役 2 周目: 予約 1,000（床）→ 新: 495 では確保できず reserve_short で打ち切り
//! ```
//!
//! **診断の出口はプロセスで 1 つ**（`OnceLock`）なので、ログを読むテストは
//! このファイルに 1 つだけ。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fuseforks_core::event::CoreEvent;
use fuseforks_core::llm::{
    ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, Role, ToolCall, Usage,
};
use fuseforks_core::model::{AgentId, AgentSpec, Endpoint, ModelTemplate};
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-failed-settle-{}",
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

/// ワーカーが払った量。**予約の床（1,000 実効）より大きく**しておかないと、
/// 予約の commit と Drop の差が残額に出ない（床の内側で収まると commit は返金側）。
const WORKER_USAGE: Usage = Usage {
    prompt: 2_000,
    completion: 125,
    cache_read: 0,
    reasoning: 125,
};

/// 進行役は 1 度だけ委譲し、結果が届いたら引用して終える。ワーカーは
/// **払ったうえで出力上限に当たる**（`OutputTruncated` に usage を載せて返す）。
///
/// 役の判別は提示ツール（`ask_*` を持つ側が進行役）。
struct AskThenTruncatedWorker;

#[async_trait::async_trait]
impl LlmBackend for AskThenTruncatedWorker {
    fn name(&self) -> &str {
        "ask-then-truncated"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let tiny = Usage {
            prompt: 1,
            completion: 1,
            cache_read: 0,
            reasoning: 0,
        };
        if let Some(ask) = req.tools.iter().find(|t| t.name.starts_with("ask_")) {
            if let Some(result) = req.messages.iter().rev().find(|m| m.role == Role::Tool) {
                return Ok(ChatResponse {
                    text: Some(format!("結果: {}", result.content)),
                    tool_calls: Vec::new(),
                    finish: Finish::Stop,
                    usage: tiny,
                    grounding: Default::default(),
                    reasoning_summary: Vec::new(),
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
                usage: tiny,
                grounding: Default::default(),
                reasoning_summary: Vec::new(),
            });
        }
        // ワーカー。プロバイダは 200 を返し、こちらが `Err` へ変える形（実機の
        // #103 と同じ経路 = `reject_empty_reasoning` を通した後の値）。
        Err(LlmError::OutputTruncated {
            limit: 64,
            usage: WORKER_USAGE,
        })
    }
}

#[tokio::test]
async fn a_paid_failure_is_settled_into_stats_log_and_budget() {
    let dir = TempDir::new();
    let log_path = dir.0.join("fuseforks.log");
    fuseforks_core::open_log(&log_path).expect("ログを開けること");
    std::fs::write(dir.0.join("world.json"), r#"{ "tokenBudget": 3000 }"#).unwrap();

    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(Arc::new(AskThenTruncatedWorker))),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig {
            schedule_interval: Duration::from_secs(3600),
            ..OrchestratorConfig::default()
        },
    )
    .await
    .expect("bootstrap できること");
    // ホストの OS ロケールに依存させない（CI は en・開発機は ja）。
    orchestrator
        .set_language(fuseforks_core::world::Language::Ja)
        .await
        .unwrap();
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();

    let lead = AgentId::from("agent_lead");
    let worker = AgentId::from("agent_w1");
    orchestrator
        .create_agent(AgentSpec::new(worker.clone(), "ワーカー", "tpl"))
        .await
        .unwrap();
    let mut spec = AgentSpec::new(lead.clone(), "進行役", "tpl");
    spec.connected_agents = vec![worker.clone()];
    orchestrator.create_agent(spec).await.unwrap();
    orchestrator.start_agent(&worker).await.unwrap();
    orchestrator.start_agent(&lead).await.unwrap();

    let before = orchestrator.snapshot(&worker).await.unwrap().total_tokens;
    assert_eq!(before, 0, "対照: 開始時点の累計は 0");

    let mut rx = orchestrator.subscribe();
    orchestrator.send_user_message(&lead, "調べて").await.unwrap();
    let mut events = Vec::new();
    while let Ok(Ok(event)) = tokio::time::timeout(Duration::from_millis(600), rx.recv()).await {
        events.push(event);
    }

    // 1. カードの累計 — 失敗した個体の数字が**払った量ぶん**動く。
    let after = orchestrator.snapshot(&worker).await.unwrap().total_tokens;
    assert_eq!(
        after,
        WORKER_USAGE.total(),
        "失敗したターンの払い（{}）がカードの累計に入ること",
        WORKER_USAGE.total()
    );

    // 2. `turn:` 行 — 成功と同じ欄で、stop だけが failed:CODE。
    let body = std::fs::read_to_string(&log_path).expect("ログが読めること");
    let worker_turn = body
        .lines()
        .find(|l| l.contains("turn: agent=agent_w1"))
        .unwrap_or_else(|| panic!("失敗したターンにも `turn:` 行が出ること:\n{body}"));
    for needle in [
        "stop=failed:LLM_OUTPUT_TRUNCATED",
        "rounds=1/-",
        "prompt=2000",
        "cached=0",
        "total=2125",
        "reasoning=125",
        "backend=ask-then-truncated",
    ] {
        assert!(
            worker_turn.contains(needle),
            "`{needle}` が読めること: {worker_turn}"
        );
    }
    // （1 呼び出しの計器 `truncated:` はここでは出ない — このバックエンドは
    // `reject_empty_reasoning` を通さず `Err` を直接返す。あの行は
    // `tests/truncated_note.rs` が留めている。）

    // 3. 予算 — 切れた応答の払いが財布から引かれ、同じ因果の次の予約が通らない。
    // **残額まで数字で見る**: 3,000 − 5（進行役 1 周目）− 2,500（ワーカー）= 495。
    // 旧実装（Drop で返金）なら 2,995 で、床 1,000 の予約は通っていた。
    assert!(
        body.contains("budget stop: agent=agent_lead") && body.contains("reason=reserve_short"),
        "ワーカーの払いが引かれていれば、進行役の 2 周目は床ぶんを確保できない:\n{body}"
    );
    assert!(
        body.contains("remaining=495"),
        "残額 = 3,000 − 5 − 2,500 = 495 が読めること:\n{body}"
    );
    let messages: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            CoreEvent::MessageSent { message } => Some(message),
            _ => None,
        })
        .collect();
    assert!(
        messages
            .iter()
            .any(|m| m.from == Endpoint::System && m.content.contains("予算")),
        "System 行「予算」が出ること: {messages:#?}"
    );
    // 負の対照: 予約が Drop で返金されていた旧実装なら、進行役の 2 周目は通って
    // ユーザーへ「結果: …」が返る。**返らないこと**が、払いが引かれた証拠。
    assert!(
        !messages.iter().any(|m| {
            matches!(&m.from, Endpoint::Agent { id } if *id == lead)
                && m.to == Endpoint::User
                && m.content.starts_with("結果:")
        }),
        "旧実装（全額返金）の形が出ていないこと: {messages:#?}"
    );
}
