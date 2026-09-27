//! 判断特化のエージェント（Spec 62・`judge_contract`）の結合テスト。
//!
//! 判断モデルは偽物（`FakeJudge`）を差し込み、呼び出し元（進行役）は固定の台本で
//! `judge_router` を 1 回だけ呼ぶ。見るのは**配送が起きたか・どこへ・答えがどこへ戻ったか**。

mod judge_support;

use fuseforks_core::event::CoreEvent;
use fuseforks_core::judge::JudgeStatus;
use fuseforks_core::model::{AgentId, Endpoint, JudgeSpec, UnknownFields};
use judge_support::{COORDINATOR, RULES, deliveries_to, run, village};

#[tokio::test]
async fn a_judge_routes_to_one_servant_and_the_answer_returns_to_the_caller() {
    let v = village("one", Some(("research", 0.9)), true, false).await;
    run(&v).await;

    let delivered = deliveries_to(&v, "agent_02").await;
    assert_eq!(delivered.len(), 1, "イクスへ 1 通");
    assert!(delivered[0].starts_with("LangGraph と AionUi の委譲を比べて"), "本文は呼び出し元のまま: {}", delivered[0]);
    assert!(delivered[0].ends_with("［判断: 振り分け役 → kind=research(0.90)］"), "判定 1 行が末尾: {}", delivered[0]);
    assert!(deliveries_to(&v, "agent_03").await.is_empty(), "ザリへは行かない");
    // 答えは構造上呼び出し元へ戻る（利用者へは流れない）。
    assert_eq!(v.tool_results.lock().unwrap().clone(), vec!["ワーカーの答え".to_owned()]);
}

#[tokio::test]
async fn two_destinations_fan_out_and_bundle_without_a_plan_wave() {
    let v = village("fan", Some(("implement", 0.8)), true, false).await;
    let events = run(&v).await;

    assert_eq!(deliveries_to(&v, "agent_02").await.len(), 1);
    assert_eq!(deliveries_to(&v, "agent_03").await.len(), 1);
    let results = v.tool_results.lock().unwrap().clone();
    assert_eq!(results.len(), 1);
    assert!(results[0].contains("## agent_02（イクス）\nワーカーの答え"), "{}", results[0]);
    assert!(results[0].contains("## agent_03（ザリ）\nワーカーの答え"), "{}", results[0]);
    // **execute_wave を通らない** — 波の記録もイベントも出ない。
    assert!(v.orchestrator.list_plan_waves().await.is_empty(), "波の記録が無い");
    assert!(
        !events.iter().any(|e| matches!(
            e,
            CoreEvent::PlanWaveStarted { .. } | CoreEvent::PlanTaskResolved { .. } | CoreEvent::PlanWaveFinished { .. }
        )),
        "波のイベントが出ない"
    );
}

#[tokio::test]
async fn routing_back_to_the_caller_is_refused_as_a_circular_wait() {
    let v = village("circular", Some(("other", 0.95)), true, false).await;
    run(&v).await;

    let results = v.tool_results.lock().unwrap().clone();
    assert_eq!(results.len(), 1);
    assert!(results[0].contains("循環する委譲"), "Spec 44 の拒否がそのまま効く: {}", results[0]);
    assert!(deliveries_to(&v, COORDINATOR).await.is_empty(), "自分へは配送しない");
}

#[tokio::test]
async fn a_judge_model_failure_delivers_nothing_and_does_not_fall_to_otherwise() {
    // otherwise の行き先は agent_02。失敗がそこへ流れたら 1 通届く。
    let v = village("fail", None, true, false).await;
    run(&v).await;

    assert_eq!(*v.judge_calls.lock().unwrap(), 1, "判断モデルは呼ばれた");
    assert!(deliveries_to(&v, "agent_02").await.is_empty(), "otherwise へ流れない");
    let results = v.tool_results.lock().unwrap().clone();
    assert_eq!(results.len(), 1);
    assert!(results[0].starts_with("判定できませんでした"), "{}", results[0]);
}

#[tokio::test]
async fn without_a_judge_model_no_judge_tool_is_offered() {
    let v = village("nomodel", Some(("research", 0.9)), false, true).await;
    run(&v).await;

    let rounds = v.seen.lock().unwrap().clone();
    let coordinator = rounds.iter().find(|n| n.iter().any(|t| t == "ask_agent_02")).expect("進行役の周");
    assert!(!coordinator.iter().any(|t| t.starts_with("judge_")), "判断モデルが無ければ生えない: {coordinator:?}");
    let views = v.orchestrator.judges().await;
    assert_eq!(views[0].status, JudgeStatus::NoJudgeModel);
    // 概形はファイルだけで決まる（判断モデルの有無と独立）— 地図のホバーが読む。
    let outline = views[0].outline.as_ref().expect("ファイルは読めている");
    assert_eq!(outline.rules, 3);
    assert_eq!(outline.questions.len(), 1);
    assert_eq!(outline.questions[0].name, "kind");
}

#[tokio::test]
async fn a_judge_turned_off_offers_no_tool_and_keeps_its_file() {
    let v = village("off", Some(("research", 0.9)), true, true).await;
    let off = |enabled| JudgeSpec {
        id: "router".into(),
        name: "振り分け役".into(),
        order: 0,
        enabled,
        unknown: UnknownFields::default(),
    };
    v.orchestrator.update_judge(off(false)).await.unwrap();
    run(&v).await;

    let rounds = v.seen.lock().unwrap().clone();
    let coordinator = rounds.iter().find(|n| n.iter().any(|t| t == "ask_agent_02")).expect("進行役の周");
    assert!(!coordinator.iter().any(|t| t.starts_with("judge_")), "無効なら生えない: {coordinator:?}");
    let views = v.orchestrator.judges().await;
    assert_eq!(views[0].status, JudgeStatus::Disabled);
    assert!(!views[0].enabled);
    // ファイルと行き先は残る（戻したときにそのまま使える）。地図の破線は状態で止める。
    assert!(views[0].outline.is_some());
    assert!(!views[0].targets.is_empty());

    v.orchestrator.update_judge(off(true)).await.unwrap();
    assert_eq!(v.orchestrator.judges().await[0].status, JudgeStatus::Active);
}

#[tokio::test]
async fn a_judge_gets_no_ask_transfer_or_plan_and_does_not_count_as_a_plan_target() {
    let v = village("split", Some(("research", 0.9)), true, true).await;
    run(&v).await;

    let rounds = v.seen.lock().unwrap().clone();
    let coordinator = rounds.iter().find(|n| n.iter().any(|t| t == "judge_router")).expect("進行役の周");
    assert!(coordinator.iter().any(|t| t == "ask_agent_02"), "サーヴァントへの委譲は残る: {coordinator:?}");
    assert!(!coordinator.iter().any(|t| t == "ask_router" || t == "transfer_to_router"), "判断役に ask / transfer は生えない: {coordinator:?}");
    // サーヴァント 1 体 + 判断役 1 = 接続 2 だが、plan はサーヴァントだけで数える。
    assert!(!coordinator.iter().any(|t| t == "plan"), "plan は生えない: {coordinator:?}");
}

#[tokio::test]
async fn deleting_a_servant_named_in_to_disables_the_judge_and_says_so() {
    let v = village("delete", Some(("research", 0.9)), true, false).await;
    assert_eq!(v.orchestrator.judges().await[0].status, JudgeStatus::Active);

    v.orchestrator.delete_agent(&"agent_03".into()).await.unwrap();

    assert_eq!(
        v.orchestrator.judges().await[0].status,
        JudgeStatus::MissingTargets { targets: vec!["agent_03".into()] }
    );
    let said = v
        .orchestrator
        .message_log(None)
        .await
        .into_iter()
        .any(|m| m.from == Endpoint::System && m.content.contains("判断役 router（振り分け役）は行き先の agent_03 が消えた"));
    assert!(said, "名指しで知らせる");
}

#[tokio::test]
async fn saving_rejects_invalid_rules_and_unknown_destinations() {
    let v = village("save", Some(("research", 0.9)), true, false).await;
    let router: AgentId = "router".into();

    let err = v.orchestrator.save_judge_file(&router, &RULES.replace("[otherwise]\nto = [\"agent_02\"]\n", "")).await.unwrap_err();
    assert_eq!(err.code(), "INVALID_JUDGE_FILE");
    let err = v.orchestrator.save_judge_file(&router, &RULES.replace("agent_03", "agent_99")).await.unwrap_err();
    assert_eq!(err.code(), "INVALID_JUDGE_FILE");
    assert!(err.to_string().contains("agent_99"), "{err}");
    // 判断役の ID も行き先には書けない（サーヴァントではない）。
    let err = v.orchestrator.save_judge_file(&router, &RULES.replace("agent_03", "router")).await.unwrap_err();
    assert_eq!(err.code(), "INVALID_JUDGE_FILE");
    // 落ちた保存はファイルを変えない。
    assert_eq!(v.orchestrator.read_judge_file(&router).await.unwrap(), RULES);
}

#[tokio::test]
async fn a_new_judge_starts_from_a_template_that_passes_the_checks() {
    let v = village("template", Some(("research", 0.9)), true, false).await;
    v.orchestrator
        .create_judge(JudgeSpec { id: "second".into(), name: "二番目".into(), order: 1, enabled: true, unknown: UnknownFields::default() })
        .await
        .unwrap();
    let text = v.orchestrator.read_judge_file(&"second".into()).await.unwrap();
    assert!(text.contains("[otherwise]"));
    let second = v.orchestrator.judges().await.into_iter().find(|j| j.id.as_str() == "second").unwrap();
    assert_eq!(second.status, JudgeStatus::Active, "雛形はそのまま有効");
}
