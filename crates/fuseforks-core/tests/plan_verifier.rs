//! 束ねの検証役（Spec 53 P2）の結合テスト — `plan_verifier_contract`。
//!
//! 検査の軸は 4 つ —
//! (1) 検証役が `None` なら束ねは 1 バイトも変わらない（凍結 1）
//! (2) 既定の検証役の結論が束ねの末尾に付き、計器が出る（凍結 3）
//! (3) 承認時は計画の確認パネルの選択が真実で、村の既定を読まない（凍結 2）
//! (4) 検証しなかった理由が束ねに 1 行書かれる（凍結 4）
//!
//! 時間切れ・予算切れ・無応答・打ち切りの写しは `delegation.rs` の単体テスト
//! （`verification_of_maps_every_task_state`）が持つ — ここで作ると 30 秒以上待つ。
//!
//! **診断の出口はプロセスで 1 つ**（`OnceLock`）なので、**ログを読むテストは
//! このファイルに 1 つだけ**（`a_default_verifier_appends_its_verdict`）。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fuseforks_core::llm::{ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, ToolCall, Usage};
use fuseforks_core::model::{AgentId, AgentSpec, Endpoint, ModelTemplate};
use fuseforks_core::plan::{PlanTaskInput, PlanWaveState};
use fuseforks_core::{
    AgentMessage, ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator,
    OrchestratorConfig,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-verifier-{tag}-{}",
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

/// 役ごとの振る舞い:
/// - 検証役: 「束ねの検証の依頼」を受けたら結論を返す
/// - 中継役: 「訊いて」で進行役へ `ask` する
/// - 進行役: 「撒いて」で plan を 1 回呼び、**それ以外は受け取った本文をそのまま返す**
///   （= 束ねを利用者・依頼主への答えとして観測できる）
/// - ワーカー: 決まった答え
struct VerifierBackend {
    workers: (String, String),
}

#[async_trait::async_trait]
impl LlmBackend for VerifierBackend {
    fn name(&self) -> &str {
        "verifier"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let latest = req
            .messages
            .iter()
            .rev()
            .find_map(|m| (!m.content.is_empty()).then(|| m.content.clone()))
            .unwrap_or_default();
        let has_plan = req.tools.iter().any(|t| t.name == "plan");
        if latest.contains("束ねの検証の依頼") {
            return Ok(ok_response("このまま使える", Vec::new()));
        }
        if latest.contains("訊いて")
            && let Some(ask) = req.tools.iter().find(|t| t.name.starts_with("ask_"))
        {
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
        if has_plan && latest.contains("撒いて") {
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
        if has_plan {
            // 進行役: 束ね（ツール結果 / System の配送）をそのまま答えにする。
            return Ok(ok_response(&latest, Vec::new()));
        }
        Ok(ok_response("了解の答えです", Vec::new()))
    }
}

struct Village {
    _dir: TempDir,
    orchestrator: Orchestrator,
    coordinator: AgentId,
    worker_a: AgentId,
    verifier: AgentId,
    relay: AgentId,
}

/// `ids` = (tag, 進行役, ワーカー A, ワーカー B, 検証役, 中継役)。
/// 検証役は**どこにも繋がない**（絆は要らない — 凍結 6）。
async fn setup(
    ids: (&str, &str, &str, &str, &str, &str),
    review: bool,
    verifier_running: bool,
) -> Village {
    let dir = TempDir::new(ids.0);
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(Arc::new(VerifierBackend {
            workers: (ids.2.to_owned(), ids.3.to_owned()),
        }))),
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

    let coordinator = AgentId::from(ids.1);
    let worker_a = AgentId::from(ids.2);
    let worker_b = AgentId::from(ids.3);
    let verifier = AgentId::from(ids.4);
    let relay = AgentId::from(ids.5);
    for id in [&worker_a, &worker_b, &verifier] {
        orchestrator
            .create_agent(AgentSpec::new(id.clone(), id.as_str(), "tpl"))
            .await
            .unwrap();
    }
    orchestrator.start_agent(&worker_a).await.unwrap();
    orchestrator.start_agent(&worker_b).await.unwrap();
    if verifier_running {
        orchestrator.start_agent(&verifier).await.unwrap();
    }
    let mut spec = AgentSpec::new(coordinator.clone(), coordinator.as_str(), "tpl");
    spec.connected_agents = vec![worker_a.clone(), worker_b.clone()];
    spec.plan_review = review;
    orchestrator.create_agent(spec).await.unwrap();
    orchestrator.start_agent(&coordinator).await.unwrap();

    let mut relay_spec = AgentSpec::new(relay.clone(), relay.as_str(), "tpl");
    relay_spec.connected_agents = vec![coordinator.clone()];
    orchestrator.create_agent(relay_spec).await.unwrap();
    orchestrator.start_agent(&relay).await.unwrap();

    Village {
        _dir: dir,
        orchestrator,
        coordinator,
        worker_a,
        verifier,
        relay,
    }
}

/// 静かになるまでイベントを飲む（窓は stats_interval = 1 秒より短く — `failures.md` #86）。
async fn drain(rx: &mut tokio::sync::broadcast::Receiver<fuseforks_core::event::CoreEvent>) {
    while tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .is_ok()
    {}
}

async fn say(village: &Village, to: &AgentId, message: &str) {
    let mut rx = village.orchestrator.subscribe();
    village
        .orchestrator
        .send_user_message(to, message)
        .await
        .unwrap();
    drain(&mut rx).await;
}

fn is_agent(endpoint: &Endpoint, id: &AgentId) -> bool {
    matches!(endpoint, Endpoint::Agent { id: e } if e == id)
}

/// `from` から `to` への発話を古い順に集める。
async fn said(orchestrator: &Orchestrator, from: &Endpoint, to: &Endpoint) -> Vec<AgentMessage> {
    orchestrator
        .message_log(None)
        .await
        .into_iter()
        .filter(|m| &m.from == from && &m.to == to)
        .collect()
}

/// 検証なしの束ね（ワーカーの表示名 = id）。**検証役が None ならこれとバイト等価**。
fn plain_bundle(a: &str, b: &str) -> String {
    format!("## {a}（{a}）\n了解の答えです\n\n## {b}（{b}）\n了解の答えです")
}

/// 凍結 1 — 検証役が None（既定）なら束ねはバイト等価で、検証役へは何も届かない。
#[tokio::test]
async fn without_a_verifier_the_bundle_is_unchanged() {
    let village = setup(
        ("none", "vf_n_c", "vf_n_wa", "vf_n_wb", "vf_n_v", "vf_n_r"),
        false,
        true,
    )
    .await;
    assert_eq!(village.orchestrator.default_verifier().await, None);

    say(&village, &village.coordinator, "撒いて").await;

    let from_c = Endpoint::Agent {
        id: village.coordinator.clone(),
    };
    let replies = said(&village.orchestrator, &from_c, &Endpoint::User).await;
    assert_eq!(
        replies.last().map(|m| m.content.as_str()),
        Some(plain_bundle("vf_n_wa", "vf_n_wb").as_str()),
        "検証役なしの束ねは既存とバイト等価"
    );
    let to_v = said(
        &village.orchestrator,
        &from_c,
        &Endpoint::Agent {
            id: village.verifier.clone(),
        },
    )
    .await;
    assert!(to_v.is_empty(), "検証役が None なら検証の依頼は届かない");
}

/// 凍結 3 と計器（**このファイルで唯一ログを読むテスト**）。
#[tokio::test]
async fn a_default_verifier_appends_its_verdict() {
    let log_dir = TempDir::new("log");
    let log_path = log_dir.0.join("fuseforks.log");
    fuseforks_core::open_log(&log_path).expect("ログを開けること");

    let village = setup(
        ("ok", "vf_ok_c", "vf_ok_wa", "vf_ok_wb", "vf_ok_v", "vf_ok_r"),
        false,
        true,
    )
    .await;
    village
        .orchestrator
        .set_default_verifier(Some(&village.verifier))
        .await
        .unwrap();

    say(&village, &village.coordinator, "撒いて").await;

    let from_c = Endpoint::Agent {
        id: village.coordinator.clone(),
    };
    let reply = said(&village.orchestrator, &from_c, &Endpoint::User)
        .await
        .pop()
        .expect("進行役が答える");
    assert_eq!(
        reply.content,
        format!(
            "{}\n\n## 検証: vf_ok_v（vf_ok_v）\nこのまま使える",
            plain_bundle("vf_ok_wa", "vf_ok_wb")
        ),
        "束ねの末尾に検証役の結論が 1 節で付く"
    );

    let request = said(
        &village.orchestrator,
        &from_c,
        &Endpoint::Agent {
            id: village.verifier.clone(),
        },
    )
    .await
    .pop()
    .expect("検証の依頼が検証役へ届く（送り手は進行役）");
    assert!(request.content.contains("【束ねの検証の依頼】"));
    assert!(
        request.content.contains("## vf_ok_wa（vf_ok_wa）\nAを調べて"),
        "配った依頼を見出しつきで渡す: {}",
        request.content
    );

    let body = std::fs::read_to_string(&log_path).expect("ログが読めること");
    assert!(
        body.contains("plan verify: agent=vf_ok_c plan_id=")
            && body.contains("verifier=vf_ok_v outcome=ok"),
        "検証の計器が出ること:\n{body}"
    );
}

/// 凍結 2 — 承認時は計画の確認パネルの選択が真実。既定の検証役が居ても、
/// 「なし」で承認すれば付かず、指名して承認すれば付く。
#[tokio::test]
async fn the_panel_choice_decides_on_dispatch() {
    let village = setup(
        ("panel", "vf_p_c", "vf_p_wa", "vf_p_wb", "vf_p_v", "vf_p_r"),
        true,
        true,
    )
    .await;
    village
        .orchestrator
        .set_default_verifier(Some(&village.verifier))
        .await
        .unwrap();
    let tasks = |village: &Village| {
        vec![PlanTaskInput {
            to: village.worker_a.clone(),
            message: "Aを調べて".to_owned(),
        }]
    };

    for choice in [None, Some(village.verifier.clone())] {
        say(&village, &village.coordinator, "撒いて").await;
        let pending = village
            .orchestrator
            .list_plan_waves()
            .await
            .into_iter()
            .rfind(|w| w.state == PlanWaveState::Pending)
            .expect("計画の確認で止まる");
        let mut rx = village.orchestrator.subscribe();
        village
            .orchestrator
            .dispatch_plan_wave(pending.plan_id, tasks(&village), choice)
            .await
            .unwrap();
        drain(&mut rx).await;
    }

    let bundles = said(
        &village.orchestrator,
        &Endpoint::System,
        &Endpoint::Agent {
            id: village.coordinator.clone(),
        },
    )
    .await;
    assert_eq!(bundles.len(), 2, "承認した 2 波の束ねが届く");
    assert!(
        !bundles[0].content.contains("## 検証"),
        "「なし」で承認した波は、既定の検証役が居ても検証しない: {}",
        bundles[0].content
    );
    assert!(
        bundles[1]
            .content
            .contains("## 検証: vf_p_v（vf_p_v）\nこのまま使える"),
        "指名して承認した波は検証する: {}",
        bundles[1].content
    );
}

/// 凍結 4 — 検証しなかった理由（参加者 / 進行役自身 / 停止中 / 待ちの輪）が束ねに書かれる。
#[tokio::test]
async fn skip_reasons_are_written_into_the_bundle() {
    // 検証役 vf_s_v は**起動しない**（停止中の理由を作るため）。
    let village = setup(
        ("skip", "vf_s_c", "vf_s_wa", "vf_s_wb", "vf_s_v", "vf_s_r"),
        false,
        false,
    )
    .await;
    let orchestrator = &village.orchestrator;
    let from_c = Endpoint::Agent {
        id: village.coordinator.clone(),
    };

    let cases = [
        (village.worker_a.clone(), "（検証役「vf_s_wa」が束ねに参加したため、検証していません）"),
        (village.coordinator.clone(), "（検証役が進行役自身だったため、検証していません）"),
        (village.verifier.clone(), "（検証役「vf_s_v」に届けられなかった（停止中）ため、検証していません）"),
    ];
    for (verifier, note) in cases {
        orchestrator.set_default_verifier(Some(&verifier)).await.unwrap();
        say(&village, &village.coordinator, "撒いて").await;
        let reply = said(orchestrator, &from_c, &Endpoint::User)
            .await
            .pop()
            .expect("進行役が答える");
        assert_eq!(
            reply.content,
            format!("{}\n\n{note}", plain_bundle("vf_s_wa", "vf_s_wb")),
            "理由の 1 行が束ねの末尾に付く"
        );
    }

    // 待ちの輪 — 中継役が進行役へ ask し、進行役がその委譲ターンで撒く。
    // 検証役 = 中継役は答えを待っている最中なので検証を頼めない。
    orchestrator
        .set_default_verifier(Some(&village.relay))
        .await
        .unwrap();
    say(&village, &village.relay, "訊いて").await;
    let to_relay = said(
        orchestrator,
        &from_c,
        &Endpoint::Agent {
            id: village.relay.clone(),
        },
    )
    .await;
    let answer = to_relay
        .iter()
        .rfind(|m| m.content.contains("## vf_s_wa"))
        .expect("進行役の答え（束ね）が依頼主の中継役へ戻る");
    assert!(
        answer.content.ends_with(
            "（検証役「vf_s_r」がこの依頼の因果の中で答えを待っているため、検証していません）"
        ),
        "待ちの輪の理由: {}",
        answer.content
    );
    assert!(
        !said(orchestrator, &from_c, &Endpoint::Agent { id: village.relay.clone() })
            .await
            .iter()
            .any(|m| m.content.contains("束ねの検証の依頼")),
        "輪になる相手へは検証の依頼を送らない"
    );
}

/// 凍結 6 — 既定の検証役を削除すると「なし」に読める（束ねはバイト等価）。
/// 未登録の個体を指定すると拒否し、設定は変わらない。
#[tokio::test]
async fn a_deleted_verifier_reads_as_none_and_unknown_is_rejected() {
    let village = setup(
        ("deleted", "vf_d_c", "vf_d_wa", "vf_d_wb", "vf_d_v", "vf_d_r"),
        false,
        true,
    )
    .await;
    let orchestrator = &village.orchestrator;
    orchestrator
        .set_default_verifier(Some(&village.verifier))
        .await
        .unwrap();

    let err = orchestrator
        .set_default_verifier(Some(&AgentId::from("vf_d_ghost")))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        fuseforks_core::error::CoreError::AgentNotFound(_)
    ));
    assert_eq!(
        orchestrator.default_verifier().await,
        Some(village.verifier.clone()),
        "拒否したときは設定が変わらない"
    );

    orchestrator.delete_agent(&village.verifier).await.unwrap();
    assert_eq!(
        orchestrator.default_verifier().await,
        None,
        "削除した個体を指す既定は「なし」に読める"
    );

    say(&village, &village.coordinator, "撒いて").await;
    let from_c = Endpoint::Agent {
        id: village.coordinator.clone(),
    };
    let reply = said(orchestrator, &from_c, &Endpoint::User)
        .await
        .pop()
        .expect("進行役が答える");
    assert_eq!(reply.content, plain_bundle("vf_d_wa", "vf_d_wb"));
    assert!(is_agent(&reply.from, &village.coordinator));
}
