//! 委譲の輪の検出と待ち時間（Spec 44）の結合テスト。
//!
//! 検査の軸は 3 つ — (1) 輪は**待たずに**即答拒否される（S2。旧実装は
//! `ask_timeout` × 深さぶん居座った） (2) 転送は末尾を除いて連鎖を運ぶので、
//! **転送後の逆向き委譲は通る**（S4 — 偽陽性の対照） (3) 時計は村の設定で、
//! 範囲外の保存は拒否される。
//!
//! **S4 の転送はツール非対応モデルの旧経路で起こす。** ツール経路では
//! 「委譲で呼ばれたターンに転送を提示しない」（#96 の門）ので、
//! 「ask された B が転送する」形は旧経路（`decide` の `tools_available=false`
//! 分岐 — 終了マーカーが無ければ最初の相手へ渡す・門を通らない）にだけ実在する
//! （P1 実装記録に詳細）。
//!
//! **診断の出口はプロセスで 1 つ**（`OnceLock`）なので、**ログを読むテストは
//! このファイルに 1 つだけ**（`a_mutual_ask_cycle_is_refused_instantly`）。
//!
//! バックエンドは本文の目印で進路を決める。**目印は入退室 System 行と
//! 衝突しない語にする** — 初版の「開始」は「稼働を**開始**しました」に部分一致し、
//! 可変文脈（#45 で最終 user 発話へ畳まれる）越しに誤発火した。
//!   「カクニンして」→ 本文で答える /
//!   「バトンして」+ ツール無し → 本文「タノム」（旧経路がこれを転送する）/
//!   「タノム」→ 最初の相手へ ask「カクニンして」/
//!   「リレー」→ 最初の相手へ ask「次へリレーして」（輪を作りに行く）/
//!   「キックオフ」→ 最初の相手へ ask「バトンして」/ それ以外 → 本文で答える

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fuseforks_core::llm::{ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, ToolCall, Usage};
use fuseforks_core::model::{AgentId, AgentSpec, ModelTemplate};
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-askcycle-{tag}-{}",
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

struct MarkerBackend;

impl MarkerBackend {
    fn ask_first(req: &ChatRequest, message: &str) -> Option<ChatResponse> {
        let tool = req.tools.iter().find(|t| t.name.starts_with("ask_"))?;
        Some(ok_response(
            "",
            vec![ToolCall {
                id: "call_ask".into(),
                name: tool.name.clone(),
                args: serde_json::json!({ "message": message }),
                extra: None,
            }],
        ))
    }
}

#[async_trait::async_trait]
impl LlmBackend for MarkerBackend {
    fn name(&self) -> &str {
        "marker"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let latest = req
            .messages
            .iter()
            .rev()
            .find_map(|m| (!m.content.is_empty()).then(|| m.content.clone()))
            .unwrap_or_default();

        if latest.contains("カクニンして") {
            return Ok(ok_response("元気です", Vec::new()));
        }
        if latest.contains("バトンして") && req.tools.is_empty() {
            // ツール非対応モデル。この本文が終了マーカー無しなので、旧経路が
            // そのまま最初の接続先へ**転送**する（decide の下段の分岐）。
            return Ok(ok_response("タノム", Vec::new()));
        }
        if latest.contains("タノム")
            && let Some(response) = Self::ask_first(&req, "カクニンして")
        {
            return Ok(response);
        }
        if latest.contains("リレー")
            && let Some(response) = Self::ask_first(&req, "次へリレーして")
        {
            return Ok(response);
        }
        if latest.contains("キックオフ")
            && let Some(response) = Self::ask_first(&req, "バトンして")
        {
            return Ok(response);
        }
        Ok(ok_response("答えです", Vec::new()))
    }
}

/// `agents[i]` は `agents[i+1]` へ接続（`wrap` なら最後は先頭へ戻る輪）。
/// `no_tools` に入れた個体はツール非対応テンプレート（旧経路）を使う。
async fn setup(
    tag: &str,
    agents: &[&str],
    wrap: bool,
    no_tools: &[&str],
) -> (TempDir, Orchestrator) {
    let dir = TempDir::new(tag);
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(Arc::new(MarkerBackend))),
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
    let mut legacy = ModelTemplate::new("tpl_notool", "旧経路", "mock-model");
    legacy.use_tools = false;
    orchestrator.upsert_template(legacy).await.unwrap();

    for id in agents {
        let template = if no_tools.contains(id) { "tpl_notool" } else { "tpl" };
        orchestrator
            .create_agent(AgentSpec::new(AgentId::from(*id), *id, template))
            .await
            .unwrap();
    }
    for (index, id) in agents.iter().enumerate() {
        let next = if index + 1 < agents.len() {
            Some(agents[index + 1])
        } else if wrap {
            Some(agents[0])
        } else {
            None
        };
        if let Some(next) = next {
            orchestrator
                .set_connections(&AgentId::from(*id), vec![AgentId::from(next)])
                .await
                .unwrap();
        }
        orchestrator.start_agent(&AgentId::from(*id)).await.unwrap();
    }
    (dir, orchestrator)
}

async fn drain(rx: &mut tokio::sync::broadcast::Receiver<fuseforks_core::event::CoreEvent>) {
    while tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .is_ok()
    {}
}

async fn sent_pairs(orchestrator: &Orchestrator) -> Vec<(String, String, String)> {
    orchestrator
        .message_log(None)
        .await
        .into_iter()
        .map(|m| (format!("{:?}", m.from), format!("{:?}", m.to), m.content))
        .collect()
}

/// S2 —「相互に委譲し合う 2 体」の輪が**待たずに**即答で拒否され、
/// デッドロックしない（**このファイルで唯一ログを読むテスト**）。
///
/// 旧実装ではこの形が `ask_timeout` × 深さぶん居座った（#106）。検出後は
/// 全体が ms 単位で完走する — drain の 500ms 窓で答えまで到達すること自体が
/// 「待っていない」の証拠になる。
#[tokio::test]
async fn a_mutual_ask_cycle_is_refused_instantly() {
    let dir = TempDir::new("log");
    let log_path = dir.0.join("fuseforks.log");
    fuseforks_core::open_log(&log_path).expect("ログを開けること");

    let (_dir, orchestrator) = setup("mutual", &["cyc_a", "cyc_b"], true, &[]).await;
    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&AgentId::from("cyc_a"), "リレーを試して")
        .await
        .unwrap();
    drain(&mut rx).await;

    let sent = sent_pairs(&orchestrator).await;
    // 正の対照: A の委譲は B へ届いている。
    assert!(
        sent.iter()
            .any(|(from, to, body)| from.contains("cyc_a")
                && to.contains("cyc_b")
                && body.contains("リレー")),
        "A から B への委譲は普通に配送されること: {sent:?}"
    );
    // 本題: B から A への委譲（輪）は**配送されない** — 会話にも残らない
    // （尋ねられていないので、記録すると広場ログが起きなかった配送を語る）。
    assert!(
        !sent
            .iter()
            .any(|(from, to, body)| from.contains("cyc_b")
                && to.contains("cyc_a")
                && body.contains("リレー")),
        "B から A への委譲（輪）は配送されてはいけない: {sent:?}"
    );
    // 拒否を読んだ B は自分で答え、答えは A へ戻り、A が利用者へ返す。
    assert!(
        sent.iter()
            .any(|(from, to, _)| from.contains("cyc_a") && to == "User"),
        "輪が拒否されても会話は完走すること: {sent:?}"
    );

    // 計器（D5）。連鎖の中身までログで読める。
    let body = std::fs::read_to_string(&log_path).expect("ログが読めること");
    assert!(
        body.contains("ask refused: agent=cyc_b to=cyc_a via=ask reason=circular")
            && body.contains("chain=[cyc_a,cyc_b]"),
        "拒否の計器が連鎖つきで出ること:\n{body}"
    );
}

/// 3 段の輪（A→B→C→A）も同じ網で止まり、会話は完走する。
#[tokio::test]
async fn a_three_hop_cycle_is_refused_at_the_closing_edge() {
    let (_dir, orchestrator) = setup("three", &["tri_a", "tri_b", "tri_c"], true, &[]).await;
    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&AgentId::from("tri_a"), "リレーを試して")
        .await
        .unwrap();
    drain(&mut rx).await;

    let sent = sent_pairs(&orchestrator).await;
    // 輪を閉じる辺（C→A）だけが配送されない。
    assert!(
        sent.iter()
            .any(|(from, to, body)| from.contains("tri_b")
                && to.contains("tri_c")
                && body.contains("リレー")),
        "B→C（輪の途中）は普通に配送されること: {sent:?}"
    );
    assert!(
        !sent
            .iter()
            .any(|(from, to, body)| from.contains("tri_c")
                && to.contains("tri_a")
                && body.contains("リレー")),
        "C→A（輪を閉じる辺）は配送されてはいけない: {sent:?}"
    );
    assert!(
        sent.iter()
            .any(|(from, to, _)| from.contains("tri_a") && to == "User"),
        "会話は完走すること: {sent:?}"
    );
}

/// S4 — 転送は末尾（直近の依頼主）の待ちを解くので、**転送後の逆向き委譲は
/// 拒否されない**（偽陽性の対照。これが無いと「常に拒否する実装」でも
/// 上の 2 本が緑になる）。
///
/// 形: user→A「キックオフ」→ A が B へ ask「バトンして」（連鎖 [A]）→
/// B は**ツール非対応**なので旧経路が本文「タノム」を C へ**転送**
/// （連鎖は末尾を除いて [] へ・`HandedOff` が A を解く）→ C が A へ
/// ask「カクニンして」（連鎖 [C]・A は居ない）→ **A が答える**。
#[tokio::test]
async fn an_ask_back_after_a_handoff_is_not_refused() {
    let (_dir, orchestrator) =
        setup("handoff", &["hnd_a", "hnd_b", "hnd_c"], true, &["hnd_b"]).await;
    let mut rx = orchestrator.subscribe();
    orchestrator
        .send_user_message(&AgentId::from("hnd_a"), "キックオフ")
        .await
        .unwrap();
    drain(&mut rx).await;

    let sent = sent_pairs(&orchestrator).await;
    // 前段の確認: B が C へ転送している（旧経路の handoff — 本文が渡る）。
    assert!(
        sent.iter()
            .any(|(from, to, body)| from.contains("hnd_b")
                && to.contains("hnd_c")
                && body.contains("タノム")),
        "B から C への転送（旧経路）が起きること: {sent:?}"
    );
    // 本題: C→A の委譲が**配送され**、A の答えが C へ戻っている。
    // 転送の末尾除去が無いと連鎖に A が残り、ここが拒否される（恒久の偽陽性）。
    assert!(
        sent.iter()
            .any(|(from, to, body)| from.contains("hnd_c")
                && to.contains("hnd_a")
                && body.contains("カクニンして")),
        "転送後の逆向き委譲（C→A）は拒否されず配送されること: {sent:?}"
    );
    assert!(
        sent.iter()
            .any(|(from, to, body)| from.contains("hnd_a")
                && to.contains("hnd_c")
                && body.contains("元気です")),
        "A の答えが C へ戻ること: {sent:?}"
    );
}

/// D4 — 時計は村の設定。範囲（30..=3600）の外は保存時に拒否され、
/// 範囲内と `None`（既定へ戻す）は保存できる。
#[tokio::test]
async fn ask_timeout_bounds_are_enforced_at_save_time() {
    let (_dir, orchestrator) = setup("bounds", &["bnd_a"], false, &[]).await;

    for invalid in [0u64, 10, 29, 3601, 86_400] {
        let err = orchestrator.set_ask_timeout(Some(invalid)).await.unwrap_err();
        assert!(
            matches!(err, fuseforks_core::error::CoreError::InvalidAskTimeout),
            "{invalid} 秒は拒否されること: {err:?}"
        );
    }
    for valid in [30u64, 600, 3600] {
        orchestrator.set_ask_timeout(Some(valid)).await.unwrap();
        assert_eq!(orchestrator.ask_timeout_secs().await, Some(valid));
    }
    // None = 既定（600 秒）へ戻す。
    orchestrator.set_ask_timeout(None).await.unwrap();
    assert_eq!(orchestrator.ask_timeout_secs().await, None);
}
