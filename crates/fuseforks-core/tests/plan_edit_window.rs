//! plan の編集窓（Spec 43）の結合テスト。
//!
//! 二相（提示 → 人の編集 → 実行）の骨格を留める。検査の軸は 4 つ —
//! (1) 提示では**配送が起きない** (2) dispatch は**人が渡した最終形**で撒く
//! (3) 束ねは `Endpoint::System` の配送で進行役の新ターンを起こす
//! (4) OFF の個体（既定）は従来どおり即配送。
//!
//! **診断の出口はプロセスで 1 つ**（`OnceLock`）なので、**ログを読むテストは
//! このファイルに 1 つだけ**（`the_instruments_tell_the_proposal_from_the_dispatch`）。
//! 他のテストは event / `list_plan_waves` / `MessageSent` で読む。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fuseforks_core::llm::{ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, ToolCall, Usage};
use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::plan::{PlanTaskInput, PlanWaveState};
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-planwindow-{tag}-{}",
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

fn ok_response(text: &str, tool_calls: Vec<ToolCall>) -> ChatResponse {
    ChatResponse {
        text: Some(text.to_owned()),
        finish: if tool_calls.is_empty() {
            Finish::Stop
        } else {
            Finish::ToolUse
        },
        tool_calls,
        usage: Usage {
            prompt: 1,
            completion: 1,
            cache_read: 0,
            cache_write: 0,
            cache_write_1h: 0,
            reasoning: 0,
        },
        grounding: Default::default(),
        reasoning_summary: Vec::new(),
    }
}

/// 進行役は「撒いて」の依頼で plan を 1 回呼び、それ以外は本文で答える。
/// ワーカー（plan が提示されない側）は `delay` 眠ってから答える。
struct PlanProposerBackend {
    delay: Duration,
    /// plan の tasks に書く宛先（テストごとに固有の id を使い、プロセス共有の
    /// ログを grep しても他のテストの行と混ざらないようにする）。
    targets: (String, String),
}

#[async_trait::async_trait]
impl LlmBackend for PlanProposerBackend {
    fn name(&self) -> &str {
        "plan-proposer"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let has_plan = req.tools.iter().any(|t| t.name == "plan");
        let latest = req
            .messages
            .iter()
            .rev()
            .find_map(|m| (!m.content.is_empty()).then(|| m.content.clone()))
            .unwrap_or_default();

        // 依頼主の合図（委譲されたターンの窓のテスト用）。`ask_*` が 1 本でも
        // 提示されていれば、その相手へ「撒いて」を委譲する。
        if latest.contains("訊いて") {
            if let Some(ask) = req.tools.iter().find(|t| t.name.starts_with("ask_")) {
                return Ok(ok_response(
                    "",
                    vec![ToolCall {
                        id: "call_ask".into(),
                        name: ask.name.clone(),
                        args: serde_json::json!({ "message": "撒いて" }),
                        extra: None,
                    }],
                ));
            }
        }
        // 進行役を長いターンで塞ぐための合図（受信箱飽和のテスト用）。
        if has_plan && latest.contains("眠って") {
            tokio::time::sleep(Duration::from_millis(1200)).await;
            return Ok(ok_response("起きました", Vec::new()));
        }
        if has_plan && latest.contains("撒いて") {
            return Ok(ok_response(
                "",
                vec![ToolCall {
                    id: "call_plan".into(),
                    name: "plan".into(),
                    args: serde_json::json!({ "tasks": [
                        { "to": self.targets.0, "message": "Aを調べて" },
                        { "to": self.targets.1, "message": "Bを調べて" },
                    ]}),
                    extra: None,
                }],
            ));
        }
        if !self.delay.is_zero() && !has_plan {
            tokio::time::sleep(self.delay).await;
        }
        // 束ねを受けた進行役の報告・提示後の報告・ワーカーの答えはどれも本文 1 発。
        Ok(ok_response("了解の答えです", Vec::new()))
    }
}

async fn setup(
    tag: &str,
    delay: Duration,
    review: bool,
    ids: (&str, &str, &str),
) -> (TempDir, Orchestrator, AgentId, AgentId, AgentId) {
    let dir = TempDir::new(tag);
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(Arc::new(PlanProposerBackend {
            delay,
            targets: (ids.1.to_owned(), ids.2.to_owned()),
        }))),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig {
            schedule_interval: Duration::from_secs(3600),
            ..OrchestratorConfig::default()
        },
    )
    .await
    .expect("bootstrap できること");
    // ホストの OS ロケールに依存させない（CI は en・開発機は ja — Spec 35）。
    orchestrator
        .set_language(fuseforks_core::world::Language::Ja)
        .await
        .unwrap();
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();

    let coordinator = AgentId::from(ids.0);
    let worker_a = AgentId::from(ids.1);
    let worker_b = AgentId::from(ids.2);
    for id in [&worker_a, &worker_b] {
        orchestrator
            .create_agent(AgentSpec::new(id.clone(), id.as_str(), "tpl"))
            .await
            .unwrap();
        orchestrator.start_agent(id).await.unwrap();
    }
    let mut spec = AgentSpec::new(coordinator.clone(), "進行役", "tpl");
    spec.connected_agents = vec![worker_a.clone(), worker_b.clone()];
    spec.plan_review = review;
    orchestrator.create_agent(spec).await.unwrap();
    orchestrator.start_agent(&coordinator).await.unwrap();
    (dir, orchestrator, coordinator, worker_a, worker_b)
}

/// 静かになるまでイベントを飲む（窓は stats_interval = 1 秒より短く保つ —
/// `failures.md` #86）。
async fn drain(rx: &mut tokio::sync::broadcast::Receiver<fuseforks_core::event::CoreEvent>) {
    while tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .is_ok()
    {}
}

/// 配送の記録（誰から誰へ）を文字列の対で集める。
async fn sent_pairs(orchestrator: &Orchestrator) -> Vec<(String, String, String)> {
    orchestrator
        .message_log(None)
        .await
        .into_iter()
        .map(|m| (format!("{:?}", m.from), format!("{:?}", m.to), m.content))
        .collect()
}

/// S1 + S2 + 計器（**このファイルで唯一ログを読むテスト**）。
///
/// 提示 → 編集した最終形で dispatch → 束ねが進行役へ届く、を 1 本の流れで
/// 確かめる。決め手は計器の対 — `plan pending:` の `to=[agent_02,agent_03]` /
/// `msg_chars=10` に対し、dispatch 後の `plan wave:` が **編集後の値**
/// （`to=[agent_03]` / 編集した本文の字数）になっていること（検収 2 の形）。
#[tokio::test]
async fn the_instruments_tell_the_proposal_from_the_dispatch() {
    let dir = TempDir::new("instruments");
    let log_path = dir.0.join("fuseforks.log");
    fuseforks_core::open_log(&log_path).expect("ログを開けること");

    let (_dir, orchestrator, coordinator, _worker_a, worker_b) =
        setup("s1s2", Duration::ZERO, true, ("insp_c", "insp_wa", "insp_wb")).await;
    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&coordinator, "撒いて")
        .await
        .unwrap();
    drain(&mut rx).await;

    // S1 — 提示で止まる: 波は pending・本文つき・ワーカーへの配送ゼロ。
    let waves = orchestrator.list_plan_waves().await;
    assert_eq!(waves.len(), 1);
    assert_eq!(waves[0].state, PlanWaveState::Pending);
    assert_eq!(waves[0].tasks[0].message.as_deref(), Some("Aを調べて"));
    let plan_id = waves[0].plan_id;
    let before_dispatch = sent_pairs(&orchestrator).await;
    assert!(
        !before_dispatch.iter().any(|(from, to, _)| {
            from.contains("insp_c") && (to.contains("insp_wa") || to.contains("insp_wb"))
        }),
        "提示の段でワーカーへの配送が起きてはいけない: {before_dispatch:?}"
    );

    // S2 — 人が 1 件消して本文を書き換えた最終形で dispatch。
    orchestrator
        .dispatch_plan_wave(
            plan_id,
            vec![PlanTaskInput {
                to: worker_b.clone(),
                message: "Cだけ調べて".to_owned(),
            }],
        )
        .await
        .expect("dispatch できること");
    drain(&mut rx).await;

    let waves = orchestrator.list_plan_waves().await;
    assert_eq!(waves[0].state, PlanWaveState::Dispatched);
    assert_eq!(waves[0].tasks.len(), 1, "承認された最終形で置き換わる");
    assert_eq!(waves[0].tasks[0].to, worker_b);

    // 束ねが System 配送で進行役へ届き、進行役の新ターンが利用者へ答える。
    let after = sent_pairs(&orchestrator).await;
    assert!(
        after
            .iter()
            .any(|(from, to, body)| from == "System"
                && to.contains("insp_c")
                && body.contains("束ね")),
        "束ねは System の配送として進行役へ届く: {after:?}"
    );

    // 計器の対（検収 1・2）。
    let body = std::fs::read_to_string(&log_path).expect("ログが読めること");
    assert!(
        body.contains("plan pending: agent=insp_c")
            && body.contains("to=[insp_wa,insp_wb]"),
        "提示の計器が出ること:\n{body}"
    );
    assert!(
        body.contains("plan dispatch: agent=insp_c"),
        "実行の起点の計器が出ること:\n{body}"
    );
    assert!(
        body.contains("plan wave: agent=insp_c") && body.contains("to=[insp_wb]"),
        "配送の計器が**編集後の宛先**で出ること（提示時と違う値 = 同一性の証拠）:\n{body}"
    );
    let pending_pos = body.find("plan pending: agent=insp_c").unwrap();
    let wave_pos = body.find("plan wave: agent=insp_c").unwrap();
    assert!(
        pending_pos < wave_pos,
        "plan wave: は dispatch の後にだけ出る（提示の段では出ない）"
    );
}

/// S3 — 破棄は何も配送せず、事実が System 行として会話に残る。
/// 破棄済みへの dispatch は `PlanWaveNotPending`。
#[tokio::test]
async fn discarding_a_pending_plan_delivers_nothing() {
    let (_dir, orchestrator, coordinator, worker_a, _worker_b) =
        setup("discard", Duration::ZERO, true, ("disc_c", "disc_wa", "disc_wb")).await;
    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&coordinator, "撒いて")
        .await
        .unwrap();
    drain(&mut rx).await;
    let plan_id = orchestrator.list_plan_waves().await[0].plan_id;

    orchestrator.discard_plan_wave(plan_id).await.unwrap();
    drain(&mut rx).await;

    let waves = orchestrator.list_plan_waves().await;
    assert_eq!(waves[0].state, PlanWaveState::Discarded);
    assert!(
        waves[0].tasks.iter().all(|t| t.message.is_none()),
        "破棄で本文を落とす"
    );
    let sent = sent_pairs(&orchestrator).await;
    assert!(
        !sent
            .iter()
            .any(|(from, to, _)| from.contains("disc_c") && to.contains(worker_a.as_str())),
        "破棄した計画からワーカーへの配送が起きてはいけない: {sent:?}"
    );
    assert!(
        sent.iter()
            .any(|(from, to, body)| from == "System" && to == "User" && body.contains("破棄")),
        "破棄の事実が System 行で会話に残ること（D3）: {sent:?}"
    );
    // 破棄済みはもう提案ではない。
    let err = orchestrator
        .dispatch_plan_wave(
            plan_id,
            vec![PlanTaskInput {
                to: worker_a,
                message: "やっぱり".to_owned(),
            }],
        )
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        fuseforks_core::error::CoreError::PlanWaveNotPending { .. }
    ));
}

/// S4 — 既定（OFF）の個体は従来どおり即配送（対照。この 1 本が無いと
/// 「常に提示する実装」でも他のテストが緑になる）。
#[tokio::test]
async fn a_default_agent_still_dispatches_immediately() {
    let (_dir, orchestrator, coordinator, worker_a, _worker_b) =
        setup("control", Duration::ZERO, false, ("ctrl_c", "ctrl_wa", "ctrl_wb")).await;
    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&coordinator, "撒いて")
        .await
        .unwrap();
    drain(&mut rx).await;

    let waves = orchestrator.list_plan_waves().await;
    assert_eq!(waves.len(), 1);
    assert_eq!(
        waves[0].state,
        PlanWaveState::Dispatched,
        "OFF の個体の波は生まれつき dispatched（提示の段を持たない）"
    );
    let sent = sent_pairs(&orchestrator).await;
    assert!(
        sent.iter()
            .any(|(from, to, _)| from.contains("ctrl_c") && to.contains(worker_a.as_str())),
        "OFF の個体は従来どおり即配送する: {sent:?}"
    );
}

/// D8 — 検証は dispatch 時点の接続で掛かる（提示後に線を切ったら拒否）。
/// D9 — 進行役が停止中なら dispatch を断る。
#[tokio::test]
async fn dispatch_validates_connections_and_liveness_at_dispatch_time() {
    let (_dir, orchestrator, coordinator, worker_a, worker_b) =
        setup("validate", Duration::ZERO, true, ("vald_c", "vald_wa", "vald_wb")).await;
    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&coordinator, "撒いて")
        .await
        .unwrap();
    drain(&mut rx).await;
    let plan_id = orchestrator.list_plan_waves().await[0].plan_id;

    // 提示の後に agent_03 への線を切る。
    orchestrator
        .set_connections(&coordinator, vec![worker_a.clone()])
        .await
        .unwrap();
    let err = orchestrator
        .dispatch_plan_wave(
            plan_id,
            vec![PlanTaskInput {
                to: worker_b.clone(),
                message: "Bを調べて".to_owned(),
            }],
        )
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            fuseforks_core::error::CoreError::PlanDispatchInvalid { .. }
        ),
        "切られた線への配送は検証が止める（今の接続で判定）: {err:?}"
    );

    // 重複も同じ 1 実装が止める。
    let err = orchestrator
        .dispatch_plan_wave(
            plan_id,
            vec![
                PlanTaskInput {
                    to: worker_a.clone(),
                    message: "1".to_owned(),
                },
                PlanTaskInput {
                    to: worker_a.clone(),
                    message: "2".to_owned(),
                },
            ],
        )
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        fuseforks_core::error::CoreError::PlanDispatchInvalid { .. }
    ));

    // 進行役を止めると dispatch は断られる（D9 — 自動起動しない）。
    orchestrator.stop_agent(&coordinator).await.unwrap();
    let err = orchestrator
        .dispatch_plan_wave(
            plan_id,
            vec![PlanTaskInput {
                to: worker_a,
                message: "Aを調べて".to_owned(),
            }],
        )
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        fuseforks_core::error::CoreError::NotRunning { .. }
    ));
    // 検証と停止で 3 回断られても、提案は pending のまま残っている。
    assert_eq!(
        orchestrator.list_plan_waves().await[0].state,
        PlanWaveState::Pending
    );
}

/// 進行役の停止は**波ごと畳む**（D5 — `stop_agent` は `interrupt_turn` を通り、
/// 波の実行者の token を切る）。ワーカーは打ち切られ、束ねは作られない —
/// 届け先の居ない束ねのためにワーカーのトークンを払い続けない側に倒す。
/// 凍結 9 の「束ねの破棄」はこの打ち切りの**残余**（完了後の停止・受信箱飽和）
/// を受ける網で、下のテストが担う。
#[tokio::test]
async fn stopping_the_coordinator_cancels_its_dispatched_wave() {
    // ワーカーは 400ms 眠る — その間に進行役を止める。
    let (_dir, orchestrator, coordinator, worker_a, _worker_b) =
        setup("stopmid", Duration::from_millis(400), true, ("stop_c", "stop_wa", "stop_wb")).await;
    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&coordinator, "撒いて")
        .await
        .unwrap();
    drain(&mut rx).await;
    let plan_id = orchestrator.list_plan_waves().await[0].plan_id;

    orchestrator
        .dispatch_plan_wave(
            plan_id,
            vec![PlanTaskInput {
                to: worker_a,
                message: "ゆっくり調べて".to_owned(),
            }],
        )
        .await
        .unwrap();
    // ワーカーが答えを作っている間に進行役を止める。
    orchestrator.stop_agent(&coordinator).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1000)).await;
    drain(&mut rx).await;

    let sent = sent_pairs(&orchestrator).await;
    assert!(
        !sent
            .iter()
            .any(|(from, to, body)| from == "System"
                && to.contains("stop_c")
                && body.contains("束ね")),
        "停止した進行役へ束ねを配送してはいけない: {sent:?}"
    );
    // 波は打ち切りとして正直に閉じる（黙って running を残さない）。
    let waves = orchestrator.list_plan_waves().await;
    assert_eq!(
        waves[0].tasks[0].state,
        fuseforks_core::plan::PlanTaskState::Interrupted,
        "打ち切られたセルは interrupted で確定する: {waves:?}"
    );
}

/// M2（凍結 9）— 波は完走したが束ねを**配送できなかった**とき、束ねは破棄され
/// System 行が残る。決定的に踏むために受信箱の飽和（capacity 1）を使う —
/// 進行役を長いターンで塞ぎ、待ち席を別の発話で埋めておく。
#[tokio::test]
async fn an_undeliverable_bundle_is_discarded_with_a_notice() {
    let dir = TempDir::new("mailboxfull");
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(Arc::new(PlanProposerBackend {
            delay: Duration::from_millis(400),
            targets: ("mbox_wa".to_owned(), "mbox_wb".to_owned()),
        }))),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig {
            schedule_interval: Duration::from_secs(3600),
            mailbox_capacity: 1,
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
    let coordinator = AgentId::from("mbox_c");
    let worker_a = AgentId::from("mbox_wa");
    let worker_b = AgentId::from("mbox_wb");
    for id in [&worker_a, &worker_b] {
        orchestrator
            .create_agent(AgentSpec::new(id.clone(), id.as_str(), "tpl"))
            .await
            .unwrap();
        orchestrator.start_agent(id).await.unwrap();
    }
    // plan は接続 2 体以上でしか生えない（提示条件）ので worker_b も繋ぐ。
    let mut spec = AgentSpec::new(coordinator.clone(), "進行役", "tpl");
    spec.connected_agents = vec![worker_a.clone(), worker_b.clone()];
    spec.plan_review = true;
    orchestrator.create_agent(spec).await.unwrap();
    orchestrator.start_agent(&coordinator).await.unwrap();

    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&coordinator, "撒いて")
        .await
        .unwrap();
    drain(&mut rx).await;
    let plan_id = orchestrator.list_plan_waves().await[0].plan_id;

    orchestrator
        .dispatch_plan_wave(
            plan_id,
            vec![PlanTaskInput {
                to: worker_a,
                message: "ゆっくり調べて".to_owned(),
            }],
        )
        .await
        .unwrap();
    // 進行役を 1,200ms のターンで塞ぎ（1 通目は即消化される）、待ち席を
    // 2 通目で埋める。ワーカーの答え（400ms）が返る頃、束ねの try_send は
    // 満席に当たる。
    orchestrator
        .send_user_message(&coordinator, "眠って")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    orchestrator
        .send_user_message(&coordinator, "眠って")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1000)).await;
    drain(&mut rx).await;

    let sent = sent_pairs(&orchestrator).await;
    assert!(
        !sent
            .iter()
            .any(|(from, to, body)| from == "System"
                && to.contains("mbox_c")
                && body.contains("束ね")),
        "満席の進行役へ束ねが届いてはいけない: {sent:?}"
    );
    assert!(
        sent.iter()
            .any(|(from, to, body)| from == "System" && to == "User" && body.contains("破棄")),
        "破棄の事実が System 行で残ること（凍結 9 — 黙って捨てない）: {sent:?}"
    );
}

/// S2 の凍結 — **配送は RepeatGuard の射程外**であることを、同一内容の束ねを
/// 2 回起こして両方ターンが立つ形で留める（次に読む人が配送へガードを
/// 足しに来る形を先に止める）。
#[tokio::test]
async fn identical_bundles_start_two_turns() {
    let (_dir, orchestrator, coordinator, worker_a, _worker_b) =
        setup("twice", Duration::ZERO, true, ("twic_c", "twic_wa", "twic_wb")).await;
    let mut rx = orchestrator.subscribe();

    for _ in 0..2 {
        orchestrator
            .send_user_message(&coordinator, "撒いて")
            .await
            .unwrap();
        drain(&mut rx).await;
        let plan_id = orchestrator
            .list_plan_waves()
            .await
            .into_iter()
            .find(|w| w.state == PlanWaveState::Pending)
            .expect("提案があること")
            .plan_id;
        orchestrator
            .dispatch_plan_wave(
                plan_id,
                vec![PlanTaskInput {
                    to: worker_a.clone(),
                    message: "同じ依頼".to_owned(),
                }],
            )
            .await
            .unwrap();
        drain(&mut rx).await;
    }

    let sent = sent_pairs(&orchestrator).await;
    let bundles = sent
        .iter()
        .filter(|(from, to, body)| {
            from == "System" && to.contains("twic_c") && body.contains("束ね")
        })
        .count();
    assert_eq!(bundles, 2, "同一内容の束ねが 2 回ともターンを起こす: {sent:?}");
}

/// 委譲で呼ばれたターンでは窓を飛ばす（2026-09-02 の実機 — 凍結 10）。
///
/// 依頼主 R が `ask` で進行役 C（`planReview` = true）へ「撒いて」と頼む。
/// C のターンは戻り口（`reply_to`）を持つので、窓を開けると提示でターンが
/// 終わり、戻り口は「提案した」の 1 行で消費される。束ねは後から hop=0 の
/// 新しい根として届き、戻り口が無いので利用者へ流れる — R は永遠に束ねを
/// 受け取れない。だから委譲されたターンでは従来どおりターンの中で撒いて束ね、
/// R へ戻す。#96 の転送の門（`!awaiting_reply`）と同じ形。
#[tokio::test]
async fn a_delegated_turn_skips_the_window_and_returns_the_bundle_to_the_requester() {
    let (_dir, orchestrator, coordinator, worker_a, worker_b) =
        setup("delegated", Duration::ZERO, true, ("dlg_c", "dlg_wa", "dlg_wb")).await;
    let requester = AgentId::from("dlg_r");
    let mut spec = AgentSpec::new(requester.clone(), "依頼主", "tpl");
    spec.connected_agents = vec![coordinator.clone()];
    orchestrator.create_agent(spec).await.unwrap();
    orchestrator.start_agent(&requester).await.unwrap();
    let mut rx = orchestrator.subscribe();

    orchestrator
        .send_user_message(&requester, "訊いて")
        .await
        .unwrap();
    drain(&mut rx).await;

    // 窓は開かない — 波は生まれつき dispatched で、ワーカーへ配送されている。
    let waves = orchestrator.list_plan_waves().await;
    assert_eq!(waves.len(), 1, "C の plan が 1 波: {waves:?}");
    assert_eq!(
        waves[0].state,
        PlanWaveState::Dispatched,
        "委譲されたターンの plan は提示で止まらない（reply_to があるので待てる場所が無い）"
    );
    let sent = sent_pairs(&orchestrator).await;
    assert!(
        sent.iter().any(|(from, to, _)| from.contains("dlg_c")
            && (to.contains(worker_a.as_str()) || to.contains(worker_b.as_str()))),
        "C はターンの中でワーカーへ撒く: {sent:?}"
    );
    // 束ねは System の配送にならず、C の答えが R へ戻る。
    assert!(
        !sent
            .iter()
            .any(|(from, to, _)| from == "System" && to.contains("dlg_c")),
        "委譲されたターンでは束ねを System 配送にしない（新しい根を作らない）: {sent:?}"
    );
    assert!(
        sent.iter()
            .any(|(from, to, _)| from.contains("dlg_c") && to.contains("dlg_r")),
        "C の答えは依頼主 R へ戻る（利用者へ流れない）: {sent:?}"
    );
    assert!(
        !sent
            .iter()
            .any(|(from, to, _)| from.contains("dlg_c") && to == "User"),
        "C から利用者への発話は無い: {sent:?}"
    );
}
