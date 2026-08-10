//! 外部の MCP クライアントからの依頼（Spec 25 P1）の結合テスト。
//!
//! 見るのは 3 層:
//!
//! 1. **入口の判断** — 窓口の未設定・削除・停止、そして同時 1 本のゲート（D7）
//! 2. **封筒** — `Endpoint::External` が `User` に化けないこと、壊れた名乗りが
//!    既定ラベルへ落ちること（D6）
//! 3. **ワイルドカード match の 6 箇所** — コンパイラが指さないので、
//!    `mcp_server_contract` 凍結 10 の挙動をここで凍結する

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_core::error::CoreError;
use agent_core::model::{AgentId, AgentSpec, Endpoint, ModelTemplate};
use agent_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};

/// テスト用の一時ディレクトリ。終了時に破棄する。
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-external-{tag}-{}-{}",
            std::process::id(),
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

async fn setup(dir: &TempDir) -> Orchestrator {
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::echo("[echo]")),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig::default(),
    )
    .await
    .expect("bootstrap できること");
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();
    orchestrator
}

/// 窓口を 1 体作って稼働させ、窓口として登録する。
async fn setup_with_reception(dir: &TempDir) -> (Orchestrator, AgentId) {
    let orchestrator = setup(dir).await;
    let id = AgentId::from("agent_desk");
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "窓口", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();
    orchestrator.set_reception(Some(&id)).await.unwrap();
    (orchestrator, id)
}

/// S1 — 窓口が受け取り、答えが戻り値として返る。
///
/// 併せて **S2**（依頼と答えが会話ログに残る）と **D6**（送り手が
/// `External` のまま `User` に化けない）を同じ 1 本で確かめる。
#[tokio::test]
async fn external_request_reaches_reception_and_answer_returns() {
    let dir = TempDir::new("answer");
    let (orchestrator, id) = setup_with_reception(&dir).await;

    let answer = orchestrator
        .ask_external("Claude Code", "村の様子を教えて")
        .await
        .expect("窓口が居るので通ること");

    assert!(answer.starts_with("[echo] "), "実際: {answer}");
    assert!(
        answer.ends_with("【送り手: Claude Code（外部クライアント）】\n村の様子を教えて"),
        "封筒に外部であることが載ること。実際: {answer}"
    );

    // S2 — 会話ログに依頼と答えの両方が残る。
    let log = orchestrator.message_log(None).await;
    let request = log
        .iter()
        .find(|m| matches!(m.from, Endpoint::External { .. }))
        .expect("依頼が External 発として残ること");
    assert_eq!(
        request.from,
        Endpoint::External {
            client: "Claude Code".to_owned()
        },
        "**User に畳まない**（D6）"
    );
    assert_eq!(request.to, Endpoint::Agent { id: id.clone() });
    assert_eq!(request.hop, 0, "外部依頼は新しい因果の根なので hop 0");

    let reply = log
        .iter()
        .find(|m| m.from == Endpoint::Agent { id: id.clone() })
        .expect("窓口の答えが残ること");
    assert!(
        matches!(reply.to, Endpoint::External { .. }),
        "答えの宛先は依頼元の外部クライアント。実際: {:?}",
        reply.to
    );
}

/// S5 — 窓口が未設定なら理由が返る。**扉は開いたまま**（ポートを閉じるのでは
/// なく、ツールが理由を返す = `mcp_server_contract` 凍結 7）。
#[tokio::test]
async fn unset_reception_is_reported_as_its_own_reason() {
    let dir = TempDir::new("unset");
    let orchestrator = setup(&dir).await;

    let err = orchestrator
        .ask_external("Claude Code", "やあ")
        .await
        .expect_err("窓口が未設定なら通らないこと");

    assert!(
        matches!(err, CoreError::ExternalReceptionUnset),
        "実際: {err:?}"
    );
    assert_eq!(err.code(), "EXTERNAL_RECEPTION_UNSET");
}

/// 窓口が**削除済み**なら「見つからない」を返す（「未設定」へ畳まない）。
///
/// 設定し直すのと初めて設定するのでは人の次の手が違うので、2 つの状態は
/// 別のエラーで返る。**削除時に窓口を掃除しない**ことの裏返しでもある。
#[tokio::test]
async fn deleted_reception_is_not_folded_into_unset() {
    let dir = TempDir::new("deleted");
    let (orchestrator, id) = setup_with_reception(&dir).await;
    orchestrator.stop_agent(&id).await.unwrap();
    orchestrator.delete_agent(&id).await.unwrap();

    let err = orchestrator
        .ask_external("Claude Code", "やあ")
        .await
        .expect_err("窓口が消えていれば通らないこと");

    assert_eq!(err.code(), "AGENT_NOT_FOUND", "実際: {err:?}");
}

/// S6 — 窓口が停止中なら、黙って待たずにその旨が返る。
#[tokio::test]
async fn stopped_reception_reports_instead_of_waiting() {
    let dir = TempDir::new("stopped");
    let (orchestrator, id) = setup_with_reception(&dir).await;
    orchestrator.stop_agent(&id).await.unwrap();

    let started = std::time::Instant::now();
    let err = orchestrator
        .ask_external("Claude Code", "やあ")
        .await
        .expect_err("停止中なら通らないこと");

    assert_eq!(err.code(), "NOT_RUNNING", "実際: {err:?}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "ask_timeout を待たずに返ること（実際: {:?}）",
        started.elapsed()
    );
}

/// D6 — `】` を含む名乗りが封筒を壊さない。**拒否ではなく既定ラベルへ落ちる**
/// （`clientInfo.name` は呼び出し側が対話的に直せない値なので、拒否すると
/// 扉ごと使えなくなる）。
#[tokio::test]
async fn malformed_client_name_falls_back_instead_of_rejecting() {
    let dir = TempDir::new("malformed");
    let (orchestrator, _id) = setup_with_reception(&dir).await;

    let answer = orchestrator
        .ask_external("こわれ】た\nクライアント", "やあ")
        .await
        .expect("**拒否しない** — 扉は開いたまま既定ラベルで通る");

    assert!(
        answer.ends_with("【送り手: external（外部クライアント）】\nやあ"),
        "封筒が 1 つに保たれること。実際: {answer}"
    );

    let log = orchestrator.message_log(None).await;
    let request = log
        .iter()
        .find(|m| matches!(m.from, Endpoint::External { .. }))
        .unwrap();
    assert_eq!(
        request.from,
        Endpoint::External {
            client: agent_core::world::DEFAULT_EXTERNAL_CLIENT.to_owned()
        },
        "記録にも正規化後の値だけが残ること（壊れた値を再放流しない）"
    );
}

/// 名乗りが上限を超えても落ちる先は同じ（字数はコードポイントで数える）。
#[tokio::test]
async fn overlong_client_name_falls_back() {
    let dir = TempDir::new("overlong");
    let (orchestrator, _id) = setup_with_reception(&dir).await;

    let long = "あ".repeat(agent_core::world::USER_NAME_MAX_CHARS + 1);
    let answer = orchestrator.ask_external(&long, "やあ").await.unwrap();

    assert!(
        answer.ends_with("【送り手: external（外部クライアント）】\nやあ"),
        "実際: {answer}"
    );
}

/// 合図が来るまで返事を止めるバックエンド。
///
/// **固定 sleep で「飛行中」を作らない** — 遅いマシンでは 1 本目が先に完走し、
/// テストは「たまたま順番に走った」を観測して緑になる（検査対象を検査しなく
/// なる典型）。合図で開ければ順序が構造で決まる。
struct GatedBackend {
    /// `chat` へ入ったことを test へ知らせる。
    entered: Arc<tokio::sync::Semaphore>,
    /// test が開けるまで `chat` を止める。
    release: Arc<tokio::sync::Semaphore>,
}

#[async_trait::async_trait]
impl agent_core::llm::LlmBackend for GatedBackend {
    fn name(&self) -> &str {
        "gated"
    }

    async fn chat(
        &self,
        _req: agent_core::llm::ChatRequest,
    ) -> Result<agent_core::llm::ChatResponse, agent_core::llm::LlmError> {
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        Ok(agent_core::llm::ChatResponse {
            text: Some("[gated] 答え".to_owned()),
            tool_calls: Vec::new(),
            finish: agent_core::llm::Finish::Stop,
            usage: agent_core::llm::Usage {
                prompt: 1,
                completion: 1,
                cache_read: 0,
                reasoning: 0,
            },
            grounding: agent_core::llm::Grounding::default(),
            reasoning_summary: Vec::new(),
        })
    }
}

/// D7 — 処理中にもう 1 本届いたら、待たせずに busy で返る（S7）。
///
/// **これが閉路とデッドロックの唯一の歯止め**。`max_hops` と予算の天井は
/// 扉を通るたびにリセットされるので効かない（外部入口は新しい因果の根で、
/// どちらも扉を通るたびに新品になる）。
#[tokio::test]
async fn second_external_request_is_refused_while_busy() {
    let dir = TempDir::new("busy");
    let entered = Arc::new(tokio::sync::Semaphore::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));

    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(Arc::new(GatedBackend {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        }))),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig::default(),
    )
    .await
    .unwrap();
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();
    let id = AgentId::from("agent_desk");
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "窓口", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();
    orchestrator.set_reception(Some(&id)).await.unwrap();
    let orchestrator = Arc::new(orchestrator);

    let first = {
        let orchestrator = Arc::clone(&orchestrator);
        tokio::spawn(async move { orchestrator.ask_external("A", "ゆっくり考えて").await })
    };

    // 1 本目が窓口のターンへ入る（= ゲートを握っている）ことを待つ。
    entered.acquire().await.unwrap().forget();

    let started = std::time::Instant::now();
    let err = orchestrator
        .ask_external("B", "割り込み")
        .await
        .expect_err("飛行中は 2 本目が通らないこと");
    assert_eq!(err.code(), "EXTERNAL_BUSY", "実際: {err:?}");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "**待たずに**返ること（待つと閉路のデッドロックが居座る）。実際: {:?}",
        started.elapsed()
    );

    // 1 本目を完走させる。
    release.add_permits(8);
    let answer = first.await.unwrap().expect("1 本目は普通に完走すること");
    assert!(answer.contains("[gated]"), "実際: {answer}");

    // ゲートは 1 本目の完了で開く（permit を握りっぱなしにしない）。
    let third = orchestrator.ask_external("C", "その後").await;
    assert!(third.is_ok(), "完了後は次が通ること: {third:?}");
}

/// `mcp_server_contract` 凍結 9 — 広場ログの可視性は**依頼と返答で違う**。
///
/// 依頼（`from=External`）は不可視、窓口の返答（`to=External`）は User 宛の
/// 返答と同じで可視。**可視述語は 1 文字も編集していない**（`from` が
/// `Agent` でない発話を既に落とす）ので、ここで凍結しておかないと
/// 親切心で分岐が足される。
#[test]
fn room_log_hides_external_requests_but_shows_replies() {
    use agent_core::model::AgentMessage;
    use agent_core::room_log::is_visible_in_room_log;

    let desk = AgentId::from("agent_desk");
    let other = AgentId::from("agent_other");
    let external = Endpoint::External {
        client: "Claude Code".to_owned(),
    };

    let request = AgentMessage::new(
        external.clone(),
        Endpoint::Agent { id: desk.clone() },
        "外からの依頼",
        0,
    );
    assert!(
        !is_visible_in_room_log(&other, &request),
        "外部の依頼は他のサーヴァントに見えない（User 発と同じ扱い）"
    );

    let reply = AgentMessage::new(
        Endpoint::Agent { id: desk.clone() },
        external,
        "窓口の答え",
        1,
    );
    assert!(
        is_visible_in_room_log(&other, &reply),
        "窓口が外へ返した答えは見える（User 宛の返答と同じ扱い）"
    );
    assert!(
        !is_visible_in_room_log(&desk, &reply),
        "自分の発話は自分の広場ログには載らない（既存の規律は不変）"
    );
}

/// `mcp_server_contract` 凍結 10 — **コンパイラが指さないワイルドカード
/// match の挙動**を凍結する。variant を足しても `_ =>` は黙って吸うので、
/// 「正しく吸っている」ことをテストでだけ確かめられる。
#[test]
fn wildcard_matches_treat_external_as_not_an_agent() {
    use agent_core::compute::count_by_sender;
    use agent_core::model::AgentMessage;

    let external = Endpoint::External {
        client: "Claude Code".to_owned(),
    };
    // `Endpoint::agent_id` — 外部クライアントはエージェントではない。
    assert_eq!(external.agent_id(), None);

    // `compute::count_by_sender` — 辺の太さの集計に外部は載らない
    // （利用者ノードを地図に出さないのと同じ側の判断）。
    let desk = AgentId::from("agent_desk");
    let log = vec![
        AgentMessage::new(
            external,
            Endpoint::Agent { id: desk.clone() },
            "外からの依頼",
            0,
        ),
        AgentMessage::new(
            Endpoint::Agent { id: desk.clone() },
            Endpoint::User,
            "答え",
            1,
        ),
    ];
    let counts = count_by_sender(&log);
    assert_eq!(counts.len(), 1, "外部発は 1 件も数えない");
    assert_eq!(counts.get(&desk), Some(&1));
}

/// 毎周ツール呼び出しを返し続けるバックエンド（周回を進めるためだけの道具）。
///
/// **提示されていないツール名を呼ぶ**ので実行はされないが、`tool_result` として
/// 返るので周は進む（L3 の経路）。ツールを 1 本も登録せずに「2 周目の周回境界」
/// を作れる — 予算の検査点はそこにしか無い。
struct LoopingBackend;

#[async_trait::async_trait]
impl agent_core::llm::LlmBackend for LoopingBackend {
    fn name(&self) -> &str {
        "looping"
    }

    async fn chat(
        &self,
        _req: agent_core::llm::ChatRequest,
    ) -> Result<agent_core::llm::ChatResponse, agent_core::llm::LlmError> {
        Ok(agent_core::llm::ChatResponse {
            text: Some(String::new()),
            tool_calls: vec![agent_core::llm::ToolCall {
                id: "call_1".into(),
                name: "not_presented".into(),
                args: serde_json::json!({}),
                extra: None,
            }],
            finish: agent_core::llm::Finish::ToolUse,
            usage: agent_core::llm::Usage {
                prompt: 1,
                completion: 1,
                cache_read: 0,
                reasoning: 0,
            },
            grounding: agent_core::llm::Grounding::default(),
            reasoning_summary: Vec::new(),
        })
    }
}

/// 予算（Spec 11）は外部依頼にも効く。**外部依頼は予算の根の 3 種類目**で、
/// 天井は毎回新品になる — その天井が実際に打ち切りへ使われることを見る。
///
/// **同時に「なぜ閉路を塞げないか」の裏取りにもなっている** — 天井が根ごとに
/// 新品ということは、扉を通るたびにリセットされるということ。
#[tokio::test]
async fn budget_ceiling_applies_to_external_requests() {
    let dir = TempDir::new("budget");
    // 周回境界でしか予算は検査されないので、**1 周で終わらない**バックエンドが要る。
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(Arc::new(LoopingBackend))),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig::default(),
    )
    .await
    .unwrap();
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();
    let id = AgentId::from("agent_desk");
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "窓口", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&id).await.unwrap();
    orchestrator.set_reception(Some(&id)).await.unwrap();
    orchestrator.set_token_budget(Some(5)).await.unwrap();

    let answer = orchestrator.ask_external("Claude Code", "調べて").await;
    assert!(answer.is_ok(), "扉自体は開く: {answer:?}");

    let log = orchestrator.message_log(None).await;
    let cut = log
        .iter()
        .any(|m| m.from == Endpoint::System && m.content.contains("実効 5 トークン"));
    assert!(cut, "天井 5 が外部依頼の打ち切りに使われること: {log:?}");

    // 2 本目も**同じ天井で新品から**始まる（前の依頼の消費を引き継がない）。
    let second = orchestrator.ask_external("Claude Code", "もう一度").await;
    assert!(
        second.is_ok(),
        "根が違えば天井も新品（= 天井は閉路を塞げない）: {second:?}"
    );
    drop(id);
}

/// `mcp_server_contract` 凍結 10 — 分岐の候補に外部依頼は出ない。
///
/// `session_store` の判定は `matches!(from, User)` の**ワイルドカード側**で、
/// variant を足してもコンパイラは指さない。分岐は「利用者がどう頼み直すか」の
/// 操作なので、外の LLM の依頼が候補に並ぶと候補の意味が崩れる。
#[tokio::test]
async fn external_requests_are_not_fork_points() {
    let dir = TempDir::new("fork");
    let (orchestrator, id) = setup_with_reception(&dir).await;

    orchestrator
        .ask_external("Claude Code", "外からの依頼")
        .await
        .unwrap();
    orchestrator
        .send_user_message(&id, "人からの依頼")
        .await
        .unwrap();
    // 人の依頼のターンが記録されるまで待つ（fork 候補は保存済みの発話から作る）。
    tokio::time::sleep(Duration::from_millis(300)).await;

    let sessions = orchestrator.list_sessions().await.unwrap();
    let current = sessions.first().expect("会話が 1 つはあること");
    let points = orchestrator.list_fork_points(&current.id).await.unwrap();

    assert!(
        points.iter().any(|p| p.text.contains("人からの依頼")),
        "利用者の依頼は候補に出ること: {points:?}"
    );
    assert!(
        !points.iter().any(|p| p.text.contains("外からの依頼")),
        "外部の依頼は候補に出ないこと: {points:?}"
    );
}

/// 窓口の設定は**書き込みの入口で**存在を確かめる（呼ばれるまで気づかない
/// 設定を作らせない）。予定の登録と同じ形。
#[tokio::test]
async fn setting_reception_to_unknown_agent_is_rejected() {
    let dir = TempDir::new("setreception");
    let orchestrator = setup(&dir).await;

    let err = orchestrator
        .set_reception(Some(&AgentId::from("agent_nope")))
        .await
        .expect_err("未登録は拒否されること");
    assert_eq!(err.code(), "AGENT_NOT_FOUND");
    assert_eq!(
        orchestrator.reception().await,
        None,
        "拒否したときは 1 バイトも変更しない"
    );
}

/// 外部クライアントの呼び名を設定すると、**封筒がその名前になる**（Spec 25）。
///
/// **見るのは表示ではなく入力。** `【送り手: X（外部クライアント）】` は
/// モデルが読むプロンプトの一部なので、設定が届いていなければ意味が無い。
#[tokio::test]
async fn configured_external_name_reaches_the_envelope() {
    let dir = TempDir::new("extname");
    let (orchestrator, _id) = setup_with_reception(&dir).await;
    orchestrator.set_external_name(Some("Neo")).await.unwrap();

    let answer = orchestrator.ask_external("Claude Code", "やあ").await.unwrap();

    assert!(
        answer.ends_with("【送り手: Neo（外部クライアント）】\nやあ"),
        "設定した呼び名が封筒に載ること。実際: {answer}"
    );
    assert!(
        !answer.contains("Claude Code"),
        "名乗りは封筒に出ないこと（設定が上書きする）。実際: {answer}"
    );

    // **記録には名乗りが残る。** 誰が実際に呼んだかは監査の材料で、
    // 呼び名はその表示規則にすぎない（`Endpoint::External` は据え置き）。
    let log = orchestrator.message_log(None).await;
    let request = log
        .iter()
        .find(|m| matches!(m.from, Endpoint::External { .. }))
        .unwrap();
    assert_eq!(
        request.from,
        Endpoint::External {
            client: "Claude Code".to_owned()
        },
        "記録は名乗りのまま（表示規則で上書きしない）"
    );
}

/// 未設定なら**名乗りへ落ちる**（既存の挙動を変えない）。
#[tokio::test]
async fn unset_external_name_falls_back_to_the_declared_name() {
    let dir = TempDir::new("extname-unset");
    let (orchestrator, _id) = setup_with_reception(&dir).await;
    assert_eq!(orchestrator.external_name().await, None);

    let answer = orchestrator.ask_external("Claude Code", "やあ").await.unwrap();
    assert!(
        answer.ends_with("【送り手: Claude Code（外部クライアント）】\nやあ"),
        "実際: {answer}"
    );
}

/// 呼び名の検査は**利用者の呼び名と同じ述語**を通る（封筒の制約が同じだから）。
#[tokio::test]
async fn external_name_is_validated_like_the_user_name() {
    let dir = TempDir::new("extname-invalid");
    let (orchestrator, _id) = setup_with_reception(&dir).await;

    let err = orchestrator
        .set_external_name(Some("こわれ】た"))
        .await
        .expect_err("封筒を壊す文字は拒否されること");
    assert_eq!(err.code(), "INVALID_USER_NAME", "実際: {err:?}");
    assert_eq!(
        orchestrator.external_name().await,
        None,
        "拒否したときは 1 バイトも変更しない"
    );
}

/// アイコンは**利用者とも役職とも別の置き場**（`{workspace}/external/`）。
///
/// 検証は同じ述語（`validate_icon`）を通る — 上限が 1 つであること自体が
/// 不変条件なので、ここで別の値を持たせない。
#[tokio::test]
async fn external_icon_has_its_own_folder_and_shares_the_predicate() {
    let dir = TempDir::new("exticon");
    let (orchestrator, _id) = setup_with_reception(&dir).await;

    assert_eq!(orchestrator.external_icon().await.unwrap(), None, "未設定は None");
    // 未設定でも削除は成功（冪等）。
    orchestrator.clear_external_icon().await.unwrap();

    // 最小の WebP（`validate_icon` が見るのはマジックバイト）。
    let webp = {
        let mut bytes = b"RIFF\x00\x00\x00\x00WEBPVP8 ".to_vec();
        bytes.extend_from_slice(&[0u8; 32]);
        bytes
    };
    orchestrator.set_external_icon(&webp).await.unwrap();
    assert_eq!(orchestrator.external_icon().await.unwrap(), Some(webp));
    // **利用者のアイコンは別物**（流用すると外の道具の依頼が自分の依頼に見える）。
    assert_eq!(orchestrator.user_icon().await.unwrap(), None);

    orchestrator.clear_external_icon().await.unwrap();
    assert_eq!(orchestrator.external_icon().await.unwrap(), None);
}
