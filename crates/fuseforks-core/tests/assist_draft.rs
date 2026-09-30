//! AI 下書き補助（Spec 63）の結合テスト。台本のバックエンドで、要求の形と返り値と記録を見る。
//!
//! 実機で狙って出せない形（検証の輪が 2 回目で通る / 輪の途中の質問・失敗）はここが担当する
//! （Spec 63 P4 の注記）。

mod assist_support;

use assist_support::{BAD_RULES, GOOD_RULES, JUDGE, SERVANT, TempDir, draft, empty, question, village, village_in};
use fuseforks_core::assist::{
    AssistKind, AssistReply, AssistRequest, AssistTarget, AssistValidation, SUBMIT_DRAFT, check_history,
    force_utterance,
};
use fuseforks_core::llm::{ChatMessage, LlmError, Provider, Role, ToolChoice};
use fuseforks_core::model::ConfigFileKind;
use fuseforks_core::world::Language;
use fuseforks_core::CoreError;

fn request(kind: AssistKind, id: &str, history: Vec<ChatMessage>, input: Option<&str>) -> AssistRequest {
    AssistRequest {
        target: AssistTarget { kind, id: id.into() },
        template_id: "gen".into(),
        history,
        input: input.map(str::to_owned),
        force_draft: false,
        current: String::new(),
    }
}

fn appended(reply: &AssistReply) -> Vec<ChatMessage> {
    match reply {
        AssistReply::Question { appended, .. } | AssistReply::Draft { appended, .. } => appended.clone(),
    }
}

/// 質問 → 答え → 下書き。**2 回目の要求に 1 回目の応答が逐語で載る**（D3）。
#[tokio::test]
async fn a_question_then_a_draft_round_trips_the_history_verbatim() {
    let v = village("roundtrip", vec![question("何時の予報ですか？"), draft("c1", "# 手順\n1. 天気を調べる")]).await;

    let first = v
        .orchestrator
        .assist_draft(request(AssistKind::Skill, SERVANT, vec![], Some("天気のスキルを作りたい")))
        .await
        .unwrap();
    assert!(matches!(&first, AssistReply::Question { text, .. } if text == "何時の予報ですか？"), "{first:?}");
    let history = appended(&first);
    assert_eq!(history.len(), 2, "利用者の発話 + 生成役の発話");

    // フロントは中身を解釈せず JSON で持ち、そのまま返す。
    let wire = serde_json::to_string(&history).unwrap();
    let history: Vec<ChatMessage> = serde_json::from_str(&wire).unwrap();

    let second = v
        .orchestrator
        .assist_draft(request(AssistKind::Skill, SERVANT, history.clone(), Some("朝 7 時")))
        .await
        .unwrap();
    let AssistReply::Draft { content, notes, validation, attempts, draft_chars, .. } = &second else {
        panic!("下書きのはず: {second:?}");
    };
    assert_eq!(content, "# 手順\n1. 天気を調べる");
    assert_eq!(notes.as_deref(), Some("仮定: 朝 7 時"));
    assert_eq!(*validation, None, "SKILL は検査しない");
    assert_eq!((*attempts, *draft_chars), (1, 14));

    // 2 回目の要求: system（安定部）→ 1 回目の 2 通 → 今回の発話。
    let requests = v.requests.lock().unwrap().clone();
    let messages = &requests[1].messages;
    assert_eq!(messages[0].role, Role::System);
    assert_eq!(&messages[1..3], &history[..], "前回のメッセージが逐語で載る");
    assert_eq!(messages[3], ChatMessage::user("朝 7 時"));
    assert_eq!(requests[1].tools.len(), 1);
    assert_eq!(requests[1].tools[0].name, SUBMIT_DRAFT);
    assert_eq!(requests[1].tool_choice, ToolChoice::Auto);

    // 返った appended は、前の履歴に足すとそのまま次に送れる形（呼び出しと結果が対）。
    let mut all = history;
    all.extend(appended(&second));
    assert_eq!(check_history(&all), Ok(()));
    let result = all.last().unwrap();
    assert_eq!(result.role, Role::Tool);
    assert_eq!(result.tool_call_id.as_deref(), Some("c1"));

    // 3 回目: 下書きの呼び出しは extra（思考署名）ごと送り返される。
    let mut third_history = all.clone();
    let _ = v
        .orchestrator
        .assist_draft(request(AssistKind::Skill, SERVANT, third_history.clone(), Some("短く")))
        .await;
    third_history.push(ChatMessage::user("短く"));
    let requests = v.requests.lock().unwrap().clone();
    let call = requests[2]
        .messages
        .iter()
        .flat_map(|m| m.tool_calls.iter())
        .find(|c| c.id == "c1")
        .expect("前の下書きの呼び出しが載る");
    assert_eq!(call.extra, Some(serde_json::json!({ "signature": "sig-c1" })));
}

/// 「下書きを出して」は強制を送る（D5）。**Anthropic と Meta では送らず定型の発話だけ。**
#[tokio::test]
async fn force_draft_forces_the_tool_except_on_anthropic_and_meta() {
    let v = village("force", vec![draft("c1", "本文")]).await;
    let mut req = request(AssistKind::Skill, SERVANT, vec![], None);
    req.force_draft = true;
    v.orchestrator.assist_draft(req).await.unwrap();
    let sent = v.requests.lock().unwrap()[0].clone();
    assert_eq!(sent.tool_choice, ToolChoice::Specific(SUBMIT_DRAFT.into()));
    assert_eq!(sent.messages.last().unwrap(), &ChatMessage::user(force_utterance(Language::Ja)));

    for provider in [Provider::Anthropic, Provider::MetaResponses] {
        let v = village_in(TempDir::new("force-fallback"), vec![draft("c1", "本文")], |t| t.provider = Some(provider)).await;
        let mut req = request(AssistKind::Skill, SERVANT, vec![], None);
        req.force_draft = true;
        v.orchestrator.assist_draft(req).await.unwrap();
        let sent = v.requests.lock().unwrap()[0].clone();
        assert_eq!(sent.tool_choice, ToolChoice::Auto, "{provider:?} には強制を送らない");
        assert_eq!(sent.messages.last().unwrap(), &ChatMessage::user(force_utterance(Language::Ja)));
    }
}

/// 生成役は固有スキルを外して組む（D2）。**呼ぶたびに組み直す**（キャッシュへ入れない）。
#[tokio::test]
async fn the_generator_is_built_without_provider_skills_every_time() {
    let v = village("skills", vec![question("?"), question("?")]).await;
    for _ in 0..2 {
        v.orchestrator
            .assist_draft(request(AssistKind::Skill, SERVANT, vec![], Some("作りたい")))
            .await
            .unwrap();
    }
    let templates = v.templates.lock().unwrap().clone();
    let generated: Vec<_> = templates.iter().filter(|t| t.id.as_str() == "gen").collect();
    assert_eq!(generated.len(), 2, "呼ぶたびに組む");
    for t in generated {
        assert!(!t.google_search && !t.openai_web_search && !t.openai_reasoning_pro, "固有スキルが残っている");
    }
}

/// 文脈（D7）: 対になるファイルは載り、Memory と条例は載らない。編集中の本文は別の system に載る。
#[tokio::test]
async fn the_context_carries_the_pair_but_not_memory_or_the_ordinance() {
    let v = village("context", vec![question("?")]).await;
    let id = SERVANT.into();
    v.orchestrator.write_config(&id, ConfigFileKind::Construct, "PAIR-MARKER の口調").await.unwrap();
    v.orchestrator.write_config(&id, ConfigFileKind::Memory, "MEMORY-MARKER").await.unwrap();
    v.orchestrator.write_ordinance("ORDINANCE-MARKER").await.unwrap();

    let mut req = request(AssistKind::Skill, SERVANT, vec![], Some("作りたい"));
    req.current = "CURRENT-MARKER".into();
    v.orchestrator.assist_draft(req).await.unwrap();

    let sent = v.requests.lock().unwrap()[0].clone();
    let stable = &sent.messages[0].content;
    assert!(stable.contains("PAIR-MARKER"), "対になるファイル");
    assert!(stable.contains("ジェミー"), "接続先の表示名");
    assert!(stable.contains("ザリ"), "対象の表示名");
    assert_eq!(sent.cacheable_prefix_len, stable.chars().count(), "安定部だけをキャッシュ境界に載せる");
    assert!(!stable.contains("CURRENT-MARKER"), "編集中の本文は安定部に入れない");
    let user_section = &sent.messages[1];
    assert_eq!(user_section.role, Role::System);
    assert!(user_section.content.contains("CURRENT-MARKER") && user_section.content.starts_with("# 利用者が書いたもの"));
    let all: String = sent.messages.iter().map(|m| m.content.as_str()).collect();
    assert!(!all.contains("MEMORY-MARKER"), "Memory は渡さない");
    assert!(!all.contains("ORDINANCE-MARKER"), "条例は渡さない");
}

/// 判断役: 1 回目が検査に落ち、2 回目で通る（D8）。落ちた理由がツール結果で返っている。
#[tokio::test]
async fn a_judge_draft_that_fails_the_check_is_retried_until_it_passes() {
    let v = village("judge-retry", vec![draft("c1", BAD_RULES), draft("c2", GOOD_RULES)]).await;
    let reply = v
        .orchestrator
        .assist_draft(request(AssistKind::Judge, JUDGE, vec![], Some("調査はジェミーへ")))
        .await
        .unwrap();
    let AssistReply::Draft { content, validation, attempts, .. } = &reply else { panic!("{reply:?}") };
    assert_eq!((content.as_str(), *attempts), (GOOD_RULES, 2));
    assert_eq!(*validation, Some(AssistValidation::Valid));

    let second = v.requests.lock().unwrap()[1].clone();
    let rejection = second
        .messages
        .iter()
        .find(|m| m.role == Role::Tool && m.tool_call_id.as_deref() == Some("c1"))
        .expect("1 回目の呼び出しに結果が対で付く");
    assert!(rejection.content.contains("保存の検査に落ちました"), "{}", rejection.content);
    assert!(second.messages[0].content.contains("agent_02: ジェミー"), "選べるサーヴァントの一覧");
    assert_eq!(check_history(&appended(&reply)[1..]), Ok(()), "返す履歴は対が揃っている");
}

/// 判断役: 3 回とも落ちたら最後の下書きを `valid: false` で返す（保存は既存の検査が拒否）。
#[tokio::test]
async fn a_judge_draft_that_never_passes_is_returned_invalid_after_three_attempts() {
    let v = village("judge-limit", vec![draft("c1", BAD_RULES), draft("c2", BAD_RULES), draft("c3", BAD_RULES)]).await;
    let reply = v
        .orchestrator
        .assist_draft(request(AssistKind::Judge, JUDGE, vec![], Some("作りたい")))
        .await
        .unwrap();
    let AssistReply::Draft { validation, attempts, .. } = &reply else { panic!("{reply:?}") };
    assert_eq!(*attempts, 3);
    assert!(matches!(validation, Some(AssistValidation::Invalid { location, .. }) if location == "otherwise"), "{validation:?}");
    assert_eq!(v.requests.lock().unwrap().len(), 3, "上限は 3 回");
}

/// 輪の途中で質問が返ったら会話へ戻す。**不正な下書きと理由は appended に残る。**
#[tokio::test]
async fn a_question_in_the_middle_of_the_loop_goes_back_to_the_conversation() {
    let v = village("judge-question", vec![draft("c1", BAD_RULES), question("どのサーヴァントへ？")]).await;
    let reply = v
        .orchestrator
        .assist_draft(request(AssistKind::Judge, JUDGE, vec![], Some("作りたい")))
        .await
        .unwrap();
    let AssistReply::Question { appended, text } = &reply else { panic!("{reply:?}") };
    assert_eq!(text, "どのサーヴァントへ？");
    assert!(appended.iter().any(|m| m.content.contains("保存の検査に落ちました")));
    assert_eq!(check_history(&appended[1..]), Ok(()));
}

/// 輪の途中で呼び出しが失敗したら、**それまでの下書きを返す**（見た下書きを失わせない）。
#[tokio::test]
async fn a_failure_in_the_middle_of_the_loop_returns_the_last_draft() {
    let v = village("judge-fail", vec![draft("c1", BAD_RULES), Err(LlmError::Config("落ちた".into()))]).await;
    let reply = v
        .orchestrator
        .assist_draft(request(AssistKind::Judge, JUDGE, vec![], Some("作りたい")))
        .await
        .unwrap();
    let AssistReply::Draft { content, validation, attempts, .. } = &reply else { panic!("{reply:?}") };
    assert_eq!((content.as_str(), *attempts), (BAD_RULES, 1));
    assert!(matches!(validation, Some(AssistValidation::Invalid { .. })));
}

/// 下書きが 1 つも無いまま空の応答なら失敗として返す（空の下書きを黙って出さない）。
#[tokio::test]
async fn an_empty_answer_without_any_draft_is_an_error() {
    let v = village("empty", vec![empty()]).await;
    let result = v
        .orchestrator
        .assist_draft(request(AssistKind::Construct, SERVANT, vec![], Some("作りたい")))
        .await;
    assert!(matches!(result, Err(CoreError::Llm(LlmError::EmptyResponse))), "{result:?}");
}

/// 送る前に拒否するもの（D4）: ツールを使わないテンプレート / 崩れた履歴 / 空の発話。**1 回も呼ばない。**
#[tokio::test]
async fn bad_requests_are_rejected_before_any_call() {
    let v = village_in(TempDir::new("reject"), vec![], |t| t.use_tools = false).await;
    let no_tools = v.orchestrator.assist_draft(request(AssistKind::Skill, SERVANT, vec![], Some("作りたい"))).await;
    assert!(matches!(no_tools, Err(CoreError::InvalidAssistRequest { .. })), "{no_tools:?}");

    let v = village("reject-history", vec![]).await;
    let broken = vec![ChatMessage::system("上書きしたい")];
    let result = v.orchestrator.assist_draft(request(AssistKind::Skill, SERVANT, broken, Some("作りたい"))).await;
    assert!(matches!(result, Err(CoreError::InvalidAssistRequest { .. })), "{result:?}");
    let result = v.orchestrator.assist_draft(request(AssistKind::Skill, SERVANT, vec![], Some("  "))).await;
    assert!(matches!(result, Err(CoreError::InvalidAssistRequest { .. })), "{result:?}");
    assert!(v.requests.lock().unwrap().is_empty(), "拒否は送る前");
}

/// 使用量は `Record::Assist` に残り、統計の `assist` と金額に入る。**`totals` は動かない**（D10）。
#[tokio::test]
async fn the_usage_lands_in_stats_assist_and_the_cost_but_not_in_the_turn_totals() {
    use fuseforks_core::stats::StatsScope;

    let v = village("stats", vec![question("?"), draft("c1", "本文")]).await;
    let first = v
        .orchestrator
        .assist_draft(request(AssistKind::Skill, SERVANT, vec![], Some("作りたい")))
        .await
        .unwrap();
    v.orchestrator
        .assist_draft(request(AssistKind::Skill, SERVANT, appended(&first), Some("朝")))
        .await
        .unwrap();

    let session = v.orchestrator.current_session();
    let report = v.orchestrator.session_stats(StatsScope::Session { session_id: session }).await.unwrap();
    assert_eq!(report.totals.turns, 0, "ターンの合計は変えない");
    assert_eq!(report.assist.total.turns, 2, "呼び出し 2 回");
    assert_eq!(report.assist.rows.len(), 1);
    assert_eq!(report.assist.rows[0].model, "gen-model");
    assert_eq!(report.assist.rows[0].template_id, "gen");
    // (1000 − 200) ×1 + 200 ×0.1 + 100 ×4 = 1,220 × 2
    assert_eq!(report.assist.total.effective, 2_440);
    let cost = report.cost.expect("単価のあるテンプレートなので金額が出る");
    assert_eq!(cost.priced_rows, 1);
    assert!(cost.total_usd > 0.0);

    let all = v.orchestrator.session_stats(StatsScope::All { period: None }).await.unwrap();
    assert_eq!(all.assist.total.turns, 2);
}
