//! Spec 56（承認して続けさせる）の配線層の結合テスト。
//!
//! 確かめるのは**配送の副作用**: 由来の印・送り手・hop・言語の解決・
//! 配送できない相手の断り方。計器（`resume after approval:`）は
//! `resume_after_approval_log.rs` が別に持つ — 診断ログの宛先はプロセスで
//! 1 つなので、ログを読むテストは 1 ファイル 1 本にする。
//!
//! **承認そのものはここで検査しない**（Spec 20 の領分）。この経路は
//! `run.json` に 1 文字も触らないので、承認と配送が別の操作であることが
//! 「触っていない」という形で読める。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fuseforks_core::event::CoreEvent;
use fuseforks_core::model::{AgentId, AgentSpec, Endpoint, ModelTemplate};
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};
use tokio::sync::broadcast::Receiver;

/// テスト用の一時ディレクトリ。終了時に破棄する。
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-resume-{tag}-{}",
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

/// テンプレート 1 件だけ登録済みのオーケストレーターを組む。
///
/// 言語は明示する — ホストの OS ロケールに依存させない（Spec 35 で言語が
/// コアの挙動の入力になった。`failures.md` #101 の再演を避ける）。
async fn setup(dir: &TempDir, language: fuseforks_core::world::Language) -> Orchestrator {
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::echo("[echo]")),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig::default(),
    )
    .await
    .expect("bootstrap できること");
    orchestrator.set_language(language).await.unwrap();
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();
    orchestrator
}

/// 一定時間静かになるまでイベントを集める。
async fn drain_until_quiet(rx: &mut Receiver<CoreEvent>, quiet: Duration) -> Vec<CoreEvent> {
    let mut events = Vec::new();
    while let Ok(Ok(event)) = tokio::time::timeout(quiet, rx.recv()).await {
        events.push(event);
    }
    events
}

/// 発話イベントだけを抜き出す。
fn messages(events: &[CoreEvent]) -> Vec<&fuseforks_core::AgentMessage> {
    events
        .iter()
        .filter_map(|e| match e {
            CoreEvent::MessageSent { message } => Some(message),
            _ => None,
        })
        .collect()
}

/// 稼働中の 1 体を用意する。
async fn spawn_agent(orchestrator: &Orchestrator, id: &AgentId) {
    orchestrator
        .create_agent(AgentSpec::new(id.clone(), "ロボットくん", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(id).await.unwrap();
}

/// 1 回の操作で、由来の印つきの定型文が System から hop 0 で 1 通だけ届き、
/// ターンが 1 本起きる。
#[tokio::test]
async fn one_press_starts_one_turn_from_system() {
    let dir = TempDir::new("one");
    let orchestrator = setup(&dir, fuseforks_core::world::Language::Ja).await;
    let agent = AgentId::from("agent_01");
    spawn_agent(&orchestrator, &agent).await;

    let mut rx = orchestrator.subscribe();
    orchestrator.resume_after_approval(&agent).await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    let sent = messages(&events);

    let resumed: Vec<_> = sent
        .iter()
        .filter(|m| m.content.starts_with("【コマンド承認】"))
        .collect();
    assert_eq!(
        resumed.len(),
        1,
        "操作 1 回に対して配送は 1 通だけ: {sent:?}"
    );
    assert_eq!(resumed[0].hop, 0, "承認による続行は新しい因果の根なので hop 0");
    assert!(
        matches!(resumed[0].from, Endpoint::System),
        "送り手は System（利用者が書いた文ではないので User を名乗らない）"
    );
    assert!(
        resumed[0].content.contains("承認しました。続けてください。"),
        "定型文が届くこと: {}",
        resumed[0].content
    );
    // 元の依頼文は再送しない（Spec 56 D2）。定型文と印だけなので短いまま。
    assert_eq!(
        resumed[0].content.lines().count(),
        2,
        "由来の印 + 定型文の 2 行だけ: {}",
        resumed[0].content
    );

    // 続きが実際に走る — 応答が User へ返るのがターンが起きた証拠。
    assert!(
        sent.iter()
            .any(|m| m.to == Endpoint::User && m.content.contains("[echo]")),
        "続行でターンが 1 本起きること: {sent:?}"
    );
}

/// 英語の村では由来の印も英語。**`【】` は両言語で共通**（構造の印）。
#[tokio::test]
async fn the_origin_mark_follows_the_village_language() {
    let dir = TempDir::new("en");
    let orchestrator = setup(&dir, fuseforks_core::world::Language::En).await;
    let agent = AgentId::from("agent_01");
    spawn_agent(&orchestrator, &agent).await;

    let mut rx = orchestrator.subscribe();
    orchestrator.resume_after_approval(&agent).await.unwrap();
    let events = drain_until_quiet(&mut rx, Duration::from_millis(400)).await;
    let sent = messages(&events);

    let resumed: Vec<_> = sent
        .iter()
        .filter(|m| matches!(m.from, Endpoint::System))
        .collect();
    assert_eq!(resumed.len(), 1, "System からの配送が 1 通: {sent:?}");
    assert!(
        resumed[0].content.starts_with("【Command approval】"),
        "en の村では英語の印が出ること（言語の解決を写し忘れると \
         ja の印が出る）: {}",
        resumed[0].content
    );
    assert!(
        !resumed[0].content.contains("コマンド承認"),
        "ja の印が混ざらないこと: {}",
        resumed[0].content
    );
}

/// 停止中の個体へは配送しない。**稼働の判定は受信箱の有無**（既存の不変条件）
/// なので、`Idle` / `Failed` / `Stopping` のどれも同じ 1 本の門で断られる。
///
/// 画面の `disabled` は導線であって保証ではない — 画面の状態が古いまま
/// 押された競合を受けるのは、実行時のここだけ。
#[tokio::test]
async fn a_stopped_agent_is_refused_without_delivery() {
    let dir = TempDir::new("stopped");
    let orchestrator = setup(&dir, fuseforks_core::world::Language::Ja).await;
    let agent = AgentId::from("agent_01");
    orchestrator
        .create_agent(AgentSpec::new(agent.clone(), "ロボットくん", "tpl"))
        .await
        .unwrap();
    // start しない = 停止中（受信箱が無い）。

    let mut rx = orchestrator.subscribe();
    let err = orchestrator
        .resume_after_approval(&agent)
        .await
        .expect_err("停止中は断られること");
    assert_eq!(err.code(), "NOT_RUNNING", "既存のコードで返すこと: {err}");

    let events = drain_until_quiet(&mut rx, Duration::from_millis(300)).await;
    assert!(
        messages(&events).is_empty(),
        "断ったときは会話ログにも 1 行も残さない: {events:?}"
    );
}

/// 一度起動してから停止した個体も同じ門で断られる（受信箱が外れている）。
#[tokio::test]
async fn an_agent_stopped_after_running_is_refused() {
    let dir = TempDir::new("after");
    let orchestrator = setup(&dir, fuseforks_core::world::Language::Ja).await;
    let agent = AgentId::from("agent_01");
    spawn_agent(&orchestrator, &agent).await;
    orchestrator.stop_agent(&agent).await.unwrap();

    let err = orchestrator
        .resume_after_approval(&agent)
        .await
        .expect_err("停止後は断られること");
    assert_eq!(err.code(), "NOT_RUNNING", "{err}");
}

/// 削除された個体は「停止中」へ畳まない — 作り直すのと起動するのでは
/// 人の次の手が違う（`ask_external` の窓口と同じ分け方）。
#[tokio::test]
async fn a_missing_agent_is_not_folded_into_not_running() {
    let dir = TempDir::new("missing");
    let orchestrator = setup(&dir, fuseforks_core::world::Language::Ja).await;

    let err = orchestrator
        .resume_after_approval(&AgentId::from("agent_99"))
        .await
        .expect_err("居ない個体は断られること");
    assert_eq!(
        err.code(),
        "AGENT_NOT_FOUND",
        "停止中と別のコードで返すこと: {err}"
    );
}
