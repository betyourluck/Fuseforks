//! 入退室の通知に何が載るか（`compose_presence_notices` の射程）。
//!
//! **起点は実機の観察**（2026-08-08）— 広場ログを切ってある個体が、自分宛でない
//! 予定の依頼文を読めていた。原因は通知の抽出が `from == System` しか見ておらず、
//! **予定の配送（`from: System, to: Agent`）まで拾っていた**こと。
//!
//! 入退室の通知は**広場ログと違って `hears_room_log` でオプトアウトできない**
//! （設計どおり — 顔ぶれの変化は全員に届く必要がある）。そこへ宛先付きの発話が
//! 混ざると、**オプトアウトを迂回して他人宛の本文が配られる**。
//!
//! ここで留めるのは 2 点だけ:
//! - 予定の配送は**宛先の個体にだけ**届く
//! - 入退室の通知（`System → User`）は従来どおり全員に届く（**負の対照**）

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fuseforks_core::event::CoreEvent;
use fuseforks_core::llm::{ChatRequest, ChatResponse, Finish, LlmBackend, LlmError, Usage};
use fuseforks_core::model::{AgentId, AgentSpec, Endpoint, ModelTemplate};
use fuseforks_core::schedule::{Recurrence, ScheduleOptions};
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};
use tokio::sync::broadcast::Receiver;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-presence-{tag}-{}",
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

/// **プロンプト全体**（役割を問わず全メッセージ）を覚える差し込み。
///
/// `System` ブロックだけを見る probe では足りない — 可変文脈は
/// **最終 user 発話へ畳まれる**ので（`prompt_cache` #45）、
/// 入退室の通知は `Role::System` に載っていない。
#[derive(Default)]
struct FullPromptProbe {
    prompts: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl LlmBackend for FullPromptProbe {
    fn name(&self) -> &str {
        "full-prompt-probe"
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, LlmError> {
        let joined: String = req
            .messages
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n---\n");
        self.prompts.lock().unwrap().push(joined);

        Ok(ChatResponse {
            text: Some("了解".into()),
            tool_calls: Vec::new(),
            finish: Finish::Stop,
            usage: Usage {
                prompt: 1,
                completion: 1,
                cache_read: 0,
                reasoning: 0,
            },
            grounding: Default::default(),
            reasoning_summary: Vec::new(),
        })
    }
}

/// 一定時間静かになるまでイベントを集める。
///
/// **`quiet` は `stats_interval`（1 秒）より短くする** — 長くすると窓が原理的に
/// 閉じず、「失敗」ではなく「永久に返らない」として現れる（`failures.md` #86）。
async fn drain_until_quiet(rx: &mut Receiver<CoreEvent>) {
    while tokio::time::timeout(Duration::from_millis(400), rx.recv())
        .await
        .is_ok()
    {}
}

fn config() -> OrchestratorConfig {
    OrchestratorConfig {
        // ティッカーに勝手に走られると観測が揺れるので、tick は手動だけにする。
        schedule_interval: Duration::from_secs(3600),
        ..OrchestratorConfig::default()
    }
}

/// 予定の宛先 1 体と、**広場ログを切った傍観者** 1 体。線は引かない
/// （顔ぶれも `ask` も出ないので、漏れる経路が入退室の通知だけに絞られる）。
async fn setup(dir: &TempDir, backend: Arc<FullPromptProbe>) -> (Orchestrator, AgentId, AgentId) {
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::new(backend)),
        Arc::new(InMemorySecretStore::new()),
        config(),
    )
    .await
    .expect("bootstrap できること");
    // ホストの OS ロケールに依存させない（CI は en・開発機は ja — Spec 35 で
    // 言語がコアの挙動の入力になった。時計を引数で受けるのと同じ規律）。
    orchestrator.set_language(fuseforks_core::world::Language::Ja).await.unwrap();
    orchestrator
        .upsert_template(ModelTemplate::new("tpl", "既定", "mock-model"))
        .await
        .unwrap();

    let target = AgentId::from("agent_01");
    orchestrator
        .create_agent(AgentSpec::new(target.clone(), "宛先", "tpl"))
        .await
        .unwrap();
    orchestrator.start_agent(&target).await.unwrap();

    let bystander = AgentId::from("agent_02");
    let mut spec = AgentSpec::new(bystander.clone(), "傍観者", "tpl");
    spec.hears_room_log = false;
    orchestrator.create_agent(spec).await.unwrap();
    orchestrator.start_agent(&bystander).await.unwrap();

    (orchestrator, target, bystander)
}

/// 予定の依頼文は**宛先の個体にしか届かない**。
///
/// 通るのは 2 点で、**両方を 1 本で見る** — 正の対照（宛先には届く）が無いと、
/// 配送そのものが壊れた実装でも緑になる。
#[tokio::test]
async fn a_scheduled_request_does_not_leak_into_other_agents_prompts() {
    const SECRET: &str = "ジンジャーエールを 3 本";

    let dir = TempDir::new("leak");
    let backend = Arc::new(FullPromptProbe::default());
    let (orchestrator, target, bystander) = setup(&dir, Arc::clone(&backend)).await;
    let mut rx = orchestrator.subscribe();
    drain_until_quiet(&mut rx).await;

    orchestrator
        .create_schedule(
            target.clone(),
            SECRET.to_owned(),
            Recurrence::Interval { every_minutes: 1 },
            ScheduleOptions::default(),
        )
        .await
        .unwrap();
    // 発火して宛先へ配送させる（時刻は引数で渡す — 壁時計に依存させない）。
    orchestrator
        .run_schedule_tick(chrono::Local::now() + chrono::Duration::minutes(2))
        .await;
    drain_until_quiet(&mut rx).await;

    let after_delivery = backend.prompts.lock().unwrap().len();
    assert!(
        backend.prompts.lock().unwrap()[..after_delivery]
            .iter()
            .any(|prompt| prompt.contains(SECRET)),
        "正の対照: 宛先の個体には依頼文が届いていること"
    );

    // 傍観者へ、依頼文と無関係な発話を送る。
    orchestrator
        .send_user_message(&bystander, "こんばんは")
        .await
        .unwrap();
    drain_until_quiet(&mut rx).await;

    let prompts = backend.prompts.lock().unwrap();
    let bystander_prompts = &prompts[after_delivery..];
    assert!(
        !bystander_prompts.is_empty(),
        "傍観者のターンが 1 回は走っていること"
    );
    for prompt in bystander_prompts {
        assert!(
            !prompt.contains(SECRET),
            "広場ログを切った傍観者に、他人宛の予定の依頼文が届いている:\n{prompt}"
        );
    }
}

/// 入退室の通知（`System → User`）は**従来どおり全員へ届く**。
///
/// 上の絞り込みで**一緒に落としていないこと**を見る負の対照。ここが落ちると、
/// 「顔ぶれの変化は全員に届く」（`hears_room_log` の外側）が壊れる。
#[tokio::test]
async fn presence_notices_still_reach_everyone() {
    let dir = TempDir::new("presence");
    let backend = Arc::new(FullPromptProbe::default());
    let (orchestrator, target, bystander) = setup(&dir, Arc::clone(&backend)).await;
    let mut rx = orchestrator.subscribe();

    // 宛先を止めると System → User の通知が 1 件積まれる。
    orchestrator.stop_agent(&target).await.unwrap();
    drain_until_quiet(&mut rx).await;
    let before = backend.prompts.lock().unwrap().len();

    orchestrator
        .send_user_message(&bystander, "こんばんは")
        .await
        .unwrap();
    drain_until_quiet(&mut rx).await;

    let prompts = backend.prompts.lock().unwrap();
    assert!(
        prompts[before..]
            .iter()
            .any(|prompt| prompt.contains("停止しました")),
        "入退室の通知は広場ログを切った個体にも届くこと"
    );
    // 宛先が Endpoint::User であることが条件なので、通知が User 宛のまま
    // 届いていることも同時に確かめている（Agent 宛だけを落とす絞り込み）。
    let _ = Endpoint::User;
}
