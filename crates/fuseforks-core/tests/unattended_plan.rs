//! 無人の計画（Spec 53 P1）の結合テスト — 計画の確認（Spec 43）を開けない 2 条件。
//!
//! 検査の軸は 4 つ —
//! (1) `autoApprovePlans` の予定から始まった因果では窓が開かない（凍結 11 (a)）
//! (2) ステータスバーのスイッチで窓が開かない・既定は OFF（凍結 11 (b)）
//! (3) 印は転送と検収の再依頼へ**写される**（予定の外の根では立たない）
//! (4) 計器 `plan review skipped:` の理由と、両方真のときの優先（`schedule`）
//!
//! **診断の出口はプロセスで 1 つ**（`OnceLock`）なので、**ログを読むテストは
//! このファイルに 1 つだけ**（`the_skip_reason_is_logged_and_schedule_wins`）。
//! 他のテストは `list_plan_waves` の波の状態で読む。
//!
//! 外部依頼（MCP）の根で印が偽になることはここでは確かめない — 外部依頼は
//! 必ず答えを待つ委譲なので、凍結 10 で窓がそもそも開かない（観測で区別できない）。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fuseforks_core::llm::{ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, ToolCall, Usage};
use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::orchestrator::ProbeApprovals;
use fuseforks_core::plan::PlanWaveState;
use fuseforks_core::schedule::{Acceptance, Recurrence, ScheduleOptions, Weekday};
use fuseforks_core::schedule_probe::{PROBE_TIMEOUT_DEFAULT, ScheduleProbe};
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-unattended-{tag}-{}",
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

/// 進行役は「撒いて」で plan を 1 回呼ぶ。中継役は「回して」で転送を 1 回呼ぶ。
/// それ以外（ワーカーの答え・束ねを受けた報告）は本文 1 発。
struct UnattendedBackend {
    /// plan の宛先（テストごとに固有の id — ログを grep しても混ざらない）。
    workers: (String, String),
}

#[async_trait::async_trait]
impl LlmBackend for UnattendedBackend {
    fn name(&self) -> &str {
        "unattended"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let latest = req
            .messages
            .iter()
            .rev()
            .find_map(|m| (!m.content.is_empty()).then(|| m.content.clone()))
            .unwrap_or_default();
        // 束ねを受けた進行役は報告だけする（もう一度撒くと波が増えて数えにくい）。
        if latest.contains("束ね") {
            return Ok(ok_response("報告します", Vec::new()));
        }
        if latest.contains("回して")
            && let Some(transfer) = req.tools.iter().find(|t| t.name.starts_with("transfer_to_"))
        {
            return Ok(ok_response(
                "",
                vec![ToolCall {
                    id: "call_transfer".into(),
                    name: transfer.name.clone(),
                    args: serde_json::json!({ "message": "撒いて" }),
                    extra: None,
                }],
            ));
        }
        if latest.contains("撒いて") && req.tools.iter().any(|t| t.name == "plan") {
            return Ok(ok_response(
                "",
                vec![ToolCall {
                    id: "call_plan".into(),
                    name: "plan".into(),
                    args: serde_json::json!({ "tasks": [
                        { "to": self.workers.0, "message": "Aを調べて" },
                        { "to": self.workers.1, "message": "Bを調べて" },
                    ]}),
                    extra: None,
                }],
            ));
        }
        Ok(ok_response("了解の答えです", Vec::new()))
    }
}

/// 全部承認する差し込み（承認の門そのものは Spec 28 のテストが持つ）。
struct ApproveAll;

impl ProbeApprovals for ApproveAll {
    fn is_approved(&self, _key: &str) -> bool {
        true
    }
}

struct Village {
    _dir: TempDir,
    orchestrator: Orchestrator,
    /// 計画の確認がオンの進行役。
    coordinator: AgentId,
    /// 進行役へ転送するだけの中継役（接続 1 体なので plan は生えない）。
    relay: AgentId,
}

/// `ids` = (tag, 進行役, 中継役, ワーカー A, ワーカー B)。
async fn setup(ids: (&str, &str, &str, &str, &str)) -> Village {
    let dir = TempDir::new(ids.0);
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(Arc::new(UnattendedBackend {
            workers: (ids.3.to_owned(), ids.4.to_owned()),
        }))),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig {
            // 時刻の tick を混ぜない（tick は手動で固定時刻を渡す）。
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

    let coordinator = AgentId::from(ids.1);
    let relay = AgentId::from(ids.2);
    let worker_a = AgentId::from(ids.3);
    let worker_b = AgentId::from(ids.4);
    for id in [&worker_a, &worker_b] {
        orchestrator
            .create_agent(AgentSpec::new(id.clone(), id.as_str(), "tpl"))
            .await
            .unwrap();
        orchestrator.start_agent(id).await.unwrap();
    }
    let mut spec = AgentSpec::new(coordinator.clone(), "進行役", "tpl");
    spec.connected_agents = vec![worker_a, worker_b];
    spec.plan_review = true;
    orchestrator.create_agent(spec).await.unwrap();
    orchestrator.start_agent(&coordinator).await.unwrap();

    let mut relay_spec = AgentSpec::new(relay.clone(), "中継役", "tpl");
    relay_spec.connected_agents = vec![coordinator.clone()];
    orchestrator.create_agent(relay_spec).await.unwrap();
    orchestrator.start_agent(&relay).await.unwrap();

    Village {
        _dir: dir,
        orchestrator,
        coordinator,
        relay,
    }
}

/// 静かになるまでイベントを飲む（窓は stats_interval = 1 秒より短く保つ —
/// `failures.md` #86）。
async fn drain(rx: &mut tokio::sync::broadcast::Receiver<fuseforks_core::event::CoreEvent>) {
    while tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .is_ok()
    {}
}

fn jst_at(y: i32, m: u32, d: u32, h: u32, min: u32, s: u32) -> chrono::DateTime<chrono::FixedOffset> {
    use chrono::TimeZone;
    chrono::FixedOffset::east_opt(9 * 3600)
        .expect("JST は妥当なオフセット")
        .with_ymd_and_hms(y, m, d, h, min, s)
        .single()
        .expect("テストの時刻は一意に決まる")
}

const THU_17: Recurrence = Recurrence::Weekly {
    weekday: Weekday::Thu,
    hour: 17,
    minute: 0,
};

/// 予定を 1 件登録し、木曜 17 時の tick を回して静かになるまで待つ。
async fn fire(village: &Village, to: &AgentId, message: &str, options: ScheduleOptions) {
    village
        .orchestrator
        .create_schedule(to.clone(), message.into(), THU_17, options)
        .await
        .unwrap();
    let mut rx = village.orchestrator.subscribe();
    village
        .orchestrator
        .run_schedule_tick(jst_at(2026, 7, 30, 17, 0, 29))
        .await;
    drain(&mut rx).await;
}

async fn ask(village: &Village, message: &str) {
    let mut rx = village.orchestrator.subscribe();
    village
        .orchestrator
        .send_user_message(&village.coordinator, message)
        .await
        .unwrap();
    drain(&mut rx).await;
}

async fn wave_states(orchestrator: &Orchestrator) -> Vec<PlanWaveState> {
    orchestrator
        .list_plan_waves()
        .await
        .into_iter()
        .map(|w| w.state)
        .collect()
}

fn auto() -> ScheduleOptions {
    ScheduleOptions {
        auto_approve_plans: true,
        ..ScheduleOptions::default()
    }
}

/// 凍結 11 の (a)(b) と計器（**このファイルで唯一ログを読むテスト**）。
///
/// 1 本目は予定の印とスイッチが**両方真** — 理由は `schedule` が出る（優先）。
/// 2 本目は利用者の依頼でスイッチだけ真 — 理由は `bypass`。どちらも `plan pending:` は出ない。
#[tokio::test]
async fn the_skip_reason_is_logged_and_schedule_wins() {
    let log_dir = TempDir::new("log");
    let log_path = log_dir.0.join("fuseforks.log");
    fuseforks_core::open_log(&log_path).expect("ログを開けること");

    let village = setup(("instr", "una_log_c", "una_log_r", "una_log_wa", "una_log_wb")).await;
    village.orchestrator.set_plan_review_bypass(true);

    fire(&village, &village.coordinator, "撒いて", auto()).await;
    ask(&village, "撒いて").await;

    assert_eq!(
        wave_states(&village.orchestrator).await,
        vec![PlanWaveState::Dispatched, PlanWaveState::Dispatched],
        "予定の印でもスイッチでも窓は開かず、ターンの中で撒く"
    );

    let body = std::fs::read_to_string(&log_path).expect("ログが読めること");
    let schedule_line = body
        .find("plan review skipped: agent=una_log_c reason=schedule")
        .expect("予定の印とスイッチが両方真なら理由は schedule");
    let bypass_line = body
        .find("plan review skipped: agent=una_log_c reason=bypass")
        .expect("利用者の依頼でスイッチだけ真なら理由は bypass");
    assert!(schedule_line < bypass_line, "発火が先・利用者の依頼が後");
    assert!(
        !body.contains("plan pending: agent=una_log_c"),
        "窓を開けていないので提示の計器は出ない:\n{body}"
    );
}

/// 凍結 11 (a) の負の対照 — 印の無い予定と利用者の依頼では、今までどおり窓が開く。
#[tokio::test]
async fn without_the_flag_the_window_still_opens() {
    let village = setup(("negative", "una_neg_c", "una_neg_r", "una_neg_wa", "una_neg_wb")).await;

    fire(&village, &village.coordinator, "撒いて", ScheduleOptions::default()).await;
    ask(&village, "撒いて").await;

    assert_eq!(
        wave_states(&village.orchestrator).await,
        vec![PlanWaveState::Pending, PlanWaveState::Pending],
        "印もスイッチも無ければ、予定からも利用者からも提案で止まる"
    );
}

/// 凍結 11 (b) — スイッチは既定 OFF、入れると利用者の依頼でも開かず、戻すとまた開く。
#[tokio::test]
async fn the_bypass_switch_is_off_by_default_and_toggles() {
    let village = setup(("bypass", "una_byp_c", "una_byp_r", "una_byp_wa", "una_byp_wb")).await;
    assert!(
        !village.orchestrator.plan_review_bypass(),
        "起動時は必ず OFF（保存しない）"
    );

    village.orchestrator.set_plan_review_bypass(true);
    ask(&village, "撒いて").await;
    village.orchestrator.set_plan_review_bypass(false);
    ask(&village, "撒いて").await;

    assert_eq!(
        wave_states(&village.orchestrator).await,
        vec![PlanWaveState::Dispatched, PlanWaveState::Pending],
        "ON の間だけ窓を飛ばし、OFF に戻せば次の依頼からまた止まる"
    );
}

/// 凍結 11 (a) — 印は転送へ写される。予定 → 中継役 → 転送 → 進行役の計画も止まらない。
/// 対照として、印の無い予定で同じ経路を通すと進行役は提案で止まる。
#[tokio::test]
async fn the_flag_follows_a_transfer() {
    let with_flag = setup(("xfer_on", "una_xon_c", "una_xon_r", "una_xon_wa", "una_xon_wb")).await;
    fire(&with_flag, &with_flag.relay, "回して", auto()).await;
    assert_eq!(
        wave_states(&with_flag.orchestrator).await,
        vec![PlanWaveState::Dispatched],
        "転送先の進行役にも予定の印が届き、窓を開けない"
    );

    let without = setup(("xfer_off", "una_xof_c", "una_xof_r", "una_xof_wa", "una_xof_wb")).await;
    fire(&without, &without.relay, "回して", ScheduleOptions::default()).await;
    assert_eq!(
        wave_states(&without.orchestrator).await,
        vec![PlanWaveState::Pending],
        "印の無い予定では、転送先の進行役は提案で止まる（対照）"
    );
}

/// 凍結 11 (a) — 印は検収の再依頼へ写される（`AcceptancePending` の欄）。
///
/// 検収は常に不一致（`NG` を出して `OK` を期待）・総試行 2 回。初回の計画も
/// 再依頼の計画も窓を開けず、波は 2 つとも実行される。
#[tokio::test]
async fn the_flag_follows_an_acceptance_redelivery() {
    let village = setup(("accept", "una_acc_c", "una_acc_r", "una_acc_wa", "una_acc_wb")).await;
    village
        .orchestrator
        .set_probe_approvals(Arc::new(ApproveAll) as Arc<dyn ProbeApprovals>)
        .await;
    let (command, args) = if cfg!(windows) {
        ("cmd".to_owned(), vec!["/C".to_owned(), "echo NG".to_owned()])
    } else {
        ("sh".to_owned(), vec!["-c".to_owned(), "echo NG".to_owned()])
    };
    let options = ScheduleOptions {
        acceptance: Some(Acceptance {
            probe: ScheduleProbe {
                command,
                args,
                expect: "OK".to_owned(),
                timeout_secs: PROBE_TIMEOUT_DEFAULT,
                cwd: None,
            },
            max_attempts: 2,
        }),
        ..auto()
    };
    fire(&village, &village.coordinator, "撒いて", options).await;

    assert_eq!(
        wave_states(&village.orchestrator).await,
        vec![PlanWaveState::Dispatched, PlanWaveState::Dispatched],
        "再依頼の計画も窓を開けない（印が再依頼の封筒へ写っている）"
    );
}
