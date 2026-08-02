//! オーケストレーター。エージェントのライフサイクルとメッセージ配送を担う。
//!
//! # 並行モデル
//!
//! - エージェント 1 体につき Tokio タスクが 1 つ。受信箱（`mpsc`）を待ち受ける。
//! - 停止は `watch` チャネルで通知し、`select!` で受信と競わせる。
//!   タスクを `abort` しないのは、LLM 呼び出しの途中で切ると
//!   課金済みの応答を捨てたうえに統計も更新されないため。
//! - 状態変化は `broadcast` で押し出す。購読者が居なくても送信は失敗しない。
//! - CPU バウンドな部分（RAG 検索・集計）だけが [`crate::compute::spawn_rayon`] 経由で
//!   Rayon プールへ逃げる。
//!
//! # 会話の終わり方（2 層）
//!
//! 主要フレームワーク（OpenAI Agents SDK / AutoGen / LangGraph）はいずれも、
//! 会話の終了を **意味的な終了**と**機械的な上限**の 2 層で持つ。
//! 片方だけでは足りない（failures.md #11）。
//!
//! ## 層 1: 意味的な終了 — モデルが決める
//!
//! OpenAI Agents SDK の規則を採る:
//! **ツール呼び出しの無いテキスト出力が最終出力**。
//!
//! 接続先を持つエージェントには [`HANDOFF_TOOL`] を提示する。
//! - ツールを呼んだ → 指定された相手へ転送し、会話が続く
//! - 本文だけを返した → **会話はそこで終わり**、ユーザーへ返る
//!
//! ツール呼び出しを実装しないサーバ向けには、終了マーカー
//! [`TERMINATION_MARKER`] を用意する（AutoGen v0.2 の `is_termination_msg` 同型）。
//!
//! ## 層 2: 機械的な上限 — 安全網
//!
//! 各発話は `hop` を持ち、[`OrchestratorConfig::max_hops`] に達した時点で連鎖を打ち切る。
//! これは LangGraph の `recursion_limit` と同じ位置づけで、**終わり方ではなく燃料切れ**。
//! 打ち切りは [`CoreEvent::HopLimitReached`] で通知する。黙って止めると
//! 「なぜ会話が終わったのか」が UI から永久に分からなくなる。
//!
//! ## 収束の前提: 履歴
//!
//! 終了条件より先に、収束の条件が要る。エージェントは直近 N 往復の履歴を持ち、
//! 自分の発言も `assistant` として見る。履歴が無いと毎回コールドスタートになり、
//! 同じ入力に同じ出力を返し続けて原理的に収束しない（failures.md #12）。

use std::collections::HashMap;
use std::collections::BTreeMap;
use std::sync::{Arc, Weak};
use std::time::Duration;

use tokio::sync::{Mutex, RwLock, broadcast, mpsc, watch};
use tokio::task::JoinHandle;

use crate::compute;
use crate::config_store::ConfigStore;
// 診断の 1 行はここを通す（stderr とログファイルの両方へ出る）。
use crate::note;
use crate::error::{CoreError, CoreResult, ErrorPayload};
use crate::event::CoreEvent;
use crate::llm::{
    BackendFactory, ChatMessage, ChatRequest, ChatResponse, LlmBackend, Role, ToolSpec,
};
use crate::model::{
    AgentId, AgentMessage, AgentSnapshot, AgentSpec, AgentStatus, ConfigFileKind, CredentialSource,
    Endpoint, ModelTemplate, ModelTemplateId, TopologyEdge,
};
use crate::plan::{PlanTaskAnnounced, PlanTaskState, PlanWaveRecord, PlanWaveStore};
use crate::rag::{RagChunk, RagIndex};
use crate::schedule::{Recurrence, ScheduledTask, Tick};
use crate::tool::{AgentTool, ToolContext, ToolRegistry};
use crate::secret::SecretStore;
use crate::world::{TopologyPosition, World};

/// 転送を要求するツールの名前。
///
/// このツールを呼ぶかどうかが、そのまま「会話を続けるか終えるか」の表明になる。
pub const HANDOFF_TOOL: &str = "handoff";

/// ツール呼び出しを実装しないサーバ向けの終了マーカー。
///
/// 本文の末尾にこれが現れたら、転送せず会話を終える。
/// AutoGen v0.2 の `is_termination_msg`（"TERMINATE" を含むか）と同型。
pub const TERMINATION_MARKER: &str = "[[END]]";

/// オーケストレーターの動作パラメータ。
#[derive(Debug, Clone)]
pub struct OrchestratorConfig {
    /// 1 つのユーザー入力から派生する転送の最大回数。
    ///
    /// **これは会話の終わり方ではなく安全網**。意味的な終了は
    /// [`HANDOFF_TOOL`] を呼ばないことで表明される。
    pub max_hops: u8,
    /// エージェントごとに保持する会話履歴の往復数。
    pub history_turns: usize,
    /// 1 回の発話処理で許すツール実行の回数。
    ///
    /// LangGraph の `recursion_limit` と同じ安全網。モデルが同じツールを
    /// 同じ引数で呼び続ける行き詰まりは実際に起きるので、上限が要る。
    /// その行き詰まりを上限より手前で切るのは [`RepeatGuard`] の側
    /// （回数の上限はコストの上限にならない。failures.md #41）。
    ///
    /// 既定 12。当初 6 だったが、**通常の調査委譲が 2 セッションで 3 回**
    /// この上限で溶けた（grep → 絞り込み → 読む、の往復は 6 では足りない）。
    /// 低い上限は節約ではなく浪費側に働く — 燃えたトークンの成果が出ないまま、
    /// 再依頼でもう一度同じだけ燃える（2026-07-30 実測）。
    pub max_tool_iterations: u8,
    /// エージェント 1 体あたりの受信箱容量。溢れたら送信側にエラーを返す（背圧）。
    pub mailbox_capacity: usize,
    /// イベントバッファの容量。UI の描画が遅れても、この範囲までは取りこぼさない。
    pub event_capacity: usize,
    /// 稼働統計を押し出す間隔。
    pub stats_interval: Duration,
    /// 予定の発火判定を回す間隔（Spec 07）。
    ///
    /// `stats_interval`（1 秒）とは**共用しない**。秒の予定は持たないので
    /// 30 秒で足りる — 「17:00 の予定が 17:00:29 に飛ぶ」は分単位の予定に
    /// とって十分な精度で、毎秒全予定を判定するのは無駄なだけ。
    pub schedule_interval: Duration,
    /// 保持するメッセージログの最大件数。超えた分は古いほうから捨てる。
    pub log_capacity: usize,
    /// プロンプトへ載せる「居合わせた会話」の件数（広場ログ）。
    ///
    /// 0 で無効。大きくすると場の見通しは良くなるが、**全エージェントの
    /// プロンプトが同じだけ膨らむ** — 人数分の乗算で効いてくるので、
    /// 会話の流れを追える最小限に留める。
    pub room_log_window: usize,
    /// 広場ログの 1 発話あたりの表示上限（文字数）。
    ///
    /// 長い発話 1 つでログ全体が埋まるのを防ぐ。要点だけ見えれば
    /// 「誰が何の話をしていたか」は伝わる。
    pub room_log_excerpt_chars: usize,
    /// 委譲（`ask_*`）で相手の答えを待つ上限。
    ///
    /// 委譲は相手の応答を**待ってブロックする**。相互に委譲し合う配置では
    /// 待ち合わせが起きうるので、必ず戻る上限が要る。`max_hops` は深さしか
    /// 縛らず、待ちの時間は縛らない。
    pub ask_timeout: Duration,
    /// 1 回の応答生成で RAG から引く断片数。
    pub rag_top_k: usize,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            max_hops: 8,
            history_turns: 8,
            max_tool_iterations: 12,
            mailbox_capacity: 64,
            event_capacity: 1_024,
            stats_interval: Duration::from_secs(1),
            schedule_interval: Duration::from_secs(30),
            log_capacity: 5_000,
            room_log_window: 12,
            room_log_excerpt_chars: 200,
            ask_timeout: Duration::from_secs(180),
            rag_top_k: 4,
        }
    }
}

/// 受信箱に入る 1 通。発話と、任意の返信路。
///
/// # なぜ返信路が要るのか
///
/// 転送（handoff）は制御ごと相手へ渡す機構で、相手の答えは**ユーザーへ**返る。
/// 電話の転送と同じで、渡した側は結果を知らない。だが「〇〇に聞いてきて」という
/// 依頼では、依頼主が答えを受け取って自分の話を続けたい。
/// この 2 つは別の機構であり、OpenAI Agents SDK も handoff と agent-as-tool を
/// 別に持っている。返信路の有無がその区別になる。
struct Envelope {
    /// 届いた発話。
    incoming: AgentMessage,
    /// 答えを返す先。`None` なら転送・通常配送（答えはユーザーへ）。
    reply_to: Option<tokio::sync::oneshot::Sender<Reply>>,
    /// 依頼元ターンのキャンセルの手掛かり（Spec 10 Phase 2）。
    ///
    /// `ask` / `plan` の配送だけが持つ（依頼元のターントークンの子）。
    /// 受信側はターン開始直後にこれを見て、キャンセル済みなら **LLM を
    /// 1 回も呼ばずに畳む**（出口 2b）。飛行中に切られる場合は、ここから
    /// さらに子を作った自ターンのトークンが周回境界で検知する — 親由来と
    /// 自分宛が 1 本に畳み込まれるので、検査箇所は増えない。
    cancel: Option<tokio_util::sync::CancellationToken>,
}

/// ask / plan の返信路の積み荷（Spec 08 で素の `String` から拡張）。
///
/// `kind` は [`handle_message`] の Finish / Handoff 分岐が刻む — 転送は文字列と
/// しては普通の答えと同じ経路で返るため、型で刻まないと区別できない。
/// 文言 parse では取らない（文言を直した瞬間に黙って壊れる）。
/// 分類の刻み手をこの 1 箇所に固定するため、data_contract で凍結している。
struct Reply {
    /// 依頼主が読む本文。
    text: String,
    /// 解決分類。`Answered`（Finish）か `HandedOff`（Handoff）のどちらか。
    kind: PlanTaskState,
}

impl Envelope {
    /// 返信を求めない通常の配送。ユーザー発話・転送・予定の発火が使う —
    /// どれも「依頼元ターン」を持たないので、キャンセルの手掛かりも持たない。
    fn plain(incoming: AgentMessage) -> Self {
        Self {
            incoming,
            reply_to: None,
            cancel: None,
        }
    }
}

/// タスク間で共有される状態。
struct Shared {
    world: RwLock<World>,
    /// 稼働中エージェントの受信箱。停止時に取り除かれるので、
    /// 「ここに居る = 送信できる」という不変条件が成り立つ。
    mailboxes: RwLock<HashMap<AgentId, mpsc::Sender<Envelope>>>,
    events: broadcast::Sender<CoreEvent>,
    factory: Arc<dyn BackendFactory>,
    /// テンプレート ID ごとの構築済みバックエンド。
    ///
    /// `reqwest::Client` は接続プールを抱えるので、発話ごとに作り直すと
    /// TLS ハンドシェイクをやり直すことになる。テンプレート更新時に破棄して整合を取る。
    backends: RwLock<HashMap<ModelTemplateId, Arc<dyn LlmBackend>>>,
    /// 秘密の保管先。設定ファイルとは別系統で、平文の `world.json` には触れない。
    secrets: Arc<dyn SecretStore>,
    store: ConfigStore,
    rag: RwLock<RagIndex>,
    log: RwLock<Vec<AgentMessage>>,
    /// 実行できるツール。同梱ツールと**共通** MCP サーバー由来のツールが同居する。
    tools: RwLock<ToolRegistry>,
    /// 接続中の共通 MCP サーバー。接続を保持し続けないと子プロセスが落ちる。
    mcp: RwLock<crate::mcp::McpManager>,
    /// エージェント別 MCP（Spec 02）。キーの有無 = 稼働中の個別接続の有無。
    ///
    /// 共有 registry には**入れない** — 入れると全員から見え、共通 MCP の
    /// 再接続（全入れ替え）とも衝突する。接続寿命はエージェントの稼働に
    /// 一致し、`stop_agent` は自分のエントリだけを畳む。状態は永続化しない
    /// （状態ファイルはプロセスが消えても「接続済み」を残して嘘をつく）。
    agent_mcp: RwLock<HashMap<AgentId, AgentMcpState>>,
    /// 時刻で発火する依頼（Spec 07）。**ここが唯一の所有者**で、
    /// `schedules.json` は常にこの内容の投影として書き出される。
    /// ticker（消化の記録）と UI（追加・削除）の書き手が 2 つあるため、
    /// ファイルを読み戻して書く形にすると片方の変更がもう片方に潰される。
    schedules: RwLock<Vec<ScheduledTask>>,
    /// plan 実行の観測記録（Spec 08 — 波ペイン）。リングバッファでプロセス寿命。
    /// ファイルへは書かない — 再起動生存は別 Spec の管轄。
    plan_waves: RwLock<PlanWaveStore>,
    /// 飛行中ターンの割り込みハンドル（Spec 10）。キーの有無 = 飛行中ターンの有無。
    ///
    /// `agent_loop` がターン開始時に入れ、終了時に**自分の seq を確かめてから**
    /// 外す（不変条件 6 — 割り込みの有効範囲はターン seq に束縛。エージェントに
    /// 紐づくフラグを置くと、ターン A への割り込みが直後のターン B へ漏れる）。
    /// 1 エージェント 1 飛行中ターン（不変条件 7 — mpsc の順次処理が根拠）なので
    /// エントリは高々 1 つ。
    turns: Mutex<HashMap<AgentId, Arc<TurnHandle>>>,
    /// ターンの通し番号の採番元。プロセス内で単調増加（エージェント間で共有 —
    /// 個々のエージェントから見ても単調なので seq 束縛の根拠には十分）。
    turn_seq: std::sync::atomic::AtomicU64,
    /// `schedules.json` 自体が JSON として読めなかったときの理由。
    ///
    /// この状態では予定の**書き込みを拒否する** — 読めなかったものを
    /// 上書きすると、利用者が直せば戻ったはずの予定を消すことになる。
    /// 起動は止めない（`mcp.json` と同じ判断: 直す画面へ到達できなくなる）。
    schedules_blocked: Option<String>,
    config: OrchestratorConfig,
}

/// スケジューラ層の実行時状態。**意図的に [`Shared`] の外に置く**（Spec 07 Notes 5）。
///
/// `Shared` に処理中フラグを足すと Spec 04 の進行役の状態管理と結合して
/// 複雑度が跳ねる。ここに居るのは「まだ働いている相手に積み増さない」ための
/// 軽いガードだけで、コアの正しさには関与しない。
#[derive(Default)]
struct ScheduleRuntime {
    /// 予定の配送先として現在ターン処理中のエージェント。
    ///
    /// 配送時に入れ、[`CoreEvent::AgentTyping`] の `active: false` で外す。
    /// イベントを取りこぼしたら**集合を空にする**（fail open）— 塞がったままに
    /// すると予定が二度と発火しない静かな停止になり、稀な二重発火より悪い。
    in_flight: Mutex<std::collections::HashSet<AgentId>>,
}

/// エージェント別 MCP の実行時状態（プロセス寿命）。
struct AgentMcpState {
    /// 接続本体。読み込み失敗時は空。
    manager: crate::mcp::McpManager,
    /// `mcp.json` の読み込み失敗（外部編集起因。失敗二分類 (1')）。
    load_error: Option<String>,
}

/// IPC へ返すエージェント別 MCP の状態。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMcpStatus {
    /// 稼働中（= 個別接続が存在する）か。停止中は未接続で、次回起動で繋がる。
    pub running: bool,
    /// `mcp.json` の読み込み失敗。`None` なら読めている（または未設定）。
    pub load_error: Option<String>,
    /// サーバー単位の接続状態（稼働中のみ。停止中は空）。
    pub servers: Vec<crate::mcp::McpServerStatus>,
}

impl Shared {
    /// イベントを押し出す。購読者が居なければ黙って捨てる。
    fn emit(&self, event: CoreEvent) {
        let _ = self.events.send(event);
    }

    /// ログへ追記し、[`CoreEvent::MessageSent`] を発行する。
    async fn record(&self, message: AgentMessage) {
        {
            let mut log = self.log.write().await;
            if log.len() >= self.config.log_capacity {
                // 先頭から捨てる。容量に達したあと無制限に伸ばすと、
                // 長時間の稼働でメモリを食い潰す。
                let overflow = log.len() + 1 - self.config.log_capacity;
                log.drain(..overflow);
            }
            log.push(message.clone());
        }
        self.emit(CoreEvent::MessageSent { message });
    }

    /// テンプレートに対応するバックエンドを取り出す。無ければ組み立てて覚える。
    ///
    /// 設定不備で代替へ退避した場合は [`CoreEvent::BackendDegraded`] で必ず通知し、
    /// **その結果を覚えない**。覚えてしまうと、原因を直しても次の発話で復帰できず、
    /// テンプレートを保存し直すまで偽の応答が続く。
    async fn backend_for(&self, template: &ModelTemplate) -> CoreResult<Arc<dyn LlmBackend>> {
        if let Some(backend) = self.backends.read().await.get(&template.id) {
            return Ok(Arc::clone(backend));
        }

        let resolution = self.factory.create(template)?;

        if let Some(reason) = resolution.degraded_reason {
            self.emit(CoreEvent::BackendDegraded {
                model_template_id: template.id.clone(),
                reason,
            });
            return Ok(resolution.backend);
        }

        self.backends
            .write()
            .await
            .insert(template.id.clone(), Arc::clone(&resolution.backend));
        Ok(resolution.backend)
    }

    /// 状態を変更し、変化があった場合のみイベントを発行する。
    ///
    /// **入退室の通知（Spec 06 P1）もここから出す。** 状態遷移はこの関数しか
    /// 通らない（単一路）ので、境界判定を呼び出し側へ散らさずに済む。
    async fn set_status(&self, id: &AgentId, status: AgentStatus) {
        // 変化の判定と、通知に要る材料（旧状態が稼働側だったか・表示名）を
        // 同じロックの中で取る。ロックの外で読み直すと、連続する遷移と
        // 競合して「変わった瞬間の名前」ではなくなる。
        let changed = {
            let mut world = self.world.write().await;
            match world.agent_mut(id) {
                Ok(record) if record.status != status => {
                    let was_running = record.status == AgentStatus::Running;
                    record.status = status;
                    Some((was_running, record.spec.name.clone()))
                }
                _ => None,
            }
        };
        let Some((was_running, name)) = changed else {
            return;
        };

        self.emit(CoreEvent::AgentStatusChanged {
            agent_id: id.clone(),
            status,
        });

        // 入退室の通知。Running / それ以外の境界をまたいだときだけ 1 件。
        // 起動失敗（Starting → Failed）はどちらも非稼働側なので出ない —
        // まだ場に現れていなかった者の失敗は入退室ではなく、進行役に
        // 「居ると思っていた」という誤った信念も発生していない（顔ぶれが示す）。
        let is_running = status == AgentStatus::Running;
        if was_running != is_running {
            // 語彙は状態語彙で統一する（オンライン/オフライン等のチャット風
            // 語彙を混ぜると、同じ状態に 2 つの言葉が並びモデルが対応を推測する）。
            // Failed だけ種別を伝える — 理由（last_error）は流さないが、
            // 失敗と正常停止の区別は進行役の次の一手を変える。
            let text = if is_running {
                format!("{id}（{name}）が稼働を開始しました")
            } else if status == AgentStatus::Failed {
                format!("{id}（{name}）が失敗により停止しました")
            } else {
                format!("{id}（{name}）が停止しました")
            };
            // from: System / to: User。User 宛の発話は全エージェントにとって
            // 他人の会話なので、広場ログの is_mine 判定に誰も掛からず全員に
            // 見える。配送は起きない（record のみ。ターンを発火させない）。
            self.record(AgentMessage::new(Endpoint::System, Endpoint::User, text, 0))
                .await;
        }
    }
}

/// 飛行中ターン 1 つぶんの割り込みハンドル（Spec 10）。
///
/// Phase 1 ではトークンは親を持たない単独生成。Phase 2 で封筒のトークンから
/// `child_token` で導出する形に変わる（親由来と自分宛が 1 トークンに畳み込まれ、
/// 検査は 1 本で足りる — 2 本を別々に見る設計は見忘れの席になるので採らない）。
struct TurnHandle {
    /// ターンの通し番号。割り込みの有効範囲はこの seq に束縛される。
    seq: u64,
    /// このターンの協調的キャンセル。検査点は周回境界だけ（契約の不変条件 1）。
    token: tokio_util::sync::CancellationToken,
    /// 最初の割り込み要求の時刻。System 行の「要求から N 秒」の起点。
    ///
    /// 2 回目以降の要求では上書きしない（利用者が連打しても、計測は
    /// 最初に押した瞬間から）。`std::sync::Mutex` なのは await を跨がないため。
    requested_at: std::sync::Mutex<Option<std::time::Instant>>,
}

/// 稼働中エージェントのタスク制御。
struct TaskHandle {
    shutdown: watch::Sender<bool>,
    join: JoinHandle<()>,
}

/// マルチエージェント・オーケストレーター。
///
/// GUI 層はこの型のメソッドだけを呼ぶ。内部の並行制御は外へ漏れない。
pub struct Orchestrator {
    shared: Arc<Shared>,
    tasks: Mutex<HashMap<AgentId, TaskHandle>>,
    stats_task: JoinHandle<()>,
    /// スケジューラ層の実行時状態（Spec 07）。ticker タスクと共有する。
    schedule_runtime: Arc<ScheduleRuntime>,
    schedule_task: JoinHandle<()>,
}

impl Orchestrator {
    /// ワークスペースから状態を復元してオーケストレーターを起動する。
    ///
    /// エージェントは復元しても自動起動しない。起動は明示操作に限る
    /// （アプリを開いた瞬間に全エージェントが課金を始めるのを避ける）。
    pub async fn bootstrap(
        store: ConfigStore,
        factory: Arc<dyn BackendFactory>,
        secrets: Arc<dyn SecretStore>,
        config: OrchestratorConfig,
    ) -> CoreResult<Self> {
        let persisted = store.load_world().await?;
        let mut world = World::from_persisted(persisted.clone());

        // 「unset なのに秘密が実在する」テンプレートは keyring へ昇格させる。
        // clear_credential は秘密の削除と unset への遷移を一体で行うので、
        // この組み合わせは正規の操作では作れない——過去の巻き戻り事故（failures.md #16）が
        // ディスクへ固定された状態である。放置するとユーザーはキーを貼り直すまで
        // 接続できず、しかも ⚙ の画面は「登録済み」と表示する（矛盾が見えない）。
        // 資格情報ストアが応答しない場合は昇格を見送る。起動を止めるほどの事態ではなく、
        // 次回起動時にまた試せばよい。
        for mut template in world.templates() {
            if template.credential == CredentialSource::Unset
                && secrets.contains(template.id.as_str()).unwrap_or(false)
            {
                template.credential = CredentialSource::Keyring;
                world.upsert_template(template);
            }
        }

        // 読み込み時の正規化（平文の秘密の除去、宙に浮いた接続の切り離し）で内容が
        // 変わったなら、その場でファイルへ書き戻す。次の編集操作まで待つと、
        // ユーザーが何もしない限りディスク上に古い内容——場合によっては秘密——が残る。
        let normalized = world.to_persisted();
        if normalized != persisted {
            store.save_world(&normalized).await?;
        }

        // 予定の読み込み（Spec 07）。ファイル全体が読めない場合も起動は止めない
        // （設定を直す画面へ到達できなくなる。mcp.json と同じ判断）が、
        // 書き込みは拒否する — 上書きすると直せば戻ったはずの予定が消える。
        let (schedules, schedules_blocked) = match store.load_schedules().await {
            Ok(loaded) => {
                for reason in &loaded.dropped {
                    note!("schedule: {reason}");
                }
                // 宛先が存在しない予定はここで落とす（World::from_persisted が
                // 宙に浮いた接続を落とすのと同じ規律）。ディスクへの反映は
                // 次の保存に任せる — 起動時に書き戻すほどの緊急性（秘密の残留）が無い。
                let (kept, dangling): (Vec<_>, Vec<_>) = loaded
                    .tasks
                    .into_iter()
                    .partition(|task| world.agent(&task.to).is_ok());
                for task in &dangling {
                    note!(
                        "schedule: 宛先 {} が存在しないため予定 {} を落としました",
                        task.to, task.id
                    );
                }
                (kept, None)
            }
            Err(err) => {
                note!(
                    "schedule: schedules.json が読めないため予定なしで起動します\
                     （書き込みは保護のため拒否されます）: {err}"
                );
                (Vec::new(), Some(err.to_string()))
            }
        };

        let (events, _) = broadcast::channel(config.event_capacity);

        let shared = Arc::new(Shared {
            world: RwLock::new(world),
            mailboxes: RwLock::new(HashMap::new()),
            events,
            factory,
            backends: RwLock::new(HashMap::new()),
            secrets,
            store,
            rag: RwLock::new(RagIndex::default()),
            log: RwLock::new(Vec::new()),
            tools: RwLock::new(ToolRegistry::new()),
            mcp: RwLock::new(crate::mcp::McpManager::default()),
            agent_mcp: RwLock::new(HashMap::new()),
            schedules: RwLock::new(schedules),
            plan_waves: RwLock::new(PlanWaveStore::default()),
            turns: Mutex::new(HashMap::new()),
            turn_seq: std::sync::atomic::AtomicU64::new(1),
            schedules_blocked,
            config,
        });

        let stats_task = spawn_stats_ticker(Arc::downgrade(&shared));
        let schedule_runtime = Arc::new(ScheduleRuntime::default());
        let schedule_task = spawn_schedule_ticker(
            Arc::downgrade(&shared),
            Arc::clone(&schedule_runtime),
            shared.events.subscribe(),
        );

        Ok(Self {
            shared,
            tasks: Mutex::new(HashMap::new()),
            stats_task,
            schedule_runtime,
            schedule_task,
        })
    }

    /// 状態変化の購読を開始する。
    pub fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.shared.events.subscribe()
    }

    // ---- 参照系 -------------------------------------------------------------

    /// 表示順に並んだ全エージェントの現在像。
    pub async fn snapshots(&self) -> Vec<AgentSnapshot> {
        self.shared.world.read().await.snapshots()
    }

    /// 単一エージェントの現在像。
    pub async fn snapshot(&self, id: &AgentId) -> CoreResult<AgentSnapshot> {
        self.shared.world.read().await.snapshot(id)
    }

    /// トポロジーの全辺。
    pub async fn edges(&self) -> Vec<TopologyEdge> {
        self.shared.world.read().await.edges()
    }

    /// 登録済みモデルテンプレート。
    pub async fn templates(&self) -> Vec<ModelTemplate> {
        self.shared.world.read().await.templates()
    }

    /// 会話をリセットする（新規チャット。Spec 03）。
    ///
    /// 消すのは `Shared.log` と各エージェントの `history` の**2 つだけ** —
    /// 稼働状態・累積統計・Memory.md・エージェント別 MCP 接続はすべて維持
    /// する（リセットするのは「会話」であって「エージェント」ではない）。
    ///
    /// 処理順は契約で固定: log クリア → history クリア → イベント発行。
    /// 飛行中のターンの完了書き込みは**許容**する — 白紙化の直後に飛行中
    /// だった発話 1 件が載るのは仕様（発話は起きた事実であり、ログに残す。
    /// hop 打ち切りの「記録してから打ち切る」と同じ規律）。
    pub async fn reset_conversation(&self) {
        self.shared.log.write().await.clear();
        self.shared.world.write().await.clear_histories();
        self.shared.emit(CoreEvent::ConversationCleared);
    }

    /// メッセージログ。`limit` を指定すると末尾からその件数だけ返す。
    pub async fn message_log(&self, limit: Option<usize>) -> Vec<AgentMessage> {
        let log = self.shared.log.read().await;
        match limit {
            Some(n) if n < log.len() => log[log.len() - n..].to_vec(),
            _ => log.clone(),
        }
    }

    /// plan 波の記録（Spec 08 — 波ペイン）。古い順・**実行中の波も含む**。
    ///
    /// 完了だけを返すと、再読み込みの瞬間に走っていた波が event でしか届かず
    /// 再投影の穴になる。フロントの突き合わせ規律（リスナー登録 → list →
    /// planId upsert）は data_contract の projection_rule が正。
    pub async fn list_plan_waves(&self) -> Vec<PlanWaveRecord> {
        self.shared.plan_waves.read().await.list()
    }

    /// エージェント別トークン消費量を集計する。
    ///
    /// 集計は Rayon プールで走るため、ログが数万件でも UI の IPC 応答は詰まらない。
    pub async fn token_usage_by_agent(&self) -> CoreResult<HashMap<AgentId, u64>> {
        let log = self.shared.log.read().await.clone();
        compute::spawn_rayon(move || compute::aggregate_token_usage(&log)).await
    }

    /// RAG 索引を検索する（診断・プレビュー用）。
    pub async fn search_rag(
        &self,
        sources: &[String],
        query: &str,
        k: usize,
    ) -> CoreResult<Vec<RagChunk>> {
        let hits = self.shared.rag.read().await.search(sources, query, k).await?;
        Ok(hits.into_iter().map(|h| h.item).collect())
    }

    /// RAG 索引に断片を追加する。
    pub async fn index_rag_chunk(&self, chunk: RagChunk) {
        self.shared.rag.write().await.insert(chunk);
    }

    /// 登録済み RAG ソース名。
    pub async fn rag_sources(&self) -> Vec<String> {
        self.shared.rag.read().await.sources()
    }

    /// ツールを登録する。同名は置き換える。
    ///
    /// 登録済みバックエンドのキャッシュとは無関係なので、稼働中でも足せる。
    /// 次の発話から提示される。
    pub async fn register_tool(&self, tool: Arc<dyn AgentTool>) {
        self.shared.tools.write().await.register(tool);
    }

    /// 登録済みツール名。
    pub async fn tool_names(&self) -> Vec<String> {
        self.shared
            .tools
            .read()
            .await
            .names()
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    // ---- 定義の編集 ---------------------------------------------------------

    /// エージェントを登録する。
    pub async fn create_agent(&self, spec: AgentSpec) -> CoreResult<AgentSnapshot> {
        let id = spec.id.clone();
        {
            let mut world = self.shared.world.write().await;
            world.register_agent(spec)?;
        }
        self.persist().await?;
        self.shared.emit(CoreEvent::TopologyChanged);
        self.snapshot(&id).await
    }

    /// エージェント定義を差し替える。
    ///
    /// 稼働中でも受け付ける。次の発話から新しい設定が反映される
    /// （プロンプトはメッセージごとに組み直すため）。
    pub async fn update_agent(&self, spec: AgentSpec) -> CoreResult<AgentSnapshot> {
        let id = spec.id.clone();
        {
            let mut world = self.shared.world.write().await;
            world.update_agent(spec)?;
        }
        self.persist().await?;
        self.shared.emit(CoreEvent::TopologyChanged);
        self.snapshot(&id).await
    }

    /// エージェントを削除する。稼働中なら先に停止する。
    pub async fn delete_agent(&self, id: &AgentId) -> CoreResult<()> {
        self.stop_agent(id).await.ok();
        {
            let mut world = self.shared.world.write().await;
            world.remove_agent(id)?;
        }
        self.shared.store.remove_agent_dir(id).await?;
        self.persist().await?;

        // その宛先の予定も消す（Spec 07。remove_agent が他エージェントからの
        // 参照を外すのと同じ規律 — 参照の回収まで含めて 1 操作）。
        // schedules.json が壊れて書き込み保護中でも削除自体は止めない:
        // in-memory から消せば発火は起きず、ファイルの残骸は保護解除後の
        // 次の保存で消える。
        {
            let mut schedules = self.shared.schedules.write().await;
            let before = schedules.len();
            schedules.retain(|task| task.to != *id);
            if schedules.len() != before && self.shared.schedules_blocked.is_none() {
                self.shared.store.save_schedules(&schedules).await?;
            }
        }

        self.shared.emit(CoreEvent::TopologyChanged);
        Ok(())
    }

    /// 接続先を差し替える。
    pub async fn set_connections(&self, id: &AgentId, targets: Vec<AgentId>) -> CoreResult<()> {
        {
            let mut world = self.shared.world.write().await;
            world.set_connections(id, targets)?;
        }
        self.persist().await?;
        self.shared.emit(CoreEvent::TopologyChanged);
        Ok(())
    }

    /// 表示順を振り直す。
    pub async fn reorder_agents(&self, order: &[AgentId]) -> CoreResult<()> {
        self.shared.world.write().await.reorder(order);
        self.persist().await
    }

    /// 接続マップの保存済みノード座標を返す。
    pub async fn topology_positions(&self) -> BTreeMap<AgentId, TopologyPosition> {
        self.shared.world.read().await.topology_positions()
    }

    /// 接続マップ上で移動したノードの座標を保存する。
    pub async fn set_topology_position(
        &self,
        id: &AgentId,
        position: TopologyPosition,
    ) -> CoreResult<()> {
        self.shared
            .world
            .write()
            .await
            .set_topology_position(id, position)?;
        self.persist().await?;
        self.shared.emit(CoreEvent::TopologyChanged);
        Ok(())
    }

    /// モデルテンプレートを登録または更新する。
    ///
    /// 構築済みバックエンドのキャッシュを破棄する。これを怠ると、
    /// エンドポイントを直したのに古い接続先へ送り続ける（設定が効かない）。
    pub async fn upsert_template(&self, mut template: ModelTemplate) -> CoreResult<()> {
        let id = template.id.clone();
        {
            let mut world = self.shared.world.write().await;

            // `credential` はコアが所有する派生状態。正当な遷移経路は
            // `set_credential` / `clear_credential` と、認証不要チェックボックス由来の
            // unset ⇄ not_required だけ。クライアントの下書きは登録前の古い
            // スナップショットを保持しうるので、ここで素通しにすると
            // 「キーは資格情報ストアに実在するのに設定上は未登録」へ巻き戻る
            // （Gemini テンプレートで実際に起きた。failures.md #16）。
            template.credential = match (
                world.template(&id).map(|t| t.credential).ok(),
                template.credential,
            ) {
                // keyring からの離脱は clear_credential（秘密の削除と一体）に限る。
                (Some(CredentialSource::Keyring), _) => CredentialSource::Keyring,
                // 秘密の裏付けが無い keyring 主張は未登録へ引き戻す。素通しにすると、
                // 保存時に捕まえられる設定不備が送信時の「見つかりません」へずれ込む。
                (previous, CredentialSource::Keyring) => {
                    if self.shared.secrets.contains(id.as_str()).unwrap_or(false) {
                        CredentialSource::Keyring
                    } else {
                        previous.unwrap_or(CredentialSource::Unset)
                    }
                }
                // unset ⇄ not_required はチェックボックスの正当な遷移。
                (_, requested) => requested,
            };

            world.upsert_template(template);
        }
        self.shared.backends.write().await.remove(&id);
        self.persist().await
    }

    /// モデルテンプレートを削除する。参照中のエージェントが居れば拒否される。
    ///
    /// 資格情報ストアの登録も同時に消す。設定だけ消して秘密を残すと、
    /// 画面のどこからも見えない孤児が OS 側に溜まり続ける。
    pub async fn remove_template(&self, id: &ModelTemplateId) -> CoreResult<()> {
        {
            let mut world = self.shared.world.write().await;
            world.remove_template(id)?;
        }
        self.shared.backends.write().await.remove(id);
        self.shared.secrets.delete(id.as_str())?;
        self.persist().await
    }

    // ---- 資格情報 -----------------------------------------------------------

    /// テンプレートの API キーを OS の資格情報ストアへ登録する。
    ///
    /// 併せてテンプレートの取得元を [`CredentialSource::Keyring`] に切り替え、
    /// 構築済みバックエンドのキャッシュを捨てる。登録したのに次の発話まで
    /// 反映されない、という状態を作らないため。
    pub async fn set_credential(&self, id: &ModelTemplateId, secret: &str) -> CoreResult<()> {
        // 貼り付け由来の前後空白・改行を落とす。正当な API キーの先頭・末尾に
        // 空白が含まれることはなく、混入すると送信時の 401 (Invalid token 等)
        // としてしか表面化しない — 登録時に吸収するのが唯一気づける場所。
        let secret = secret.trim();
        {
            // 存在しないテンプレートに対して秘密を書き込ませない。
            let world = self.shared.world.read().await;
            world.template(id)?;
        }
        self.shared.secrets.set(id.as_str(), secret)?;

        {
            let mut world = self.shared.world.write().await;
            let mut template = world.template(id)?.clone();
            template.credential = CredentialSource::Keyring;
            world.upsert_template(template);
        }
        self.shared.backends.write().await.remove(id);
        self.persist().await
    }

    /// テンプレートの API キーを資格情報ストアから削除する。
    ///
    /// 取得元は「未設定」へ戻す。「認証不要」へ落とすと、キーを消しただけの
    /// テンプレートが認証ヘッダ無しで外部へ送られるようになる。
    pub async fn clear_credential(&self, id: &ModelTemplateId) -> CoreResult<()> {
        self.shared.secrets.delete(id.as_str())?;

        {
            let mut world = self.shared.world.write().await;
            if let Ok(existing) = world.template(id) {
                let mut template = existing.clone();
                template.credential = CredentialSource::Unset;
                world.upsert_template(template);
            }
        }
        self.shared.backends.write().await.remove(id);
        self.persist().await
    }

    /// API キーが登録済みかどうかだけを返す。**値は返さない。**
    pub fn has_credential(&self, id: &ModelTemplateId) -> CoreResult<bool> {
        self.shared.secrets.contains(id.as_str())
    }

    /// 設定ファイルを読む。
    pub async fn read_config(&self, id: &AgentId, kind: ConfigFileKind) -> CoreResult<String> {
        // 未登録エージェントのファイルを読めてしまわないよう存在確認を先に行う。
        self.shared.world.read().await.agent(id)?;
        self.shared.store.read_config(id, kind).await
    }

    // ---- MCP -----------------------------------------------------------------

    /// `mcp.json` の宣言を読む。
    pub async fn mcp_config(&self) -> CoreResult<crate::mcp::McpConfig> {
        self.shared.store.read_mcp_config().await
    }

    /// `mcp.json` を書き、その場で接続し直す。
    pub async fn set_mcp_config(&self, config: &crate::mcp::McpConfig) -> CoreResult<()> {
        self.shared.store.write_mcp_config(config).await?;
        self.reload_mcp().await
    }

    /// MCP サーバーへ接続し直し、ツール登録簿を入れ替える。
    ///
    /// 1 台の失敗で全体を止めない（[`crate::mcp::McpManager::connect_all`]）。
    /// 各サーバーの結果は [`Orchestrator::mcp_statuses`] で読める。
    ///
    /// # Errors
    /// `mcp.json` が壊れている場合。**空として扱わない** — 書き間違えた瞬間に
    /// 全ツールが黙って消えると、利用者は原因に辿り着けない。
    pub async fn reload_mcp(&self) -> CoreResult<()> {
        let config = self.shared.store.read_mcp_config().await?;
        let next = crate::mcp::McpManager::connect_all(&config).await;

        // 古い接続のツールを先に外す。消さずに新しいものを登録すると、
        // 繋がっていないサーバーのツールがモデルへ提示され続ける。
        let previous = {
            let mut slot = self.shared.mcp.write().await;
            std::mem::replace(&mut *slot, next)
        };
        {
            let mut registry = self.shared.tools.write().await;
            for tool in previous.tools() {
                registry.unregister(tool.name());
            }
            let current = self.shared.mcp.read().await;
            for tool in current.tools() {
                registry.register(Arc::clone(tool));
            }
        }
        // 旧接続は登録簿から外し終えてから畳む（畳む間も古い呼び出しは来ない）。
        previous.shutdown().await;
        Ok(())
    }

    /// 各 MCP サーバーの接続状態。UI へそのまま出せる。
    pub async fn mcp_statuses(&self) -> Vec<crate::mcp::McpServerStatus> {
        self.shared.mcp.read().await.statuses().to_vec()
    }

    /// エージェント別 MCP の状態（Spec 02）。
    ///
    /// 停止中は「未接続」としか答えられない — 接続はエージェントの稼働に
    /// 紐付き、状態は永続化しない（嘘をつく状態ファイルを持たない）。
    pub async fn agent_mcp_status(&self, id: &AgentId) -> CoreResult<AgentMcpStatus> {
        self.shared.world.read().await.agent(id)?;
        let map = self.shared.agent_mcp.read().await;
        Ok(match map.get(id) {
            Some(state) => AgentMcpStatus {
                running: true,
                load_error: state.load_error.clone(),
                servers: state.manager.statuses().to_vec(),
            },
            None => AgentMcpStatus {
                running: false,
                load_error: None,
                servers: Vec::new(),
            },
        })
    }

    // ---- 村の条例 -------------------------------------------------------------

    /// 村の条例（全エージェント共通の規則）を読む。未設定なら空文字。
    pub async fn read_ordinance(&self) -> CoreResult<String> {
        self.shared.store.read_ordinance().await
    }

    /// 村の条例を書く。次の発話からすべてのエージェントに反映される
    /// （プロンプトはメッセージごとに組み直すため、再起動は不要）。
    pub async fn write_ordinance(&self, content: &str) -> CoreResult<()> {
        self.shared.store.write_ordinance(content).await
    }

    // ---- 村の黒板 -------------------------------------------------------------

    /// 村の黒板（work_dir の `黒板/`）を読む。GUI 投影用・読み取り専用。
    ///
    /// 対象は登録エージェントの work_dir（先に見つかった順・重複除去）。
    /// 条例の運用は共通 work_dir が前提だが、複数の work_dir が混在していても
    /// 全部読み、[`crate::blackboard::BlackboardNote::dir`] で区別できる形で返す。
    pub async fn read_blackboard(&self) -> CoreResult<Vec<crate::blackboard::BlackboardNote>> {
        let mut dirs: Vec<String> = Vec::new();
        for snapshot in self.snapshots().await {
            if let Some(dir) = snapshot.work_dir {
                if !dirs.contains(&dir) {
                    dirs.push(dir);
                }
            }
        }

        let mut notes = Vec::new();
        for dir in dirs {
            notes.extend(
                crate::blackboard::read_blackboard_dir(std::path::Path::new(&dir)).await?,
            );
        }
        Ok(notes)
    }

    // ---- アイコン -------------------------------------------------------------

    /// エージェントのアイコン（WebP バイト列）を読む。未設定なら `None`。
    pub async fn agent_icon(&self, id: &AgentId) -> CoreResult<Option<Vec<u8>>> {
        self.shared.world.read().await.agent(id)?;
        self.shared.store.read_icon(id).await
    }

    /// エージェントのアイコンを設定する。中身の検証（WebP・サイズ上限）は store が担う。
    pub async fn set_agent_icon(&self, id: &AgentId, bytes: &[u8]) -> CoreResult<()> {
        self.shared.world.read().await.agent(id)?;
        self.shared.store.write_icon(id, bytes).await
    }

    /// エージェントのアイコンを削除する。
    pub async fn clear_agent_icon(&self, id: &AgentId) -> CoreResult<()> {
        self.shared.world.read().await.agent(id)?;
        self.shared.store.delete_icon(id).await
    }

    /// 設定ファイルを書く。
    ///
    /// `mcp.json` の保存で、そのエージェントが**稼働中なら**個別接続を
    /// 張り直す（Spec 02）。停止中は検証つきの保存だけで、次回起動で反映。
    pub async fn write_config(
        &self,
        id: &AgentId,
        kind: ConfigFileKind,
        content: &str,
    ) -> CoreResult<()> {
        self.shared.world.read().await.agent(id)?;
        self.shared.store.write_config(id, kind, content).await?;

        if kind == ConfigFileKind::Mcp {
            let running = self.tasks.lock().await.contains_key(id);
            if running {
                if let Some(old) = self.shared.agent_mcp.write().await.remove(id) {
                    old.manager.shutdown().await;
                }
                connect_agent_mcp(&self.shared, id).await;
            }
        }
        Ok(())
    }

    /// 登録簿を永続化する。
    pub async fn persist(&self) -> CoreResult<()> {
        let persisted = self.shared.world.read().await.to_persisted();
        self.shared.store.save_world(&persisted).await
    }

    // ---- ライフサイクル -----------------------------------------------------

    /// エージェントを起動する。
    ///
    /// # Errors
    /// - 未登録なら [`CoreError::AgentNotFound`]
    /// - 既に稼働中なら [`CoreError::AlreadyRunning`]
    pub async fn start_agent(&self, id: &AgentId) -> CoreResult<()> {
        let mut tasks = self.tasks.lock().await;
        if tasks.contains_key(id) {
            return Err(CoreError::AlreadyRunning {
                agent_id: id.to_string(),
            });
        }

        {
            // 起動前に定義とテンプレートの整合を確認する。
            // 起動してから最初の発話で落ちるより、ここで断るほうが原因が分かりやすい。
            let world = self.shared.world.read().await;
            let record = world.agent(id)?;
            world.template(&record.spec.model_template_id)?;
        }

        self.shared.set_status(id, AgentStatus::Starting).await;

        // エージェント別 MCP を接続する（Spec 02）。接続寿命は稼働に一致。
        // 読み込み失敗・接続失敗でも起動は止めない — 状態として保持され、
        // agent_mcp_status で読める（共通 MCP と同じ規律）。
        connect_agent_mcp(&self.shared, id).await;

        let (mailbox_tx, mailbox_rx) = mpsc::channel(self.shared.config.mailbox_capacity);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        // 受信箱を先に公開してからタスクを起こす。逆順だと、起動直後の
        // 送信が「稼働中なのに宛先が無い」で落ちる窓ができる。
        self.shared
            .mailboxes
            .write()
            .await
            .insert(id.clone(), mailbox_tx);

        {
            let mut world = self.shared.world.write().await;
            if let Ok(record) = world.agent_mut(id) {
                record.started_at = Some(std::time::Instant::now());
                record.last_error = None;
                // 起動は新しい会話の開始として扱う。前回の文脈を引き継ぐと
                // 「始め直したつもりが続きだった」という分かりにくい状態になる。
                record.history.clear();
            }
        }

        let shared = Arc::clone(&self.shared);
        let agent_id = id.clone();
        let join = tokio::spawn(async move {
            agent_loop(agent_id, mailbox_rx, shutdown_rx, shared).await;
        });

        tasks.insert(
            id.clone(),
            TaskHandle {
                shutdown: shutdown_tx,
                join,
            },
        );
        drop(tasks);

        self.shared.set_status(id, AgentStatus::Running).await;
        Ok(())
    }

    /// 飛行中のターンを協調的に打ち切る（Spec 10）。
    ///
    /// 切るのは**ターン**であってエージェントではない — 稼働は降ろさず、
    /// 会話も履歴も消えず、次の封筒は普通に処理される。検知は周回境界
    /// （次の LLM 呼び出しの前）なので、飛行中の呼び出し・実行中のツールは
    /// 完走してから止まる。
    ///
    /// 飛行中のターンが無ければ**何もしない**（出口 2c — 「今の仕事を止めて」に
    /// 仕事が無いのは成功であって失敗ではない。エラーも通知も出さない）。
    /// 冪等 — 二重に呼んでも計測の起点（最初の要求時刻）は動かず、
    /// 出口の行も 1 本のまま（書くのは切られたターン自身なので）。
    pub async fn interrupt_turn(&self, id: &AgentId) {
        let handle = {
            let turns = self.shared.turns.lock().await;
            turns.get(id).map(Arc::clone)
        };
        let Some(handle) = handle else { return };

        // 計測の起点は最初の要求。連打で上書きすると「要求から N 秒」が縮んで、
        // 検知の遅さ（Notes 2 の判断材料）が実際より小さく記録される。
        {
            let mut requested = handle.requested_at.lock().expect("await を跨がない");
            if requested.is_none() {
                *requested = Some(std::time::Instant::now());
            }
        }
        handle.token.cancel();
        note!("interrupt requested: agent={id} seq={}", handle.seq);
    }

    /// 村の飛行中ターンを全部打ち切る（Spec 10 P4）。
    ///
    /// [`Orchestrator::interrupt_turn`] を全員へ適用するだけの薄い皮 —
    /// **for 文であること自体が仕様**（独自の機構・独自の重複排除を持たない）。
    /// 冪等: 飛行中ターンが 1 つも無くても成功。進行役とワーカーが親子で
    /// 二重に切られても、出口の行は各ターンが検知時に 1 回書くだけなので
    /// 重複しない。
    pub async fn interrupt_all(&self) {
        let ids: Vec<AgentId> = self.shared.turns.lock().await.keys().cloned().collect();
        for id in ids {
            self.interrupt_turn(&id).await;
        }
    }

    /// エージェントを停止する。処理中の発話は完了を待つ。
    ///
    /// # Errors
    /// 稼働していない場合 [`CoreError::NotRunning`]。
    pub async fn stop_agent(&self, id: &AgentId) -> CoreResult<()> {
        let handle = {
            let mut tasks = self.tasks.lock().await;
            tasks.remove(id).ok_or_else(|| CoreError::NotRunning {
                agent_id: id.to_string(),
            })?
        };

        // 飛行中のターンへ**最初に**割り込みを立てる（Spec 10 P5）。これが無いと、
        // 長いツールループの完走を下の join で最大 30 秒待つ。ステータスは
        // 不変条件 4 の但し書き側 — Running へ戻さず Stopping → Idle へ進む
        // （finish_interrupted はステータスに触れないので衝突しない）。
        // Stopping の通知より前に立てるのは順序の保証のため — 通知を見た側
        // （UI・テスト）が「割り込みはもう立っている」に依存できる。
        // 30 秒の上限は割り込みが効かない異常系の網としてそのまま残す。
        self.interrupt_turn(id).await;

        self.shared.set_status(id, AgentStatus::Stopping).await;
        // 受信箱を先に外し、停止処理中に新しい発話が積まれないようにする。
        self.shared.mailboxes.write().await.remove(id);

        let _ = handle.shutdown.send(true);
        // 処理中の LLM 呼び出しが終わるのを待つ。無限には待たない。
        if tokio::time::timeout(Duration::from_secs(30), handle.join)
            .await
            .is_err()
        {
            // タイムアウトしてもタスクは自走を続けるが、受信箱は既に外れているので
            // 次のループで停止する。ここで abort しないのは前掲の理由による。
        }

        // 個別 MCP を畳む（**自分のエントリだけ**。同じコマンドを使う他
        // エージェントのプロセスは別 spawn なので巻き添えにならない）。
        if let Some(state) = self.shared.agent_mcp.write().await.remove(id) {
            state.manager.shutdown().await;
        }

        {
            let mut world = self.shared.world.write().await;
            if let Ok(record) = world.agent_mut(id) {
                if let Some(started) = record.started_at.take() {
                    record.accumulated_uptime_secs += started.elapsed().as_secs();
                }
            }
        }
        self.shared.set_status(id, AgentStatus::Idle).await;
        Ok(())
    }

    /// 全エージェントを停止する。アプリ終了時に呼ぶ。
    pub async fn shutdown(&self) {
        let ids: Vec<AgentId> = self.tasks.lock().await.keys().cloned().collect();
        for id in ids {
            let _ = self.stop_agent(&id).await;
        }
    }

    // ---- 配送 ---------------------------------------------------------------

    /// ユーザー発話をエージェントへ投入する。
    ///
    /// # Errors
    /// - 宛先が稼働していない場合 [`CoreError::NotRunning`]
    /// - 受信箱が飽和している場合 [`CoreError::MailboxFull`]
    pub async fn send_user_message(&self, to: &AgentId, content: &str) -> CoreResult<()> {
        self.send_user_message_broadcast(to, content, &[]).await
    }

    /// ユーザー発話を**同報の 1 通として**エージェントへ投入する。
    ///
    /// `co_recipients` は同報の全宛先（受信者自身を含む）。UI は宛先ごとに
    /// このメソッドを呼び、毎回同じリストを渡す。受信者のプロンプトには
    /// 「全員が既に受け取っている」という注記が入り、転送する理由を消す
    /// （同報の反響防止）。**宛先外のエージェントへは何も送られない** —
    /// 同報の存在自体、宛先本人たちしか知らない。
    ///
    /// 2 体未満のリストは単独宛と同義なので、注記は付かない。
    pub async fn send_user_message_broadcast(
        &self,
        to: &AgentId,
        content: &str,
        co_recipients: &[AgentId],
    ) -> CoreResult<()> {
        let mut message = AgentMessage::new(
            Endpoint::User,
            Endpoint::Agent { id: to.clone() },
            content,
            0,
        );
        if co_recipients.len() >= 2 {
            message.co_recipients = co_recipients.to_vec();
        }
        self.shared.record(message.clone()).await;
        deliver(&self.shared, to, message).await
    }

    // ---- 予定（Spec 07） -----------------------------------------------------

    /// 登録済みの予定（登録順）。
    pub async fn schedules(&self) -> Vec<ScheduledTask> {
        self.shared.schedules.read().await.clone()
    }

    /// 予定を登録する。
    ///
    /// # Errors
    /// - 再現規則が不正な場合 [`CoreError::InvalidSchedule`]
    /// - 宛先が未登録の場合 [`CoreError::AgentNotFound`]
    /// - `schedules.json` が壊れていて書き込みが保護されている場合
    ///   [`CoreError::ScheduleStoreBlocked`]
    pub async fn create_schedule(
        &self,
        to: AgentId,
        message: String,
        recurrence: Recurrence,
    ) -> CoreResult<ScheduledTask> {
        self.ensure_schedules_writable()?;
        recurrence
            .validate()
            .map_err(|err| CoreError::InvalidSchedule {
                reason: err.to_string(),
            })?;
        // 宛先の存在確認。停止中は許す（発火時に飛ばす規則が受け止める）が、
        // 未登録は登録の時点で弾く — 発火するまで誰も気づかない予定を作らせない。
        self.shared.world.read().await.agent(&to)?;

        let task = ScheduledTask {
            id: uuid::Uuid::new_v4().to_string(),
            to,
            message,
            recurrence,
            created_at_ms: crate::model::now_ms(),
            last_consumed_due_ms: None,
            enabled: true,
        };

        let mut schedules = self.shared.schedules.write().await;
        schedules.push(task.clone());
        // 書き込みロックを持ったまま保存する。保存を外に出すと、並んだ 2 つの
        // 変更が互いの内容を tmp ファイルで踏み合う（world.json には無い事情 —
        // あちらの書き手は UI だけだが、こちらは ticker と UI の 2 系統ある）。
        self.shared.store.save_schedules(&schedules).await?;
        Ok(task)
    }

    /// 予定を削除する。
    ///
    /// # Errors
    /// - 該当 ID が無い場合 [`CoreError::ScheduleNotFound`]
    pub async fn delete_schedule(&self, id: &str) -> CoreResult<()> {
        self.ensure_schedules_writable()?;
        let mut schedules = self.shared.schedules.write().await;
        let before = schedules.len();
        schedules.retain(|task| task.id != id);
        if schedules.len() == before {
            return Err(CoreError::ScheduleNotFound(id.to_owned()));
        }
        self.shared.store.save_schedules(&schedules).await
    }

    /// 予定の一時停止・再開（Spec 07 の `enabled`）。
    ///
    /// # Errors
    /// - 該当 ID が無い場合 [`CoreError::ScheduleNotFound`]
    pub async fn set_schedule_enabled(&self, id: &str, enabled: bool) -> CoreResult<()> {
        self.ensure_schedules_writable()?;
        let mut schedules = self.shared.schedules.write().await;
        let task = schedules
            .iter_mut()
            .find(|task| task.id == id)
            .ok_or_else(|| CoreError::ScheduleNotFound(id.to_owned()))?;
        task.enabled = enabled;
        self.shared.store.save_schedules(&schedules).await
    }

    /// 予定の発火判定を 1 回実行する。
    ///
    /// 通常はティッカーが `Local::now()` で呼ぶ。**時刻を引数に取るのは
    /// テストのため**（壁時計に依存するテストを書かない — Spec 04 の規律）。
    pub async fn run_schedule_tick<Tz: chrono::TimeZone>(&self, now: chrono::DateTime<Tz>) {
        schedule_tick(&self.shared, &self.schedule_runtime, now).await;
    }

    /// `schedules.json` が読めない状態での書き込みを拒否する。
    fn ensure_schedules_writable(&self) -> CoreResult<()> {
        match &self.shared.schedules_blocked {
            Some(reason) => Err(CoreError::ScheduleStoreBlocked {
                reason: reason.clone(),
            }),
            None => Ok(()),
        }
    }
}

impl Drop for Orchestrator {
    fn drop(&mut self) {
        // 統計ティッカーは純粋な副作用タスクなので、ここは abort でよい。
        self.stats_task.abort();
        // 予定ティッカーも同じ。消化の永続化は tick 単位で完結しており、
        // tick の途中で切っても「消化したのに発火していない」は起きない
        // （消化の書き込みは配送成功の後）。
        self.schedule_task.abort();
    }
}

/// 稼働統計を定期的に押し出すタスクを起こす。
///
/// `Weak` を握るのは、このタスクが [`Orchestrator`] の生存を延ばさないようにするため。
/// `Arc` を持たせると、オーケストレーターを捨ててもティッカーが動き続ける。
fn spawn_stats_ticker(shared: Weak<Shared>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let interval = match shared.upgrade() {
            Some(s) => s.config.stats_interval,
            None => return,
        };
        let mut ticker = tokio::time::interval(interval);

        loop {
            ticker.tick().await;
            let Some(shared) = shared.upgrade() else {
                return;
            };

            let world = shared.world.read().await;
            for snapshot in world.snapshots() {
                if snapshot.status.is_active() {
                    shared.emit(CoreEvent::AgentStatsUpdated {
                        agent_id: snapshot.id,
                        uptime_secs: snapshot.uptime_secs,
                        total_tokens: snapshot.total_tokens,
                        prompt_tokens: snapshot.prompt_tokens,
                        cached_tokens: snapshot.cached_tokens,
                    });
                }
            }
        }
    })
}

/// 予定の発火判定を回すタスクを起こす（Spec 07）。
///
/// [`spawn_stats_ticker`] と同じく `Weak` を握る。加えてイベント購読を持ち、
/// [`CoreEvent::AgentTyping`] の `active: false` で二重発火ガードの集合から
/// 相手を外す — tick を待たずに処理するので、ガードの解除が最大 30 秒
/// 遅れることはない。
///
/// 最初の tick は 1 間隔ぶん待ってから。`tokio::time::interval` の既定は
/// 即時発火で、起動の瞬間（フロントの覆いがまだ出ている間）に予定が走るのは
/// 誰も見ていない発火になる。
fn spawn_schedule_ticker(
    shared: Weak<Shared>,
    runtime: Arc<ScheduleRuntime>,
    mut events: broadcast::Receiver<CoreEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let interval = match shared.upgrade() {
            Some(s) => s.config.schedule_interval,
            None => return,
        };
        let mut ticker =
            tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
        // 眠っていた PC が起きた直後に溜まった tick を連射しない。
        // 発火規則は「now 以前の直近の予定時刻」を毎回求めるので、
        // tick を密に打ち直しても同じ判定を繰り返すだけになる。
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    let Some(shared) = shared.upgrade() else { break };
                    schedule_tick(&shared, &runtime, chrono::Local::now()).await;
                }
                event = events.recv() => match event {
                    Ok(CoreEvent::AgentTyping { agent_id, active: false }) => {
                        runtime.in_flight.lock().await.remove(&agent_id);
                    }
                    Ok(_) => {}
                    // 取りこぼしたら fail open（集合を空にする）。塞がったままに
                    // すると予定が二度と発火しない静かな停止になり、
                    // 稀な二重発火より悪い（Spec 07 Notes 5）。
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        runtime.in_flight.lock().await.clear();
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
            }
        }
    })
}

/// 予定 1 巡ぶんの判定と実行（Spec 07 の配線層）。
///
/// 判定そのものは [`ScheduledTask::decide`]（純関数）に委ね、ここは
/// **副作用の順序**だけを持つ: 配送 → 記録 → 消化 → 保存。
/// 消化の書き込みが配送成功の後にあるので、途中で落ちても
/// 「消化したのに発火していない」は起きない（逆の「発火したのに消化が
/// 残っていない」は再発火として現れ、既知の制限に含まれる）。
async fn schedule_tick<Tz: chrono::TimeZone>(
    shared: &Arc<Shared>,
    runtime: &ScheduleRuntime,
    now: chrono::DateTime<Tz>,
) {
    let tasks: Vec<ScheduledTask> = shared.schedules.read().await.clone();
    let mut consumed: Vec<(String, u64)> = Vec::new();

    for task in tasks {
        match task.decide(&now) {
            Tick::Idle => {}
            Tick::Consume { due_ms } => {
                // 猶予超過。debug ログのみ — 「閉じていた」の事後報告は直す手が
                // 無く、数日分まとめて会話ログへ流すと本物の通知が埋まる。
                note!(
                    "schedule: {}（{}）の予定時刻を猶予超過で消化（発火せず）",
                    task.id,
                    task.recurrence.label_ja()
                );
                consumed.push((task.id.clone(), due_ms));
            }
            Tick::Fire { due_ms } => {
                let running = shared.mailboxes.read().await.contains_key(&task.to);
                if !running {
                    // 停止中へは撒かない。消化して、会話ログへ 1 行だけ残す
                    // （消化するのでログも 1 回だけになる）。
                    let name = {
                        let world = shared.world.read().await;
                        world
                            .agent(&task.to)
                            .map(|record| record.spec.name.clone())
                            // 宛先が削除済みでも通知は成立させる（ID で示す）。
                            .unwrap_or_else(|_| task.to.to_string())
                    };
                    shared
                        .record(AgentMessage::new(
                            Endpoint::System,
                            Endpoint::User,
                            format!(
                                "{}（{name}）への予定「{}」を飛ばしました（停止中）",
                                task.to,
                                task.recurrence.label_ja()
                            ),
                            0,
                        ))
                        .await;
                    consumed.push((task.id.clone(), due_ms));
                    continue;
                }

                // まだ働いている相手に積み増さない（二重発火の軽い護り）。
                // 消化しないので次の tick で再判定される — 壁時計系は待つうちに
                // 猶予を超えれば Consume へ倒れる。それで正しい。
                if runtime.in_flight.lock().await.contains(&task.to) {
                    continue;
                }

                // 本文の先頭に由来を書く。封筒（【送り手: Concordia】）だけでは
                // モデルが人の発話と区別できない。会話ペインにもそのまま出るので
                // 利用者も定期発火だと分かる。
                let content = format!(
                    "【定期実行: {}】\n{}",
                    task.recurrence.label_ja(),
                    task.message
                );
                // 予定発火はユーザー発話と同格の新しい起点なので hop は 0 —
                // そこから先の転送・委譲に満額の燃料を渡す。
                let message = AgentMessage::new(
                    Endpoint::System,
                    Endpoint::Agent {
                        id: task.to.clone(),
                    },
                    content,
                    0,
                );

                // 配送してから記録する。逆にすると、受信箱が飽和していた場合に
                // 「配られていない発話」が会話ペインへ残る。
                match deliver(shared, &task.to, message.clone()).await {
                    Ok(()) => {
                        shared.record(message).await;
                        runtime.in_flight.lock().await.insert(task.to.clone());
                        consumed.push((task.id.clone(), due_ms));
                    }
                    Err(err) => {
                        // MailboxFull（背圧）: 消化せず次の tick で再試行する。
                        // NotRunning: 上の running 判定との間で停止された競合。
                        // どちらも次の tick が正しく拾い直す。
                        note!(
                            "schedule: {} への配送を見送りました: {err}",
                            task.to
                        );
                    }
                }
            }
        }
    }

    if consumed.is_empty() {
        return;
    }

    // 消化をまとめて書き込む。書き込みロックを持ったまま保存するのは
    // CRUD 側（create/delete/set_enabled）と同じ理由 — 書き手が 2 系統ある。
    let mut schedules = shared.schedules.write().await;
    for (id, due_ms) in consumed {
        if let Some(task) = schedules.iter_mut().find(|task| task.id == id) {
            task.last_consumed_due_ms = Some(due_ms);
        }
    }
    if shared.schedules_blocked.is_none() {
        if let Err(err) = shared.store.save_schedules(&schedules).await {
            // 保存失敗は発火を止める理由にならない。in-memory は既に消化済みで
            // 二重発火は起きず、次の消化で再度保存を試みる。
            note!("schedule: schedules.json の保存に失敗しました: {err}");
        }
    }
}

/// 宛先の受信箱へ届ける。
///
/// `try_send` を使うのは背圧を可視化するため。`send().await` にすると、
/// 詰まった受信箱を待つあいだ送信側のエージェントまで停止して連鎖的に固まる。
async fn deliver(shared: &Shared, to: &AgentId, message: AgentMessage) -> CoreResult<()> {
    deliver_envelope(shared, to, Envelope::plain(message)).await
}

/// 返信路つきの配送。
async fn deliver_envelope(shared: &Shared, to: &AgentId, envelope: Envelope) -> CoreResult<()> {
    let sender = {
        let mailboxes = shared.mailboxes.read().await;
        mailboxes.get(to).cloned()
    };

    let sender = sender.ok_or_else(|| CoreError::NotRunning {
        agent_id: to.to_string(),
    })?;

    sender.try_send(envelope).map_err(|err| match err {
        mpsc::error::TrySendError::Full(_) => CoreError::MailboxFull {
            agent_id: to.to_string(),
            capacity: shared.config.mailbox_capacity,
        },
        mpsc::error::TrySendError::Closed(_) => CoreError::NotRunning {
            agent_id: to.to_string(),
        },
    })
}

/// エージェント 1 体分の実行ループ。
async fn agent_loop(
    agent_id: AgentId,
    mut inbox: mpsc::Receiver<Envelope>,
    mut shutdown: watch::Receiver<bool>,
    shared: Arc<Shared>,
) {
    loop {
        tokio::select! {
            // 停止通知を受信より優先する。停止要求が受信箱の滞留に埋もれないように。
            biased;

            result = shutdown.changed() => {
                // 送信側が落ちた場合も停止扱いにする。
                if result.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            received = inbox.recv() => {
                let Some(envelope) = received else { break };

                // 未着手封筒の畳み（Spec 10 — 出口 2b）。依頼元のターンが既に
                // 切られていれば、**LLM を 1 回も呼ばずに**畳む。会話ログにも
                // 履歴にも積まず、TurnInterrupted も出さない（ターンは中断されて
                // いない。始まらなかっただけ）。依頼主への Reply だけが必須 —
                // 送らずに drop すると親が no_answer に誤分類する（実装バグと定義）。
                // 親が既に interrupted で確定していれば send は失敗するが、それは
                // 正常（race の勝敗は分類を変えない）。
                if envelope.cancel.as_ref().is_some_and(|c| c.is_cancelled()) {
                    note!(
                        "turn folded: agent={agent_id} hop={} from={}",
                        envelope.incoming.hop,
                        match &envelope.incoming.from {
                            Endpoint::User => "user".to_owned(),
                            Endpoint::System => "system".to_owned(),
                            Endpoint::Agent { id } => id.to_string(),
                        },
                    );
                    if let Some(reply_to) = envelope.reply_to {
                        let _ = reply_to.send(Reply {
                            text: "この依頼はユーザーの指示で打ち切られました。\
                                   答えはありません。"
                                .to_owned(),
                            kind: PlanTaskState::Interrupted,
                        });
                    }
                    continue;
                }

                // 失敗しても「何を頼まれたか」だけは履歴へ残せるよう、依頼を控える。
                // handle_message は envelope ごと受け取るので、ここで取っておかないと
                // 失敗時に依頼文へ触れる手段が無くなる。
                let incoming = envelope.incoming.clone();

                // 入力中表示。処理は LLM 呼び出しを含み数十秒かかりうるので、
                // 開始と終了を対で流す。終了は成功・失敗を問わず必ず流す —
                // 片方だけだと「入力中…」が出しっぱなしになる。
                shared.emit(CoreEvent::AgentTyping {
                    agent_id: agent_id.clone(),
                    active: true,
                });

                // ターンの割り込みハンドル（Spec 10）。ターンごとに新しい seq と
                // トークンを発行する — エージェントに紐づくフラグを置くと、
                // ターン A への割り込みが直後のターン B へ漏れる（不変条件 6）。
                //
                // 封筒がキャンセルの手掛かりを持つなら、自ターンのトークンは
                // その**子**として作る（Phase 2）。依頼元の打ち切りも自分への
                // interrupt_turn も同じ 1 本に畳み込まれ、周回境界の検査は
                // 増えない。上の畳み検査との競合も無い — キャンセル済みの親から
                // 作った子はキャンセル済みで生まれる（tokio-util の仕様で確認済み）
                // ので、この隙間で親が切られてもターンは最初の周回境界で止まる。
                let turn = Arc::new(TurnHandle {
                    seq: shared
                        .turn_seq
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                    token: match &envelope.cancel {
                        Some(parent) => parent.child_token(),
                        None => tokio_util::sync::CancellationToken::new(),
                    },
                    requested_at: std::sync::Mutex::new(None),
                });
                shared
                    .turns
                    .lock()
                    .await
                    .insert(agent_id.clone(), Arc::clone(&turn));

                let outcome = handle_message(&shared, &agent_id, envelope, &turn).await;

                // 自分の seq を確かめてから外す。順次処理（不変条件 7）の下では
                // 必ず自分だが、seq を見ない remove は「別のターンのハンドルを
                // 巻き添えで消す」変更に無言で耐えてしまう。
                {
                    let mut turns = shared.turns.lock().await;
                    if turns.get(&agent_id).is_some_and(|h| h.seq == turn.seq) {
                        turns.remove(&agent_id);
                    }
                }

                shared.emit(CoreEvent::AgentTyping {
                    agent_id: agent_id.clone(),
                    active: false,
                });

                if let Err(err) = outcome {
                    // 履歴を先に直す。ここを飛ばすと、依頼が会話ログにしか残らず
                    // どのプロンプト経路にも載らない（失敗のたびに健忘が起きる）。
                    record_failed_turn(&shared, &agent_id, &incoming, &err).await;

                    let payload = ErrorPayload::from(&err);
                    let fatal = err.stops_the_agent();

                    // 失敗したターンもログへ出す。**`turn:` 行は成功経路にしか
                    // 無かった**ため、落ちたターンはログに 1 行も残らず、
                    // 「まだ飛んでいる」と「2 分前に死んだ」が同じ無音に見えた
                    // （2026-07-31、ログを入れた初日に詰まった）。
                    // AgentFailed はトーストへ出るが、トーストは残らない。
                    note!(
                        "turn failed: agent={agent_id} hop={} code={} fatal={fatal}: {}",
                        incoming.hop,
                        payload.code,
                        payload.message,
                    );

                    {
                        let mut world = shared.world.write().await;
                        if let Ok(record) = world.agent_mut(&agent_id) {
                            record.last_error = Some(payload.clone());
                        }
                    }
                    shared.emit(CoreEvent::AgentFailed {
                        agent_id: agent_id.clone(),
                        error: payload,
                    });

                    // 一過性の失敗では止めない。設定不備やスキーマ不整合のように
                    // 再送しても回復しないものだけ、稼働を降ろして原因を目に見せる。
                    if fatal {
                        shared.set_status(&agent_id, AgentStatus::Failed).await;
                        break;
                    }
                }
            }
        }
    }
}

/// 受信した発話へ送り手の封筒を付ける。
///
/// ユーザーの言葉もエージェントからの転送も同じ user ロールで届くため、名前を
/// 書かないと受信側は区別できない — 実際にユーザーの発話を「他のエージェントが
/// 話した言葉」と取り違えた。**プロンプトと履歴の両方へ同じ形で入れる**ので、
/// 組み立てはこの 1 箇所に置く（2 箇所で組むと、片方だけ直したときに
/// 過去のターンだけ出所不明に戻る）。
async fn attribute_sender(shared: &Arc<Shared>, incoming: &AgentMessage) -> String {
    let sender_label = match &incoming.from {
        Endpoint::User => "ユーザー".to_owned(),
        // 表示は UI と同じ「Concordia」。プロンプトと画面で同じ送り手が
        // 違う名前になると、利用者とエージェントの会話が噛み合わない。
        Endpoint::System => "Concordia".to_owned(),
        Endpoint::Agent { id } => {
            let world = shared.world.read().await;
            world
                .agent(id)
                .map(|record| record.spec.name.clone())
                // 送り手が既に削除されていても発話は成立させる。ID で示す。
                .unwrap_or_else(|_| id.to_string())
        }
    };
    format!("【送り手: {sender_label}】\n{}", incoming.content)
}

/// 失敗したターンの**受信側だけ**を履歴へ残す。
///
/// # なぜ要るか（実機で観測、2026-07-31）
///
/// 履歴への書き込みは [`handle_message`] の終盤（統計と同じ節）にあり、
/// 途中の `?` で抜けると**受け取った依頼ごと履歴に残らない**。一方で
/// 広場ログはユーザー発の発話を対象外にし、自分宛も `is_mine` で除外する
/// （それらは履歴にある、という前提で組まれている）。両者の前提が噛み合わず、
/// **失敗したターンの依頼はどのプロンプト経路にも載らない**状態になっていた。
///
/// 実害: 出力上限で 1 ターン落ちた直後、進行役が「直前に何を頼まれたか」を
/// 完全に失い、他のエージェントへ聞いて回った（相手も知らない）。会話ログには
/// 残って画面には見えているので、利用者からは「なぜ忘れたのか」が分からない。
///
/// この repo には既に対になる原則がある — hop 打ち切りの「記録してから打ち切る」と
/// `reset_rule` の「発話は起きた事実でありログに残す」。失敗経路だけが外れていた。
///
/// # 何を積むか
///
/// 受信側は成功時と**同じ封筒**（[`attribute_sender`]）で積む。応答側は実際に
/// 何も言えていないので、失敗した事実を目印として置く — 往復の対を崩すと
/// 役割の交互性が壊れ、プロバイダによっては 400 で拒否される（failures.md #29）。
/// ツール結果は積まない。依頼文さえ残れば「何を頼まれたか」は復元でき、
/// 途中経過まで抱えるのは別の判断（履歴の肥大と引き換えになる）。
async fn record_failed_turn(
    shared: &Arc<Shared>,
    agent_id: &AgentId,
    incoming: &AgentMessage,
    error: &CoreError,
) {
    let attributed = attribute_sender(shared, incoming).await;
    let note = format!(
        "（このターンは失敗し、返答できませんでした: {error}。\
         依頼は未処理のまま残っています）"
    );

    let mut world = shared.world.write().await;
    if let Ok(record) = world.agent_mut(agent_id) {
        record.push_exchange(&attributed, &note, shared.config.history_turns);
    }
}

/// 打ち切られたターンの消費量。[`finish_interrupted`] へまとめて渡す。
///
/// 個別引数にしないのは、u64 が 3 つ並ぶと呼び出し側の取り違えが
/// コンパイルを通ってしまうため。
struct TurnSpend {
    /// 累計トークン（入力 + 出力）。
    tokens: u64,
    /// キャッシュから読んだ入力トークン。
    cached: u64,
    /// 入力トークン。
    prompt: u64,
    /// 打ち切りまでに完走した LLM 呼び出しの周回数。
    rounds: u32,
    /// 受信した発話の hop。
    hop: u8,
}

/// 割り込みで打ち切られたターンの出口（Spec 10 — 契約の出口 2a）。
///
/// 3 点セット: (a) 会話ログへ System の 1 行（要求から検知までの elapsed を
/// 含む — LLM 呼び出し中の切断を別 Spec で入れるかの判断材料）
/// (b) 履歴へ [`record_interrupted_turn`] の注記 (c) 依頼主が居れば
/// `Reply { kind: Interrupted }`。まとめの LLM 呼び出しは**しない**
/// （打ち切りの直後にもう 1 回課金しない — RepeatGuard の打ち切りと同じ判断）。
///
/// 打ち切りは失敗ではない（不変条件 4） — `AgentFailed` を出さず、
/// `last_error` にも書かず、ステータスは Running のまま。だからこの関数は
/// `Ok(())` を返す。ここまでに使ったトークンは実際に消費したので統計へ積む。
async fn finish_interrupted(
    shared: &Arc<Shared>,
    agent_id: &AgentId,
    reply_to: Option<tokio::sync::oneshot::Sender<Reply>>,
    turn: &TurnHandle,
    sent_user_turn: &str,
    spend: TurnSpend,
) -> CoreResult<()> {
    // None = 自分への interrupt_turn ではなく、依頼元の打ち切りが子トークン
    // 経由で連鎖した（Phase 2）。そのとき「要求から 0.0 秒」と書くと、
    // 検知が一瞬だったという嘘の計測値になる — 計測が無いことを言葉で言う。
    let elapsed = turn
        .requested_at
        .lock()
        .expect("await を跨がない")
        .map(|at| at.elapsed());
    let cause = match elapsed {
        Some(elapsed) => format!("要求から {:.1} 秒", elapsed.as_secs_f64()),
        None => "依頼元の打ち切りに連鎖".to_owned(),
    };

    // (b) 履歴 + 統計。1 回の world ロックで済ませる。
    record_interrupted_turn(shared, agent_id, sent_user_turn, &spend).await;

    // (a) 会話ログへ System の 1 行。表示名は「切られた本人」— System 行は
    // 全員の会話ペインに出るので、誰のターンかを名指ししないと読めない。
    let display = {
        let world = shared.world.read().await;
        world
            .agent(agent_id)
            .map(|record| record.spec.name.clone())
            .unwrap_or_else(|_| agent_id.to_string())
    };
    shared
        .record(AgentMessage::new(
            Endpoint::System,
            Endpoint::User,
            format!("{agent_id}（{display}）のターンをユーザーの指示で打ち切りました（{cause}）"),
            0,
        ))
        .await;

    // 出口の行はここ（切られたターン自身）だけが書く。割り込んだ側は書かない —
    // 二重割り込み・interrupt_all・親トークン経由が重なっても 1 本になる。
    shared.emit(CoreEvent::TurnInterrupted {
        agent_id: agent_id.clone(),
        turn_seq: turn.seq,
    });

    // (c) 依頼主への返信。文言は契約（P3）の固定文。受け取り手が既に
    // 諦めている（タイムアウト・親も打ち切り済み）ことはあるので送信の失敗は
    // 無視する — 「drop は実装バグ」の射程はワーカーが送らないことであって、
    // 確定済みの親が受け取らないことではない。
    if let Some(reply_to) = reply_to {
        let _ = reply_to.send(Reply {
            text: "この依頼はユーザーの指示で打ち切られました。答えはありません。".to_owned(),
            kind: PlanTaskState::Interrupted,
        });
    }

    note!(
        "turn interrupted: agent={agent_id} seq={} hop={} rounds={} elapsed_ms={} \
         prompt={} cached={} total={}",
        turn.seq,
        spend.hop,
        spend.rounds,
        // 連鎖（None）は -1。0 と区別する — 0 は「即検知」という実測値。
        elapsed.map_or(-1, |e| i128::try_from(e.as_millis()).unwrap_or(i128::MAX)),
        spend.prompt,
        spend.cached,
        spend.tokens,
    );
    Ok(())
}

/// 打ち切られたターンの受信側を履歴へ残す（Spec 10 — 出口 2a の (b)）。
///
/// [`record_failed_turn`] と**文言を共有しない** — 失敗の文言を使い回すと、
/// 次のターンの自分が「エラーが起きた」と誤読する。起きたのは指示による
/// 打ち切りで、依頼そのものは健在。
///
/// 受信側は `sent_user_turn`（実際に送った形）をそのまま積む。`attributed`
/// だけに縮めると送信と保存が食い違い、その位置で前方一致が切れる
/// （failures.md #45 — 打ち切りの検知点では組み立てが済んでいるので、
/// 失敗経路と違って送った形が手元にある。縮める理由が無い）。
async fn record_interrupted_turn(
    shared: &Arc<Shared>,
    agent_id: &AgentId,
    sent_user_turn: &str,
    spend: &TurnSpend,
) {
    let note = "（このターンはユーザーの指示で打ち切られました。\
                依頼は未処理のまま残っています）";

    let mut world = shared.world.write().await;
    if let Ok(record) = world.agent_mut(agent_id) {
        record.total_tokens += spend.tokens;
        record.cached_tokens += spend.cached;
        record.prompt_tokens += spend.prompt;
        record.push_exchange(sent_user_turn, note, shared.config.history_turns);
    }
}

/// プロンプトキャッシュの診断行を 1 周ごとに残す。
///
/// # なぜ率ではなく生の数字が要るか
///
/// カードの「入力の N% をキャッシュ」だけでは、**0% の理由が三つ巴**になる —
/// (a) プロバイダの最小長を下回った (b) 前方一致が壊れた (c) プロバイダが
/// 値を返していない。この 3 つは処方が全部違うのに、画面上は同じ 0% に見える。
///
/// # 累積では判別できない
///
/// カードが持つ `promptTokens` は**ターンをまたいだ累積**なので、閾値との
/// 比較に使えない — 1 周 1,000 トークンのエージェントでも 5 周喋れば 5,000 に
/// なり、「閾値を超えているのに 0%」と誤読される。判定には**その周の値**が要る。
///
/// # ハッシュは system プロンプト全文に掛ける
///
/// 安定部分（`stable_len` まで）だけに掛けると、顔ぶれや Memory が変わって
/// 前方一致が切れた場合 (b) を「変わっていない」と表示してしまう。会話が
/// キャッシュに載るには **systemInstruction 全体**がバイト一致している必要がある。
///
/// ハッシュ値はプロセスをまたいで比較しない（`DefaultHasher` の値は Rust の
/// 版に依存する）。見るのは**同じセッション内で周ごとに変わったかどうか**だけ。
fn note_cache_diag(
    agent_id: &AgentId,
    model: &str,
    round: u32,
    usage: &crate::llm::Usage,
    system: SystemDigest,
    history: HistoryDepth,
) {
    note!(
        "cache: agent={agent_id} model={model} round={round} \
         prompt={} cached={} system_chars={} stable_chars={} system_blocks={} \
         history_msgs={}/{} system_digest={:016x}",
        usage.prompt,
        usage.cache_read,
        system.chars,
        system.stable_chars,
        system.blocks,
        history.msgs,
        history.limit,
        system.digest,
    );
}

/// 履歴の通数と上限。**`history_msgs` が `limit` に張り付いていたら窓が滑っている**
/// = 毎ターン先頭の 1 往復が落ち、前方一致は system の直後で切れる。
#[derive(Debug, Clone, Copy)]
struct HistoryDepth {
    msgs: usize,
    limit: usize,
}

/// プロバイダへ実際に渡る system 面の指紋。
///
/// # 数えるのは「連結後」でなければならない
///
/// adapter は `Role::System` のメッセージを**配列のどこにあっても全部引き抜いて**
/// 1 つの `system` / `systemInstruction` へ連結する（`gemini.rs` / `anthropic.rs` の
/// `encode`）。したがって「system プロンプト 1 本」だけを数えても、実際に前方一致の
/// 先頭を占める文字列とは別物になる。
///
/// 初版はまさにそれを数えており、可変ブロック（参照資料・広場ログ・入退室）が
/// 毎ターン変わっていても digest は動かなかった。**その計装で「前方一致は壊れて
/// いない」と読んだのは誤診**で、検出不能だっただけ（failures.md #45）。
#[derive(Debug, Clone, Copy)]
struct SystemDigest {
    /// 連結後の文字数。
    chars: usize,
    /// 安定部分の文字数（`cacheable_prefix_len` と同じ値）。
    stable_chars: usize,
    /// system ブロックの本数。2 本を超えていたら可変ブロックが混ざっている。
    blocks: usize,
    /// 連結後の全文のハッシュ。
    digest: u64,
}

impl SystemDigest {
    /// adapter と**同じ畳み方**で数える。ここがズレると指紋の意味が消える。
    fn of(messages: &[ChatMessage], stable_len: usize) -> Self {
        use std::hash::{Hash, Hasher};

        let blocks: Vec<&str> = messages
            .iter()
            .filter(|m| m.role == Role::System)
            .map(|m| m.content.as_str())
            .collect();
        let joined = blocks.join("\n\n");

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        joined.hash(&mut hasher);
        Self {
            chars: joined.chars().count(),
            stable_chars: stable_len,
            blocks: blocks.len(),
            digest: hasher.finish(),
        }
    }
}

/// 同じ結果が何回返ったら次を実行しないか（failures.md #41 の処方 1）。
///
/// 2 = 「同じ呼び出しに同じ結果が 2 回返ったら、3 回目は実行しない」。
const REPEAT_BLOCK_AFTER: u32 = 2;

/// 1 つの呼び出し（ツール名 + 引数）について、ターン内で最後に見た結果。
#[derive(Debug)]
struct SeenCall {
    /// ツール名。
    name: String,
    /// 引数。等価判定は `serde_json::Value` の中身で行う（キーの並びに依存しない）。
    args: serde_json::Value,
    /// 直近にこの呼び出しが返した本文。
    body: String,
    /// `body` が**変わらないまま**返ってきた回数。
    count: u32,
}

/// 同一のツール呼び出しの繰り返しを検出する（failures.md #41 の処方 1）。
///
/// # 判定材料に結果**本文**を使う理由
///
/// 台帳の処方は「ツール名 + 引数 + エラー文言」だが、同梱ツールは失敗を
/// `Err` ではなく **`Ok(<エラー文の本文>)`** で返す（「ツールの失敗は会話を
/// 止めない」という既存規律の帰結。`file` / `fd` / `grep` / `sd` すべてこの形）。
/// したがって `Result::is_err` で失敗を数えると、実機で燃えた経路
/// （`sd` の失敗を 12 周繰り返した failures.md #39）は 1 件も検出できない。
/// 失敗が型に載っていない以上、**モデルへ返る本文の完全一致**が失敗の
/// 一致を表す唯一の実体になる。文言を parse して失敗かどうかを推定するのは
/// やらない（Spec 08 で「分類は文言 parse でなく型で運ぶ」と決めた側の話）。
///
/// 成功の繰り返しも同じ扱いで止まる。同じ入力に同じ出力が返っている以上、
/// 3 回目に新しい情報は無い。
///
/// # 数えるのは「呼び出しごと」であって「隣接」ではない
///
/// 当初は直前の 1 件とだけ比べていた（隣接する 2 回で判定）。**実機では 1 件も
/// 発火しなかった** — モデルは 1 周に 2〜3 本を並列で呼ぶので、同じ読み直しは
/// 周をまたいで現れ、間に別の呼び出しが挟まって数えが切れる。実測（2026-07-31）:
///
/// ```text
/// round 24 file(A) → 12054 字   round 2 file(B) → 12045 字
/// round 25 file(A) → 12054 字   round 3 file(B) → 12045 字 + file(C) + file(D)
/// round 26 grep    → 別物       round 5 file(B) → 12045 字
/// round 28 file(A) → 12054 字（3 回目。隣接判定では素通し）
/// ```
///
/// そこで **(ツール名 + 引数) ごとに独立して数える**。一致の条件は
/// 完全一致のままで、「隣り合っているか」の要求だけを外した。
/// 同じ呼び出しが**違う結果**を返したら、そこで数え直す（追記が進む・待っていた
/// 状態が変わる、のように同じ操作が実を結んでいる場合は繰り返しではない）。
///
/// # 止めるのはループではなく、その 1 本
///
/// 3 回目を実行しないだけで、ターンのツールループは続ける。並列の 1 本が
/// 重複しただけで、進行中の作業まで殺さない。**その周のツールが全部
/// ブロックされたとき**（= 新しいことを何もしていない周）だけ打ち切る。
#[derive(Debug, Default)]
struct RepeatGuard {
    /// ターン内で見た呼び出し。(name, args) につき 1 件。
    ///
    /// `Vec` なのは `serde_json::Value` が `Hash` を実装しないため。
    /// 1 ターンの相異なる呼び出しは高々数十件で、線形走査で足りる。
    seen: Vec<SeenCall>,
}

impl RepeatGuard {
    /// この呼び出しを実行せずに止めるか。**実行の前**に引く。
    ///
    /// 結果はまだ無いので、ここで見られるのはツール名と引数だけ。
    /// 「同じ引数で同じ結果が [`REPEAT_BLOCK_AFTER`] 回返った呼び出しが、
    /// また同じ引数で来た」ときに真を返す。
    fn blocks(&self, name: &str, args: &serde_json::Value) -> bool {
        self.repeats(name, args) >= REPEAT_BLOCK_AFTER
    }

    /// この呼び出しに同じ結果が返った回数。打ち切りの通知に載せる。
    fn repeats(&self, name: &str, args: &serde_json::Value) -> u32 {
        self.find(name, args).map_or(0, |seen| seen.count)
    }

    /// 実行した 1 件を記録する。**実行の後**に引く。
    fn observe(&mut self, name: &str, args: &serde_json::Value, body: &str) {
        match self.position(name, args) {
            Some(index) => {
                let seen = &mut self.seen[index];
                if seen.body == body {
                    seen.count += 1;
                } else {
                    // 結果が変わった = この呼び出しは行き詰まっていない。数え直す。
                    seen.body = body.to_owned();
                    seen.count = 1;
                }
            }
            None => self.seen.push(SeenCall {
                name: name.to_owned(),
                args: args.clone(),
                body: body.to_owned(),
                count: 1,
            }),
        }
    }

    fn find(&self, name: &str, args: &serde_json::Value) -> Option<&SeenCall> {
        self.seen
            .iter()
            .find(|seen| seen.name == name && &seen.args == args)
    }

    fn position(&self, name: &str, args: &serde_json::Value) -> Option<usize> {
        self.seen
            .iter()
            .position(|seen| seen.name == name && &seen.args == args)
    }
}

/// 受信した発話を 1 件処理する。
///
/// 手順: プロンプト組み立て → RAG 付与 → LLM 呼び出し → 統計更新 → 記録 → 転送。
///
/// **途中で失敗すると履歴は書かれない。** 呼び出し側（[`agent_loop`]）が
/// [`record_failed_turn`] で受信側だけを残す責任を持つ。
async fn handle_message(
    shared: &Arc<Shared>,
    agent_id: &AgentId,
    envelope: Envelope,
    turn: &TurnHandle,
) -> CoreResult<()> {
    let Envelope {
        incoming,
        reply_to,
        // 自ターンのトークンは agent_loop が子として導出済み（`turn.token`）。
        // ここで別々に見ると 2 本の検査になる — 1 本に畳むのが Phase 2 の核。
        cancel: _,
    } = envelope;
    // ターンの開始を残す。**無音の起点が分からないと、飛行中と落ちた後を
    // 区別できない** — `tool:` 行はツールを呼んだ周にしか出ないので、
    // LLM の応答を待っている間はログが止まって見える（2026-07-31 に実際に
    // 詰まった。ツール 4 周目のあと 2 分無音で、生死が判定できなかった）。
    note!(
        "turn start: agent={agent_id} hop={} from={} chars={}",
        incoming.hop,
        match &incoming.from {
            Endpoint::User => "user".to_owned(),
            Endpoint::Agent { id } => id.to_string(),
            Endpoint::System => "system".to_owned(),
        },
        incoming.content.chars().count(),
    );
    // 1. 定義とテンプレートを取り出す。ロックはここで手放し、LLM 呼び出しは持たずに行う。
    let (spec, template) = {
        let world = shared.world.read().await;
        let record = world.agent(agent_id)?;
        let template = world.template(&record.spec.model_template_id)?.clone();
        (record.spec.clone(), template)
    };

    // 2. システムプロンプトを組む。安定部分の長さも同時に得る（キャッシュ境界）。
    //    接地の有無はテンプレート由来（エージェント個別の設定ではない）。
    //    フラグではなく grounding_active() を見る — 互換経路のまま真になっている
    //    設定（world.json の直接編集で作れる）に「検索できます」と教えないため。
    //
    //    顔ぶれ（Spec 06 P1.5）はここで組む。順序はツール提示順（=
    //    connected_agents の保存順）と同一 — 顔ぶれだけ別の整列規則を持つと、
    //    同じ相手の並びが transfer_to_* と食い違い、モデルに二重管理を強いる。
    //    形式は agent_id（表示名）: 状態。id はモデルの宛先語彙（ツール名）で、
    //    無いと表示名 → id の対応をツール説明から二段引きすることになる。
    let roster: Option<String> = {
        let world = shared.world.read().await;
        let entries: Vec<String> = spec
            .connected_agents
            .iter()
            .map(|id| {
                world
                    .agent(id)
                    .map(|record| {
                        format!("{id}（{}）: {}", record.spec.name, record.status.label())
                    })
                    // 接続先が消えていても行は成立させる（ID と不明で示す）。
                    .unwrap_or_else(|_| format!("{id}: 不明"))
            })
            .collect();
        (!entries.is_empty()).then(|| entries.join(" / "))
    };
    let (system_prompt, stable_len) = shared
        .store
        .compose_system_prompt(&spec, template.grounding_active(), roster.as_deref())
        .await?;

    // 3. 転送先ごとのツールを組む。
    //    OpenAI Agents SDK は handoff を「宛先 1 つにつきツール 1 本」で表現し、
    //    `transfer_to_<agent>` という名前を使う。単一ツール + 宛先パラメータより、
    //    名前で選ばせるほうがモデルの学習分布に近い。
    //    宛先は ID だけでなく**表示名**も添えて渡す。会話は表示名で流れるので、
    //    名前と ID を結ぶ情報がプロンプトに無いと、モデルは誰に渡すか推測になる。
    let targets: Vec<(AgentId, String)> = {
        let world = shared.world.read().await;
        spec.connected_agents
            .iter()
            .map(|id| {
                let display = world
                    .agent(id)
                    .map(|record| record.spec.name.clone())
                    // 接続先が消えていても転送経路自体は壊さない。ID で示す。
                    .unwrap_or_else(|_| id.to_string());
                (id.clone(), display)
            })
            .collect()
    };
    let handoffs = HandoffTools::build(&targets);
    let use_handoff_tools = template.use_tools && !handoffs.is_empty();

    // 4. プロンプトを組む。順序は system → 手順 → 履歴 → 可変の文脈 + 今回の受信。
    //
    // **`Role::System` は「安定なもの」専用の枠として扱う。** adapter は
    // Role::System のメッセージを配列のどこにあっても全部引き抜いて 1 つの
    // system / systemInstruction へ連結するので（gemini.rs / anthropic.rs の
    // encode）、可変なものを System で積むと**配列上の位置に関係なく前方一致の
    // 先頭へ戻る**。位置を変えるだけでは直らない（failures.md #45）。
    let mut messages = vec![ChatMessage::system(system_prompt)];
    if !handoffs.is_empty() {
        messages.push(ChatMessage::system(handoffs.protocol_note(use_handoff_tools)));
    }

    // 履歴。これが無いと毎回コールドスタートになり、同じ入力に同じ出力を返し続ける。
    //
    // 通数を控える理由は診断のため。`history_turns` は**滑る窓**で（world.rs の
    // push_exchange が先頭から drain する）、埋まると毎ターン先頭の 1 往復が落ちる。
    // 前方一致はそこで切れるので、窓が埋まった瞬間からキャッシュは system 止まりに
    // なる。窓が上限に張り付いているかは通数を見ないと分からない。
    let history_msgs = {
        let world = shared.world.read().await;
        match world.agent(agent_id) {
            Ok(record) => {
                messages.extend(record.history.iter().cloned());
                record.history.len()
            }
            Err(_) => 0,
        }
    };

    // ここから下は**毎ターン変わる文脈**。System では積まず、`context` へ溜めて
    // 最後に今回の受信と一緒に 1 本の user 発話として送る。
    //
    // こうする理由は 2 つ。(1) System で積むと adapter が先頭へ畳むので
    // 前方一致がそこで切れる。(2) user ロールで別々に積むと user が連続し、
    // ロールの交互を要求するプロバイダで壊れる。**1 本に畳めば両方避けられる。**
    //
    // 履歴には入れない（`attributed` だけを積む）— 今回だけの文脈を履歴へ
    // 焼き付けると、以後の全ターンのプレフィックスに残り続ける。
    let mut context: Vec<String> = Vec::new();

    // RAG。Rayon 側で検索するので、この待ち時間に他エージェントも進む。
    // **毎ターン必ず変わる**（今回の発話で検索するため）。意味の上でも、
    // 参照資料は「今回の問いに答えるための材料」なので問いの近くが正しい。
    if !spec.rag_sources.is_empty() {
        let hits = shared
            .rag
            .read()
            .await
            .search(&spec.rag_sources, &incoming.content, shared.config.rag_top_k)
            .await?;
        if !hits.is_empty() {
            let refs = hits
                .iter()
                .map(|h| format!("- [{}] {}", h.item.source, h.item.text))
                .collect::<Vec<_>>()
                .join("\n");
            context.push(format!("## 参照資料\n{refs}"));
        }
    }

    // 居合わせた会話（広場ログ）。受信側でオプトアウトできる（Spec 03）:
    // 毎ターン最大 12 件 × 200 字の固定費であり、場の共有が要らない役には
    // 価値が無い。false でも自分の発話は他者の広場ログに載る（受信側だけの設定）。
    //
    // 元は「場の背景であって自分とのやり取りではない」から System の枠で履歴の
    // **前**に置いていた。その読みは筋が通っていたが、**他人が喋るたびに前方一致が
    // 切れる**という代償が見えていなかった — 村として使っているときにこそ
    // キャッシュが効かなくなる。
    if spec.hears_room_log
        && let Some(room) = compose_room_log(shared, agent_id, &shared.config).await
    {
        context.push(room);
    }

    // 入退室の通知（Spec 06 P1）。**広場ログの gate の外**に置く —
    // 広場ログのオプトアウトは「場の共有が要らない役から固定費を外す」機能で、
    // 入退室は場の雑談ではなく配送先の正しさに関わる情報。コストの設定が
    // 経路の正しさを黙って壊す形にしない。
    if let Some(notices) =
        compose_presence_notices(shared, &shared.config, roster.is_some()).await
    {
        context.push(notices);
    }

    // 送り手の封筒。ユーザーの言葉もエージェントからの転送も同じ user ロールで
    // 届くため、名前を書かないと受信側は区別できない — 実際にユーザーの発話を
    // 「他のエージェントが話した言葉」と取り違えた。プロンプトと履歴の両方へ
    // 同じ形で入れる。履歴に入れないと、次のターンで再び出所不明になる。
    let attributed = attribute_sender(shared, &incoming).await;

    // 同報の注記。「みんなへ」と呼びかけられたのに自分しか受け取っていないように
    // 見えると、各エージェントは律儀に接続先へ転送して反響が起きる（実機で観測）。
    // 転送を禁止するのではなく、「全員が既に受け取っている」という事実を与えて
    // 転送する理由そのものを消す。
    //
    // これも System では積まない。同報かどうかは発話ごとに変わるので、System へ
    // 入れると adapter が先頭へ畳んで前方一致を切る（failures.md #45）。
    if incoming.co_recipients.len() >= 2 {
        let world = shared.world.read().await;
        let names: Vec<String> = incoming
            .co_recipients
            .iter()
            .map(|id| {
                world
                    .agent(id)
                    .map(|record| record.spec.name.clone())
                    // 宛先が既に削除されていても注記自体は成立させる。ID で示す。
                    .unwrap_or_else(|_| id.to_string())
            })
            .collect();
        // 「転送するな」だけでは足りない。実機では、転送の代わりに
        // 「ユーザーから依頼です、自己紹介お願いします」という**新しい発話**を
        // 全員へ配って回り、同じ混乱が起きた（促しは転送ではないので注記の射程外だった）。
        // 塞ぐべきは経路ではなく、**他人の分まで面倒を見ようとする動機**のほう。
        context.push(format!(
            "【同報】この発話はあなたを含む {} 体（{}）へ同時に届いています。\
             全員が同じ内容を既に受け取っており、**それぞれが自分で答えます**。\
             したがって、この内容を他のエージェントへ転送する必要はありませんし、\
             他の参加者に発言を促す必要もありません。\
             あなたは**あなた自身の分だけ**答えてください。",
            names.len(),
            names.join("、")
        ));
    }

    // 可変の文脈と今回の受信を **1 本の user 発話**に畳んで送る。
    //
    // **送った文字列をそのまま履歴へ積む**（下の push_exchange へ渡す）。
    // 当初は `attributed` だけを積んで「今回だけの文脈を履歴へ焼き付けない」
    // ようにしたが、それは**送信と保存の食い違い**を作る。次のターンでは履歴側の
    // 短い文字列がその位置に来るので、**前方一致がそこで切れる** — 以後どれだけ
    // 会話が伸びてもキャッシュは system + tools で頭打ちになる（failures.md #45）。
    //
    // 揃えるほうが記録としても正しい。エージェントは実際にその文脈込みで受け取って
    // おり、`attributed` だけを積むのは受け取った内容についての嘘になる。
    context.push(attributed.clone());
    let sent_user_turn = context.join("\n\n");
    messages.push(ChatMessage::user(&sent_user_turn));

    // 指紋は**組み終わってから**取る。adapter と同じ畳み方で数えないと、
    // 実際に前方一致の先頭を占める文字列とは別物を測ることになる。
    let system_digest = SystemDigest::of(&messages, stable_len);
    let history_depth = HistoryDepth {
        msgs: history_msgs,
        limit: shared.config.history_turns.saturating_mul(2),
    };

    // 5. ツールを提示する。転送用と実行用を 1 つの集合としてモデルへ渡す。
    //    モデルから見れば「次に何をするか」の選択肢はどちらも同じ粒度で、
    //    転送だけ別扱いにする理由が無い。区別するのはこちら側の役目。
    //    同梱ツールはエージェント個別の提示制御（enabled_tools + 作業フォルダ
    //    連動の自動除外）を通す — 使わないツールのスキーマは毎ターンの
    //    固定費になる（トークン節約は最重要課題）。
    let mut specs = if use_handoff_tools {
        let mut both = handoffs.specs();
        both.extend(handoffs.ask_specs());
        // 並列委譲は接続先 2 体以上のときだけ載る（Spec 04）。
        // 1 体しか繋がっていないエージェントには使えない選択肢なので、
        // そのスキーマを毎ターンの固定費として払わせない。
        both.extend(handoffs.plan_specs());
        both
    } else {
        Vec::new()
    };
    let shared_specs: Vec<ToolSpec> = shared
        .tools
        .read()
        .await
        .specs()
        .into_iter()
        .filter(|tool| is_bundled_tool_presented(&tool.name, &spec))
        .collect();
    // エージェント別 MCP のツールを重ねる（ツール収集の最終形）。
    // 同名は個別が勝つ — 共通と同じサーバーを自分専用の接続先で
    // 置き換える正当な手段（上書き可能な加算）。
    let personal_specs: Vec<ToolSpec> = {
        let map = shared.agent_mcp.read().await;
        map.get(agent_id)
            .map(|state| state.manager.tools().iter().map(|tool| tool.spec()).collect())
            .unwrap_or_default()
    };
    let executable = merge_tool_specs(shared_specs, personal_specs);
    specs.extend(executable.iter().cloned());
    let use_tools = !specs.is_empty() && template.use_tools;

    // 6. 実行ループ。
    //    規則は OpenAI Agents SDK と同じ:
    //    ツールを呼んだら実行して結果を積み、もう一度呼ぶ。
    //    ツールを呼ばないテキスト出力が出たら、それが最終出力。
    let backend = shared.backend_for(&template).await?;
    // キャッシュ読み取り分は別に数える。合計だけ見ていると、キャッシュが
    // 一度も効いていない状態と完全に効いている状態が同じ数字に見える
    // (実際、実機で 5 体全員が無キャッシュのまま数日走っていた。failures.md #33)。
    let mut cached = 0u64;
    // 入力ぶんも別に数える。キャッシュ率の分母は合計ではなく入力。
    let mut prompt = 0u64;
    let mut tokens = 0u64;
    // 接地の来歴は 1 周ぶんではなく**ターンぶん**で持つ。検索した周と
    // 関数を呼んだ周は別なので、周ごとに上書きすると先に起きた接地が消える。
    let mut grounding = crate::llm::Grounding::default();
    let mut outcome = Outcome::Finish {
        content: String::new(),
    };

    // ツール実行の上限。エージェント個別の指定があれば優先する
    // （コーディング用エージェントは調査のツール往復が多く、既定では足りない）。
    let max_tool_iterations = spec
        .max_tool_iterations
        .unwrap_or(shared.config.max_tool_iterations)
        .max(1);
    let mut tool_limit_hit = false;
    // 同一失敗の検出（failures.md #41 の処方 1）。ターン内でだけ数える —
    // ターンを跨ぐ繰り返しは別の問題（依頼が同じなら同じ失敗をもう一度たどるのは
    // 正しい）で、ここで縛るとやり直しの依頼まで殺す。
    let mut repeat_guard = RepeatGuard::default();
    // 繰り返しで打ち切ったツール名。まとめ呼び出しと最終文言の分岐に使う。
    let mut repeat_stop: Option<String> = None;
    // 観測用（Spec 04 Notes 2 のトリガー判定の実測材料）。
    // llm_rounds は上限の較正（12 で足りているか）、plan_wave は波の因果の追跡。
    let mut llm_rounds: u32 = 0;
    let mut plan_wave: u32 = 0;

    for iteration in 0..max_tool_iterations {
        // 割り込みの検査点（Spec 10 — 契約の不変条件 1）。周回境界 =
        // 次の LLM 呼び出しを組み立てる前。ここなら呼び出しと結果の対（#29）が
        // 必ず揃っており、送信と保存の一致（#45）も壊れない。飛行中の
        // LLM 呼び出し・実行中のツールは完走させる（rev1 の判断 — 検知の
        // 遅さは System 行の elapsed で測り、Notes 2 の判断材料にする）。
        if turn.token.is_cancelled() {
            return finish_interrupted(
                shared,
                agent_id,
                reply_to,
                turn,
                &sent_user_turn,
                TurnSpend {
                    tokens,
                    cached,
                    prompt,
                    rounds: llm_rounds,
                    hop: incoming.hop,
                },
            )
            .await;
        }

        let request = ChatRequest {
            model: template.model.clone(),
            messages: messages.clone(),
            tools: if use_tools { specs.clone() } else { Vec::new() },
            tool_choice: if use_tools {
                crate::llm::ToolChoice::Auto
            } else {
                crate::llm::ToolChoice::None
            },
            temperature: template.temperature,
            max_tokens: template.max_output_tokens,
            effort: template.effort,
            cacheable_prefix_len: stable_len,
        };

        let mut response = backend.chat(request).await?;
        llm_rounds += 1;
        note_cache_diag(
            agent_id,
            &template.model,
            llm_rounds,
            &response.usage,
            system_digest,
            history_depth,
        );
        tokens += response.usage.total();
        cached += response.usage.cache_read;
        prompt += response.usage.prompt;
        // 転送で抜ける周の接地も拾う。break の後ろに置くと、検索してから
        // 転送したターンの来歴が丸ごと落ちる。
        grounding.absorb(std::mem::take(&mut response.grounding));

        // 転送の要求は「会話を渡す」ことなので、ここでループを抜ける。
        // 結果が返ってくる種類の操作ではない。
        outcome = handoffs.decide(&response, use_handoff_tools);
        if matches!(outcome, Outcome::Handoff { .. }) {
            break;
        }

        // 実行対象のツール呼び出しを拾う。転送用の名前はここには来ない
        // （上で Handoff として抜けている）。委譲（`ask_*`）は**結果が返る**ので、
        // 転送ではなくこちら側 — 実行ツールと同じ扱いでループを回す。
        let calls: Vec<_> = response
            .tool_calls
            .iter()
            .filter(|call| {
                executable.iter().any(|spec| spec.name == call.name)
                    || (use_handoff_tools && handoffs.resolve_ask(&call.name).is_some())
                    // plan は executable にも resolve_ask にも該当しない。
                    // ここへ足し忘れると `calls` が空 = 最終出力と読まれ、
                    // モデルが呼んだのに**何も起きず本文だけ返る**
                    // （エラーにならないので気づけない）。
                    || (use_handoff_tools
                        && handoffs.offers_plan()
                        && call.name == HandoffTools::PLAN)
            })
            .cloned()
            .collect();

        if calls.is_empty() {
            // ツールを呼ばなかった = 最終出力。
            break;
        }

        // 呼び出しと結果は**対で**積む。呼び出しを残さずに結果だけ積むと、
        // プロバイダが「対応する呼び出しが無い結果」として拒否する。
        messages.push(ChatMessage::assistant_tool_calls(
            response.text.clone().unwrap_or_default(),
            calls.clone(),
        ));

        // この周で実際に走った本数と、繰り返しで止めた本数。
        // **全部止めた周だけがループの打ち切り条件**（新しいことを何もしていない周）。
        let mut executed_in_round = 0usize;
        let mut blocked_in_round: Option<String> = None;

        for call in &calls {
            // 同じ呼び出しに同じ結果が返り続けているなら、この 1 本は実行しない
            // （failures.md #41 の処方 1）。**結果は必ず積む** — 呼び出しだけ
            // 残して結果を落とすと、次のリクエストが「対応する結果が無い
            // 呼び出し」として 400 で拒否される（#29）。
            // 返す本文は短くする。**ここが効きの本体** — 同じ 12,000 字を
            // もう一度積むと、以後の全周回でそれが再送される。
            if repeat_guard.blocks(&call.name, &call.args) {
                let repeats = repeat_guard.repeats(&call.name, &call.args);
                shared.emit(CoreEvent::ToolRepeatBlocked {
                    agent_id: agent_id.clone(),
                    tool: call.name.clone(),
                    repeats,
                });
                messages.push(ChatMessage::tool_result(
                    &call.id,
                    &call.name,
                    format!(
                        "`{}` は同じ引数で既に {repeats} 回、同じ結果を返しています。\
                         もう一度呼んでも同じなので実行しませんでした。\
                         引数か手順を変えるか、**できなかったこと自体を答えとして**\
                         報告してください。",
                        call.name
                    ),
                ));
                note!(
                    "tool blocked: agent={agent_id} round={} name={} repeats={repeats}",
                    iteration + 1,
                    call.name,
                );
                blocked_in_round.get_or_insert_with(|| call.name.clone());
                continue;
            }

            // 並列委譲は 1 回の呼び出しで N 体ぶんの仕事をする。ツール実行の
            // 上限（`max_tool_iterations`）の消費も 1 回で済む。
            let result = if use_handoff_tools
                && handoffs.offers_plan()
                && call.name == HandoffTools::PLAN
            {
                plan_wave += 1;
                Ok(run_plan(
                    shared,
                    agent_id,
                    &handoffs,
                    call,
                    incoming.hop,
                    plan_wave,
                    &turn.token,
                )
                .await)
            } else {
                match handoffs.resolve_ask(&call.name) {
                    Some(target) if use_handoff_tools => {
                        ask_agent(shared, agent_id, target, call, incoming.hop, &turn.token).await
                    }
                    _ => execute_tool(shared, agent_id, call).await,
                }
            };
            shared.emit(CoreEvent::ToolInvoked {
                agent_id: agent_id.clone(),
                tool: call.name.clone(),
                ok: result.is_ok(),
            });
            let ok = result.is_ok();
            let body = match result {
                Ok(text) => text,
                // 失敗しても会話を止めない。モデルが読んで次を決める。
                Err(err) => format!("ツールの実行に失敗しました: {err}"),
            };
            // ツール 1 本ごとの実測。**`body_chars` がこの行の主目的** — ツール結果は
            // 履歴に積まれて以後の全周回で再送されるので、1 本の大きさが
            // そのターンの入力トークンに周回数ぶん掛かって効く。ターン行の
            // `rounds` と `prompt` だけでは「何がプロンプトを太らせたか」が
            // 追えなかった（2026-07-31 の 730,406 トークンの診断で不足した欄）。
            // `ok` は `Err` だったかどうかで、同梱ツールは失敗も `Ok` の本文で
            // 返すため `ok=true` のまま失敗していることがある（CoreEvent::ToolInvoked
            // と同じ意味。判定材料にするなら本文の側を見る）。
            note!(
                "tool: agent={agent_id} round={} name={} ok={ok} args_chars={} body_chars={}",
                iteration + 1,
                call.name,
                call.args.to_string().chars().count(),
                body.chars().count(),
            );
            // 数えるのは**モデルへ返した本文**。同梱ツールの失敗は `Err` ではなく
            // この本文に乗るので、ここで数えないと実機の失敗ループは検出できない。
            repeat_guard.observe(&call.name, &call.args, &body);
            executed_in_round += 1;
            messages.push(ChatMessage::tool_result(&call.id, &call.name, body));
        }

        // **この周が丸ごと空振りだったときだけ**打ち切る。1 本が重複しただけの
        // 周は続ける — 並列で呼ばれた残りは新しい仕事をしている。
        // 上限到達の通知は出さない（当たったのは上限ではない。理由を 1 つに保つ）。
        if executed_in_round == 0
            && let Some(tool) = blocked_in_round
        {
            note!(
                "turn cut: agent={agent_id} round={} reason=repeat tool={tool}",
                iteration + 1,
            );
            repeat_stop = Some(tool);
            break;
        }

        // 上限に達したら、次の周回は回さずに今ある本文で終える。
        if iteration + 1 == max_tool_iterations {
            shared.emit(CoreEvent::ToolLimitReached {
                agent_id: agent_id.clone(),
                max_iterations: max_tool_iterations,
            });
            tool_limit_hit = true;
        }
    }

    // まとめ呼び出しが失敗したときの理由。フォールバック文言に載せる（#4 の規律:
    // 退避には落ちた事実・理由・復帰条件の 3 点を出口に付ける）。
    let mut summary_error: Option<String> = None;

    // ツール上限で打ち切られてテキストが無いときは、**ツールの使用を禁じて最後に
    // 1 回だけ呼び、ここまでの結果を文章化させる**。
    //
    // 中間のツール結果はこのターンの `messages` にしか存在せず、履歴には
    // 積まれない。まとめずに捨てると、利用者が「続けて」と送るたびに
    // ゼロから調査をやり直して同じ上限に当たり、トークンだけが燃え続ける
    // （実機で 3 ターン連続 146k tok を観測）。ここで 1 回のまとめ呼び出しに
    // 変換すれば、燃えたトークンの成果がそのまま答えになる。
    //
    // 繰り返しの打ち切り（failures.md #41 の処方 1）も同じ扱いにする。理由は同じで、
    // まとめずに終えると利用者が「続けて」と送り、同じ所まで走って同じ所で止まる。
    if let Outcome::Finish { content } = &outcome
        && content.trim().is_empty()
        && (tool_limit_hit || repeat_stop.is_some())
    {
        messages.push(ChatMessage::system(match &repeat_stop {
            Some(tool) => format!(
                "`{tool}` を同じ引数で繰り返し呼び、同じ結果が返り続けたため、\
                 ツール実行を打ち切りました。これ以上ツールは使えません。\
                 ここまでのツール結果から分かったことと、**何ができなかったのか**を\
                 最終回答としてまとめてください。同じ操作を勧める提案はしないでください。"
            ),
            None => "ツール実行の上限に達しました。これ以上ツールは使えません。\
                 ここまでのツール結果から分かったことを、最終回答としてまとめてください。\
                 調査が途中なら、どこまで分かっていて何が残っているかを書いてください。"
                .to_owned(),
        }));
        // ツールを取り上げるのは `tools` を消すことではなく `tool_choice` で縛る。
        // 履歴には直前のツール往復（tool_use / tool_result）が積まれたままなので、
        // `tools` を空にすると Anthropic が「tool ブロックを含むなら tools の定義が
        // 必須」の 400 を返し、**まとめはモデルに届く前にワイヤで死ぬ**
        // （実機で発生。failures.md #36）。定義は残し、使用だけを禁じる。
        let request = ChatRequest {
            model: template.model.clone(),
            messages: messages.clone(),
            tools: if use_tools { specs.clone() } else { Vec::new() },
            tool_choice: crate::llm::ToolChoice::None,
            temperature: template.temperature,
            max_tokens: template.max_output_tokens,
            effort: template.effort,
            cacheable_prefix_len: stable_len,
        };
        // まとめの失敗でターンごと落とさない。ただし**理由は握り潰さない** —
        // ここを `if let Ok` で書いていた間、まとめが落ちても理由はログにも
        // イベントにもフォールバック文言にも残らず、現場から診断不能だった。
        match backend.chat(request).await {
            Ok(mut response) => {
                // まとめ呼び出しの周。**ここは必ず書き込みになる** —
                // tool_choice を None へ変えると履歴層のキャッシュが落ちるため
                // （failures.md #42 の bounds）。0% でも異常ではない。
                note_cache_diag(
                    agent_id,
                    &template.model,
                    llm_rounds + 1,
                    &response.usage,
                    system_digest,
                    history_depth,
                );
                tokens += response.usage.total();
                cached += response.usage.cache_read;
                prompt += response.usage.prompt;
                grounding.absorb(std::mem::take(&mut response.grounding));
                match response.text {
                    Some(text) if !text.trim().is_empty() => {
                        outcome = Outcome::Finish { content: text };
                    }
                    // 本文が無いのにツール呼び出しがある = プロバイダが
                    // `tool_choice: none` を無視した。「空だった」に丸めると、
                    // モデルの不調と経路の不調が同じ文言になり切り分けられない
                    // （実機の flash-lite / 互換経路で「本文が空」を観測。
                    // この分岐はその容疑を次回から名指しするための計器）。
                    None | Some(_) if !response.tool_calls.is_empty() => {
                        summary_error = Some(format!(
                            "モデルが本文ではなくツール呼び出し（{}）で応えました。\
                             この経路は tool_choice の禁止指定を無視している可能性があります",
                            response
                                .tool_calls
                                .iter()
                                .map(|call| call.name.as_str())
                                .collect::<Vec<_>>()
                                .join("、")
                        ));
                    }
                    _ => {}
                }
            }
            Err(err) => summary_error = Some(err.to_string()),
        }
    }

    // それでも最終出力が空なら、正直な文言で置き換える。
    //
    // 空の発話を記録すると (1) UI に空バブルが出る (2) 履歴に空の assistant が
    // 積まれ、**次のターンの API リクエストが 400 (text content blocks must be
    // non-empty) で落ちてエージェントごと止まる**。空という値は連鎖的に
    // 毒になる（failures.md #29、実機で発生）。
    if let Outcome::Finish { content } = &mut outcome
        && content.trim().is_empty()
    {
        // 理由を必ず添える。「失敗しました」だけでは、設定を直せば済むのか
        // ワイヤの障害なのかを利用者が判別できない。
        let reason = || {
            summary_error
                .as_deref()
                .map(|err| format!("失敗の理由: {err}。"))
                .unwrap_or_else(|| "モデルは応答しましたが本文が空でした。".to_owned())
        };
        *content = if let Some(tool) = &repeat_stop {
            // 打ち切りの理由は上限ではないので、上限の直し方を案内しない
            // （直しても直らないものを勧めると、次の依頼がそのぶん燃える）。
            format!(
                "（`{tool}` を同じ引数で繰り返し呼び、同じ結果が返り続けたため\
                 ツール実行を打ち切りました。まとめの生成にも失敗しています。{}\
                 頼み方を変えるか、必要な情報を直接渡してください。）",
                reason()
            )
        } else if tool_limit_hit {
            format!(
                "（ツール実行の上限 {max_tool_iterations} 回に達し、まとめの生成にも\
                 失敗しました。{}\
                 エージェント設定で上限を上げるか、依頼を小さく分けてください。）",
                reason()
            )
        } else {
            "（モデルから本文が返りませんでした。もう一度頼んでみてください。）".to_owned()
        };
    }

    // 7. 統計と履歴を更新する。履歴には「実際に言ったこと」を積む。
    //    受信側は**送ったものをそのまま**積む — プロンプトと履歴の形を揃えないと、
    //    過去のターンだけ出所不明に戻るうえ、**その位置で前方一致が切れて
    //    キャッシュが頭打ちになる**（failures.md #45）。
    {
        let mut world = shared.world.write().await;
        if let Ok(record) = world.agent_mut(agent_id) {
            record.total_tokens += tokens;
            record.cached_tokens += cached;
            record.prompt_tokens += prompt;
            record.push_exchange(
                &sent_user_turn,
                &outcome.spoken(),
                shared.config.history_turns,
            );
        }
    }

    // 観測用のターン行（Spec 04 Notes 2 / Notes 12 のトリガー判定の実測材料）。
    // prompt の伸びは束ねの履歴肥大 (O(N²) 懸念) を、rounds は上限 12 の較正を、
    // waves は plan の利用実態を、それぞれ将来の判断のために記録する。
    // 機構は入れない — 測らずに入れると「効いているか分からない機構」が増える。
    // stop はループの抜け方。rounds が上限より小さいのに短く終わったターンを
    // 「モデルが早く答えた」と読み違えないために出す（繰り返しの打ち切りは
    // rounds を途中で止める唯一の機構）。
    let stop = match &repeat_stop {
        Some(tool) => format!("repeat:{tool}"),
        None if tool_limit_hit => "tool_limit".to_owned(),
        None => "-".to_owned(),
    };
    note!(
        "turn: agent={agent_id} hop={} rounds={llm_rounds}/{max_tool_iterations} \
         waves={plan_wave} stop={stop} prompt={prompt} cached={cached} total={tokens}",
        incoming.hop,
    );

    // 8. 記録と転送。
    let next_hop = incoming.hop.saturating_add(1);
    let from = Endpoint::Agent {
        id: agent_id.clone(),
    };

    let deliveries = match &outcome {
        Outcome::Finish { content } => {
            // 会話はここで終わり。ただし**誰へ返すか**は、頼まれ方で決まる。
            // 委譲（ask）で来た発話なら答えは依頼主へ戻る。通常配送ならユーザーへ。
            let destination = match &reply_to {
                Some(_) => incoming.from.clone(),
                None => Endpoint::User,
            };
            let mut outgoing = AgentMessage::new(from, destination, content, next_hop);
            outgoing.tokens = tokens as u32;
            // 接地の来歴は発話に添えて表示層へ渡す（`MessageSent` が運ぶ）。
            // プロンプトへは戻らない — 組み立て側は `content` しか読まない。
            outgoing.grounding = grounding;
            shared.record(outgoing).await;

            if let Some(reply_to) = reply_to {
                // 受け取り手が既に諦めている（タイムアウト）ことはあるので、
                // 送信の失敗は無視する。こちらの処理は完了している。
                let _ = reply_to.send(Reply {
                    text: content.clone(),
                    kind: PlanTaskState::Answered,
                });
            }
            return Ok(());
        }
        Outcome::Handoff { deliveries } => deliveries,
    };

    // 委譲（ask / plan）で来た依頼に、答えを返さず**転送で応じた**場合。
    //
    // `reply_to` は上の `Finish` 分岐でしか使われないため、ここで何もしないと
    // 送信側が drop されるだけになり、依頼主は「相手から答えが返りませんでした。」
    // を読む。**これは嘘である** — 答えは返っており、宛先が違うだけで会話は
    // 第三者へ渡っている（そして最終的にユーザーへ流れる）。
    //
    // 転送そのものは抑制しない。ワーカーの正当な選択を握り潰すと、
    // 「呼んだのに何も起きない」という別の穴に変わる。直すのは文言だけ。
    if let Some(reply_to) = reply_to {
        let names = {
            let world = shared.world.read().await;
            deliveries
                .iter()
                .map(|(to, _)| {
                    world
                        .agent(to)
                        .map(|record| record.spec.name.clone())
                        .unwrap_or_else(|_| to.to_string())
                })
                .collect::<Vec<_>>()
                .join("、")
        };
        let _ = reply_to.send(Reply {
            text: format!(
                "相手はこの依頼に自分で答えず、{names} へ会話を渡しました。\
                 答えはこちらへ戻りません。必要なら別の相手に頼むか、自分で進めてください。"
            ),
            kind: PlanTaskState::HandedOff,
        });
    }

    // 宛先ごとに 1 通として記録する（fan-out）。トークンは 1 ターンぶんの消費なので、
    // 全通に載せると宛先数で二重計上される。先頭の 1 通にだけ載せる。
    let mut queued = Vec::with_capacity(deliveries.len());
    for (index, (to, message)) in deliveries.iter().enumerate() {
        let mut outgoing = AgentMessage::new(
            from.clone(),
            Endpoint::Agent { id: to.clone() },
            message,
            next_hop,
        );
        outgoing.tokens = if index == 0 { tokens as u32 } else { 0 };
        // 接地も 1 ターンぶんの事実なので、トークンと同じく先頭の 1 通にだけ載せる。
        // 全通に複製すると、表示で畳んだあとも同じ出典が宛先数ぶん並ぶ。
        if index == 0 {
            outgoing.grounding = std::mem::take(&mut grounding);
        }

        // 同じ内容を複数宛先へ渡す fan-out は、受け手から見ればエージェント発の
        // 同報。宛先一覧を封筒に載せ、受け手同士が「相手はこれを知らない」と
        // 誤解して伝言し合う経路（ユーザー同報の反響と同型）を塞ぐ。
        // 内容が宛先ごとに違う配送は同報ではないので載せない —
        // 「全員が同じ内容を受け取っている」という注記が嘘になる。
        let same_content: Vec<AgentId> = deliveries
            .iter()
            .filter(|(_, m)| m == message)
            .map(|(t, _)| t.clone())
            .collect();
        if same_content.len() >= 2 {
            outgoing.co_recipients = same_content;
        }

        shared.record(outgoing.clone()).await;
        queued.push((to, outgoing));
    }

    // 燃料切れの判定は宛先共通（同じターン由来なので hop も同じ）。
    // 記録は済ませてから打ち切る——発話自体は起きたのだから、ログには残す。
    if next_hop >= shared.config.max_hops {
        shared.emit(CoreEvent::HopLimitReached {
            agent_id: agent_id.clone(),
            max_hops: shared.config.max_hops,
        });
        return Ok(());
    }

    // 転送失敗（宛先停止中・受信箱飽和）は、このエージェント自身の失敗ではない。
    // 自分を Failed に落とさず、事象として通知するに留める。
    // 1 宛先の失敗で残りを道連れにしない——枝は独立している。
    for (to, outgoing) in queued {
        if let Err(err) = deliver(shared, to, outgoing).await {
            shared.emit(CoreEvent::AgentFailed {
                agent_id: to.clone(),
                error: ErrorPayload::from(&err),
            });
        }
    }

    Ok(())
}

/// 共有ツールと個別 MCP ツールを 1 つの集合へ畳む（純関数）。
///
/// 同名は個別が勝つ（上書き可能な加算）。順序は共有 → 個別で安定させる。
fn merge_tool_specs(shared_specs: Vec<ToolSpec>, personal: Vec<ToolSpec>) -> Vec<ToolSpec> {
    if personal.is_empty() {
        return shared_specs;
    }
    let mut merged: Vec<ToolSpec> = shared_specs
        .into_iter()
        .filter(|spec| !personal.iter().any(|p| p.name == spec.name))
        .collect();
    merged.extend(personal);
    merged
}

/// エージェント別 MCP を接続して登録する（Spec 02）。
///
/// 読み込み失敗（外部編集で壊れた mcp.json = 失敗二分類 (1')）でも
/// エージェントの起動は止めない。個別ツール 0 本で稼働し、失敗理由は
/// [`AgentMcpStatus::load_error`] として読める。
async fn connect_agent_mcp(shared: &Shared, id: &AgentId) {
    let state = match shared.store.read_agent_mcp_config(id).await {
        Ok(config) => AgentMcpState {
            manager: crate::mcp::McpManager::connect_all(&config).await,
            load_error: None,
        },
        Err(err) => AgentMcpState {
            manager: crate::mcp::McpManager::default(),
            load_error: Some(err.to_string()),
        },
    };
    shared.agent_mcp.write().await.insert(id.clone(), state);
}

/// 同梱ツールをこのエージェントへ提示するか（enabled_tools_invariant）。
///
/// - 同梱ツール以外（MCP 由来）は常に提示（このフィルタの対象外）
/// - 作業フォルダが要るツールは、未設定なら enabled_tools に関わらず
///   提示しない（自動除外が明示より優先。使えないツールを見せない）
/// - enabled_tools が None なら既定 = 全提示、Some なら列挙分だけ
fn is_bundled_tool_presented(name: &str, spec: &AgentSpec) -> bool {
    if !crate::tools::BUNDLED_TOOL_NAMES.contains(&name) {
        return true;
    }
    if crate::tools::WORK_DIR_TOOL_NAMES.contains(&name) && spec.work_dir.is_none() {
        return false;
    }
    match &spec.enabled_tools {
        None => true,
        Some(enabled) => enabled.iter().any(|tool| tool == name),
    }
}

/// ツールを 1 本実行する。
///
/// 未知の名前でも `Err` にせず文字列を返すのは、モデルが読んで直せるようにするため。
/// ここで会話ごと落とすと、名前を打ち間違えただけでターンが終わる。
async fn execute_tool(
    shared: &Arc<Shared>,
    agent_id: &AgentId,
    call: &crate::llm::ToolCall,
) -> CoreResult<String> {
    // 実行解決は提示と同じ規則の逆引き: **個別 MCP を先に**引き、
    // 無ければ共有 registry（同名は個別が勝つ）。個別ツールは registry に
    // 入っていないため、他エージェントからは名前を知っていても実行できない。
    let personal = {
        let map = shared.agent_mcp.read().await;
        map.get(agent_id).and_then(|state| {
            state
                .manager
                .tools()
                .iter()
                .find(|tool| tool.name() == call.name)
                .cloned()
        })
    };
    let tool = match personal {
        Some(tool) => Some(tool),
        None => shared.tools.read().await.get(&call.name).cloned(),
    };

    let Some(tool) = tool else {
        return Ok(format!(
            "`{}` というツールはありません。提示された名前から選んでください。",
            call.name
        ));
    };

    // 作業フォルダ（grep / diff の探索範囲）は呼び出しの瞬間に解決する。
    // ツール登録時に固定すると、設定変更が次の再登録まで効かない。
    let work_dir = {
        let world = shared.world.read().await;
        world
            .agent(agent_id)
            .ok()
            .and_then(|record| record.spec.work_dir.clone())
            .map(std::path::PathBuf::from)
    };

    let ctx = ToolContext {
        agent_id: agent_id.clone(),
        work_dir,
    };
    tool.call(&ctx, &call.args).await
}

/// 他のエージェントへ質問し、**答えを待って**返す（委譲）。
///
/// 転送との違いは行き先だけ。転送は制御ごと渡してユーザーへ返るが、委譲は
/// 答えが呼び出し元へ戻り、ツール結果として会話が続く。
///
/// **必ず有限時間で戻る。** 相手が応答しない・相互に委譲し合う配置では
/// 待ち合わせが起きうるので、上限で打ち切って理由を文字列で返す
/// （ツールの失敗は会話を止めない、という既存の規律に合わせる）。
async fn ask_agent(
    shared: &Arc<Shared>,
    from: &AgentId,
    to: &AgentId,
    call: &crate::llm::ToolCall,
    hop: u8,
    parent: &tokio_util::sync::CancellationToken,
) -> CoreResult<String> {
    let question = call
        .args
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_owned();

    let next_hop = hop.saturating_add(1);
    if next_hop >= shared.config.max_hops {
        shared.emit(CoreEvent::HopLimitReached {
            agent_id: from.clone(),
            max_hops: shared.config.max_hops,
        });
        return Ok("転送の上限に達したため、これ以上は尋ねられません。".to_owned());
    }

    // ask は分類を捨てる（.0）。分類は波ペインの素材で、ask の関心ではない。
    Ok(deliver_and_wait(shared, from, to, &question, next_hop, parent)
        .await
        .0)
}

/// 1 件の依頼を配送し、答えを待つ（`ask` と `plan` の共通部分）。
///
/// **切り出してあるのは、2 つの経路で失敗の文言と境界を揃えるため。**
/// 別々に書くと、同じ配置で ask は通り plan は止まる、という説明できない差が
/// いずれ生まれる。`hop` の判定は呼び出し側に置く — plan では波全体で
/// 一様に決まる制約なので、タスクごとに判定すると同じ文字列が人数分並ぶ。
///
/// 戻り値は**必ず文字列と分類の組**。相手が停止中でも無応答でも例外にしない
/// （ツールの失敗で会話を止めない、という既存の規律）。分類（Spec 08）は
/// 波ペインのセル色の素材で、`ask` 側は捨てるだけ — 計時も同じ理由で
/// ここに入れない（plan の観測の関心を ask に背負わせない）。
async fn deliver_and_wait(
    shared: &Arc<Shared>,
    from: &AgentId,
    to: &AgentId,
    question: &str,
    next_hop: u8,
    parent: &tokio_util::sync::CancellationToken,
) -> (String, PlanTaskState) {
    let mut outgoing = AgentMessage::new(
        Endpoint::Agent { id: from.clone() },
        Endpoint::Agent { id: to.clone() },
        question,
        next_hop,
    );
    // 質問自体のトークンは呼び出し元のターンに計上済み。二重計上しない。
    outgoing.tokens = 0;
    shared.record(outgoing.clone()).await;

    let (tx, rx) = tokio::sync::oneshot::channel();
    let envelope = Envelope {
        incoming: outgoing,
        reply_to: Some(tx),
        // 依頼元ターンの子（Spec 10 Phase 2）。依頼元が切られたら、この封筒が
        // 生んだ仕事（未着手の畳み・飛行中の検知）だけが連鎖して止まる。
        // 受信側の別の依頼は別トークンなので巻き添えにならない。
        cancel: Some(parent.child_token()),
    };

    if let Err(err) = deliver_envelope(shared, to, envelope).await {
        // 相手が停止中・受信箱が飽和。会話は止めず、モデルに事実を返す。
        return (
            format!("相手に尋ねられませんでした: {err}"),
            PlanTaskState::Undeliverable,
        );
    }

    match tokio::time::timeout(shared.config.ask_timeout, rx).await {
        // 答え（Answered）か転送の事実（HandedOff）。刻み手は handle_message。
        Ok(Ok(reply)) => (reply.text, reply.kind),
        // 相手が答えずにタスクを終えた（停止・失敗）。転送で応じた場合は
        // handle_message が事実を送るので、ここへは来ない。
        Ok(Err(_)) => (
            "相手から答えが返りませんでした。".to_owned(),
            PlanTaskState::NoAnswer,
        ),
        Err(_) => (
            "相手からの答えが時間内に返りませんでした。".to_owned(),
            PlanTaskState::TimedOut,
        ),
    }
}

/// 並列委譲（`plan`）を 1 波ぶん実行する（Spec 04）。
///
/// # 失敗の 3 分類
///
/// 処方が分かれる根拠は「**その値がいつ確定するか**」の 1 点だけ:
///
/// - **静的な不正**（波の中で不変・事前に確かめられる）→ **何も配送せず差し戻す**
/// - **波全体で一様な制約**（波の中で不変・全タスクが同値）→ **1 つの結果文字列**
/// - **動的な失敗**（配送の瞬間まで確定しない）→ **そのタスクの結果文字列**
///
/// 部分実行を避けるのは、「どこまで走ったか」の追跡を利用者に強いるから。
/// ただし稼働状態は**確かめても配送時には別の値でありうる**ので検証に含めない。
/// 確かめられないものを検証に含めると、嘘の保証になる。
///
/// 戻り値は 3 分類のいずれも `String`。エラーチャネルを使わないのは、
/// `Err` を返すと実行ループが「ツールの実行に失敗しました」で包み、
/// モデルが読むべき「なぜ配送されなかったか」が一段深い所へ埋まるため。
async fn run_plan(
    shared: &Arc<Shared>,
    from: &AgentId,
    handoffs: &HandoffTools,
    call: &crate::llm::ToolCall,
    hop: u8,
    wave: u32,
    parent: &tokio_util::sync::CancellationToken,
) -> String {
    // 1. 静的な不正を全件見る。1 件でも不正なら何も配送しない。
    let Some(tasks) = call.args.get("tasks").and_then(serde_json::Value::as_array) else {
        return "plan には tasks（依頼の配列）が必要です。何も配送していません。".to_owned();
    };
    if tasks.is_empty() {
        return "plan の tasks が空です。誰にも頼まずに終わりました。\
                頼む相手が居ないなら、plan を呼ばずに自分で答えてください。"
            .to_owned();
    }

    let mut wave_tasks: Vec<(AgentId, String)> = Vec::with_capacity(tasks.len());
    for (index, task) in tasks.iter().enumerate() {
        let position = index + 1;
        let (Some(to), Some(message)) = (
            task.get("to").and_then(serde_json::Value::as_str),
            task.get("message").and_then(serde_json::Value::as_str),
        ) else {
            return format!(
                "{position} 件目の依頼に to と message の両方が必要です。何も配送していません。"
            );
        };

        let target = AgentId::from(to);
        // 提示はターンの開始時、検証は今。この間に繋ぎ替えは起こりうる。
        if !handoffs.is_target(&target) {
            return format!(
                "{position} 件目の宛先「{to}」は、あなたの接続先ではありません。\
                 頼めるのは {} です。何も配送していません。",
                handoffs.roster().join("、")
            );
        }
        if wave_tasks.iter().any(|(existing, _)| *existing == target) {
            return format!(
                "宛先「{to}」が同じ波に 2 回あります。1 回の plan で同じ相手へ頼めるのは 1 件です。\
                 2 件目は次の波で頼んでください。何も配送していません。"
            );
        }
        wave_tasks.push((target, message.to_owned()));
    }

    // 2. 波全体で一様に決まる制約。1 回だけ確かめ、1 つの文字列で返す
    //    （タスク数ぶん同じ文字列を並べない）。判定式は ask_agent と同一。
    let next_hop = hop.saturating_add(1);
    if next_hop >= shared.config.max_hops {
        shared.emit(CoreEvent::HopLimitReached {
            agent_id: from.clone(),
            max_hops: shared.config.max_hops,
        });
        return "転送の上限に達したため、これ以上は頼めません。何も配送していません。".to_owned();
    }

    // 観測用の波の開始行。宛先と依頼サイズを記録し、後から
    // 「この波は前の波の結果を読まずに書けたか」を追えるようにする
    // （Spec 04 Notes 2 の depends_on トリガー判定の材料）。
    note!(
        "plan wave: agent={from} wave={wave} tasks={} to=[{}] msg_chars={}",
        wave_tasks.len(),
        wave_tasks
            .iter()
            .map(|(target, _)| target.as_str())
            .collect::<Vec<_>>()
            .join(","),
        wave_tasks
            .iter()
            .map(|(_, message)| message.chars().count())
            .sum::<usize>(),
    );
    let dispatched_at = std::time::Instant::now();

    // 波の記録と告知（Spec 08）。配送ゼロの plan はここへ到達しない（上の
    // 早期 return）ので、記録と stderr の数え方は構造的に一致する。
    // 開始時刻だけが壁時計（epoch ms）、所要はすべて単調時計（Instant）。
    let started_at_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
    let announced: Vec<(AgentId, u32)> = wave_tasks
        .iter()
        .map(|(target, message)| (target.clone(), message.chars().count() as u32))
        .collect();
    let plan_id = shared
        .plan_waves
        .write()
        .await
        .begin_wave(from.clone(), wave, &announced, started_at_ms);
    shared.emit(CoreEvent::PlanWaveStarted {
        plan_id,
        agent_id: from.clone(),
        wave,
        tasks: announced
            .iter()
            .map(|(to, msg_chars)| PlanTaskAnnounced {
                to: to.clone(),
                msg_chars: *msg_chars,
            })
            .collect(),
        started_at_ms,
    });

    // 3. 並列配送。JoinSet で各タスクを実行時へ載せる — ここが `ask_*` の
    //    直列委譲との唯一の構造的な差で、壁時計が人数倍にならない理由。
    //    並列なのは**配送**であって実行ではない。各エージェントの受信箱は
    //    1 本なので、ワーカーが別の仕事で塞がっていればその分だけ待つ。
    //    タスクの所要はここで測る — deliver_and_wait に計時を入れない
    //    （ask に plan の観測の関心を背負わせない）。
    let mut set = tokio::task::JoinSet::new();
    for (index, (target, message)) in wave_tasks.iter().enumerate() {
        let shared = Arc::clone(shared);
        let from = from.clone();
        let target = target.clone();
        let message = message.clone();
        let parent = parent.clone();
        set.spawn(async move {
            let task_started = std::time::Instant::now();
            let (answer, state) =
                deliver_and_wait(&shared, &from, &target, &message, next_hop, &parent).await;
            (index, answer, state, task_started.elapsed().as_millis() as u64)
        });
    }

    // 進行役のターンが切られたら、波の待ちもここで畳む（Spec 10 — U2）。
    // 周回境界の検査だけでは、最悪 ask_timeout（既定 180 秒）が割り込み不能の
    // まま残る。ワーカー側は封筒の子トークンが同じ cancel で連鎖して止まるので、
    // ここで待ち続けても新しい答えは（打ち切りの報告以外）もう来ない。
    let mut wave_interrupted = false;
    let mut answers: Vec<Option<String>> = vec![None; wave_tasks.len()];
    loop {
        tokio::select! {
            biased;

            () = parent.cancelled() => {
                wave_interrupted = true;
                break;
            }
            joined = set.join_next() => {
                let Some(joined) = joined else { break };
                match joined {
                    Ok((index, answer, state, elapsed_ms)) => {
                        // 解決した順に記録と event を刻む。セルは波の完了を待たず
                        // 個別に色が変わる（全滅まで灰色、にしない）。
                        let to = wave_tasks[index].0.clone();
                        shared
                            .plan_waves
                            .write()
                            .await
                            .resolve_task(plan_id, &to, state, elapsed_ms);
                        shared.emit(CoreEvent::PlanTaskResolved {
                            plan_id,
                            to,
                            state,
                            elapsed_ms,
                        });
                        answers[index] = Some(answer);
                    }
                    // タスク自体が落ちた（パニック）。1 件の異常で波ごと落とさない。
                    // 記録上は finish_wave が Running を NoAnswer に倒す。
                    Err(err) => tracing_note(&err),
                }
            }
        }
    }

    if wave_interrupted {
        // set の drop で残りの待ちを畳む。配送済みの封筒はそのまま — ワーカーは
        // 子トークンで自分の周回境界（または着手時）に止まる。答えは受け取らない
        // （部分的な束ねを作らない — 束ねると次のターンの進行役が「全員から
        // 答えが揃った」と誤読する）。
        drop(set);

        // 未解決のタスクを interrupted で確定させ、波を閉じる。倒し先が
        // no_answer でないのは、答えなかったのではなく止めさせたから。
        // frontend は planWaveFinished で残った running を no_answer に倒すので、
        // その前に 1 件ずつ resolve を流して running を残さない。
        let folded_at = dispatched_at.elapsed().as_millis() as u64;
        for (index, (to, _)) in wave_tasks.iter().enumerate() {
            if answers[index].is_none() {
                shared
                    .plan_waves
                    .write()
                    .await
                    .resolve_task(plan_id, to, PlanTaskState::Interrupted, folded_at);
                shared.emit(CoreEvent::PlanTaskResolved {
                    plan_id,
                    to: to.clone(),
                    state: PlanTaskState::Interrupted,
                    elapsed_ms: folded_at,
                });
            }
        }
        // 束ねは作らなかったので 0 文字（「何も束ねていない」の正直な大きさ）。
        note!(
            "plan wave interrupted: agent={from} wave={wave} resolved={}/{}",
            answers.iter().filter(|a| a.is_some()).count(),
            wave_tasks.len(),
        );
        shared
            .plan_waves
            .write()
            .await
            .finish_wave(plan_id, 0, folded_at);
        shared.emit(CoreEvent::PlanWaveFinished {
            plan_id,
            bundle_chars: 0,
            elapsed_ms: folded_at,
        });

        // この文字列は進行役の周回に返るが、直後の周回境界で本人も止まるので
        // モデルは読まない。読まれる前提の文言にしない（人がログで読む行）。
        return "plan はユーザーの指示で打ち切られました。".to_owned();
    }

    // 4. 束ねる。見出しは `agent_id（表示名）` — 表示名だけにしないのは、
    //    表示名の一意性がどこも保証されていないから（同名が 2 体いると
    //    どちらの答えか判別できなくなる）。順序は入力順に戻す。
    let bundle = wave_tasks
        .iter()
        .zip(answers)
        .map(|((target, _), answer)| {
            let display = handoffs.display_of(target).unwrap_or_else(|| target.as_str());
            let body = answer.unwrap_or_else(|| "答えの取得中に問題が起きました。".to_owned());
            format!("## {target}（{display}）\n{body}")
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    // 束ねの大きさを記録する（Spec 04 Notes 7 の「実測してから決める」の実測側）。
    // 束ねは進行役の履歴に積まれ、以後の波のたびに入力として運ばれる —
    // 波数 × N 体で膨らむ構造なので、上限や要約を入れるかの判断材料をここで取る。
    // 機構は入れない。測らずに入れると「効いているか分からない機構」が増えるだけ。
    let bundle_chars = bundle.chars().count() as u64;
    let elapsed_ms = dispatched_at.elapsed().as_millis() as u64;
    note!(
        "plan bundle: agent={from} wave={wave} tasks={} chars={bundle_chars} \
         elapsed_ms={elapsed_ms}",
        wave_tasks.len(),
    );

    // 波の完了（Spec 08）。Running のまま残ったタスク（JoinSet パニックの経路
    // のみ）は finish_wave が NoAnswer に倒す — 完了した波に永遠の「実行中」を
    // 残さない。
    shared
        .plan_waves
        .write()
        .await
        .finish_wave(plan_id, bundle_chars, elapsed_ms);
    shared.emit(CoreEvent::PlanWaveFinished {
        plan_id,
        bundle_chars,
        elapsed_ms,
    });
    bundle
}

/// `JoinSet` のタスク異常を握り潰さずに記録する。
///
/// このクレートはログ基盤を持たない（GUI 層に一切依存しない制約）ので、
/// 標準エラーへ 1 行出すに留める。**黙って捨てない**ことだけが目的。
fn tracing_note(err: &tokio::task::JoinError) {
    note!("plan のタスクが異常終了しました: {err}");
}

/// 入退室の通知を組み立てる（Spec 06 P1）。
///
/// System 発の発話（`set_status` が記録する入退室）だけを抽出する。
/// [`compose_room_log`] と別の関数なのは gate が違うから — 広場ログは
/// `hearsRoomLog` でオプトアウトできるが、こちらは全員に届く。
///
/// # 可視範囲
///
/// **広場ログと同じ窓**（`room_log_window` 件の遡り）に従う。窓から押し出された
/// 通知は見えなくなるが、情報が消えるのではなく時間軸だけが落ちる —
/// 現在の状態は顔ぶれ（P1.5）が常に持っている（顔ぶれが権威、通知が語り）。
async fn compose_presence_notices(
    shared: &Shared,
    config: &OrchestratorConfig,
    has_roster: bool,
) -> Option<String> {
    if config.room_log_window == 0 {
        return None;
    }

    let lines: Vec<String> = {
        let log = shared.log.read().await;
        // 生ログの直近 window 件の中から System 発を拾う。
        // 「System 発だけを window 件」にはしない — それだと古い通知が
        // 会話に押し出されずいつまでも残り、「窓に従う」という契約が嘘になる。
        log.iter()
            .rev()
            .take(config.room_log_window)
            .filter(|message| message.from == Endpoint::System)
            .map(|message| format!("- {}", message.content))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    };

    if lines.is_empty() {
        return None;
    }

    // 「顔ぶれが権威、通知が語り」の案内は、顔ぶれの節が実際に出ている
    // 相手にだけ書く。接続 0 体の個体に存在しない節を指させない。
    let authority_note = if has_roster {
        "\n\n現在の状態は「今の顔ぶれ」が正です。"
    } else {
        ""
    };
    Some(format!(
        "## 入退室（新しいものが下）\n{}{authority_note}",
        lines.join("\n")
    ))
}

/// 「居合わせた会話」を組み立てる（広場ログ）。
///
/// # なぜ「聞こえる」と「反応する」を分けるのか
///
/// 各エージェントの履歴は私的で、他人の発言は一切見えなかった。だが村の広場では、
/// 話は自分宛でなくても聞こえる。かといって**聞こえるたびに反応させると
/// 反響が起き、トークンが人数分燃える**（failures.md #20）。
/// そこで配送（＝ターンの発火）は宛先だけに保ち、**可視性だけを共有する**。
/// これがこの関数の役割で、ここに載る発話はターンを発火させない。
///
/// # 何を載せないか
///
/// **ユーザーが宛先を選んだ発話は載せない。** ユーザーは聴衆を選んで話しており、
/// 広場ログがその選択を迂回する裏口になってはいけない
/// （「宛先外のエージェントはメッセージがあったことすら知らないべき」）。
/// 自分が送り手・受け手である発話も載せない — それは既に自分の履歴にある。
async fn compose_room_log(
    shared: &Shared,
    agent_id: &AgentId,
    config: &OrchestratorConfig,
) -> Option<String> {
    if config.room_log_window == 0 {
        return None;
    }

    let overheard: Vec<AgentMessage> = {
        let log = shared.log.read().await;
        log.iter()
            .rev()
            .filter(|message| {
                // エージェント発の発話だけ。ユーザー発は聴衆が選ばれている。
                let from_agent = matches!(message.from, Endpoint::Agent { .. });
                let is_mine = message.from == (Endpoint::Agent { id: agent_id.clone() })
                    || message.to == (Endpoint::Agent { id: agent_id.clone() });
                from_agent && !is_mine
            })
            .take(config.room_log_window)
            .cloned()
            .collect()
    };

    if overheard.is_empty() {
        return None;
    }

    let world = shared.world.read().await;
    let label = |endpoint: &Endpoint| -> String {
        match endpoint {
            Endpoint::User => "ユーザー".to_owned(),
            // UI と同じ名前（語彙の二重化を作らない）。
            Endpoint::System => "Concordia".to_owned(),
            Endpoint::Agent { id } => world
                .agent(id)
                .map(|record| record.spec.name.clone())
                .unwrap_or_else(|_| id.to_string()),
        }
    };

    // 収集は新しい順なので、表示は古い順へ戻す。
    let lines: Vec<String> = overheard
        .iter()
        .rev()
        .map(|message| {
            let excerpt = truncate_chars(&message.content, config.room_log_excerpt_chars);
            format!(
                "- {} → {}: {}",
                label(&message.from),
                label(&message.to),
                excerpt
            )
        })
        .collect();

    Some(format!(
        "## この場で交わされていた会話\n\
         あなた宛ではありませんが、同じ場に居たので聞こえていた発言です。\
         **返事をする義務はありません。** 文脈として使ってください。\n{}",
        lines.join("\n")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_named(name: &str) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: String::new(),
            parameters: serde_json::json!({ "type": "object" }),
        }
    }

    /// 同名は個別が勝つ（上書き可能な加算）。順序は共有 → 個別。
    #[test]
    fn personal_tools_override_shared_ones_by_name() {
        let shared = vec![spec_named("grep"), spec_named("memo__recall")];
        let personal = vec![spec_named("memo__recall"), spec_named("memo__store")];

        let merged = merge_tool_specs(shared, personal);
        let names: Vec<&str> = merged.iter().map(|s| s.name.as_str()).collect();

        assert_eq!(names, vec!["grep", "memo__recall", "memo__store"]);
        assert_eq!(
            merged.iter().filter(|s| s.name == "memo__recall").count(),
            1,
            "同名は 1 本に畳まれ、個別側が残る"
        );
    }

    /// 同じ呼び出し + 同じ結果が 2 回続いたら、3 回目は実行しない。
    #[test]
    fn the_third_identical_call_is_blocked() {
        let args = serde_json::json!({ "path": "README.en.md" });
        let err = "`README.en.md` を読めません: 見つかりません";
        let mut guard = RepeatGuard::default();

        assert!(!guard.blocks("file", &args), "1 回目は素通し");
        guard.observe("file", &args, err);
        assert!(!guard.blocks("file", &args), "2 回目も素通し（1 回では判定しない）");
        guard.observe("file", &args, err);

        assert!(guard.blocks("file", &args), "3 回目は実行しない");
        assert_eq!(guard.repeats("file", &args), 2);
    }

    /// **間に別の呼び出しが挟まっても数えは切れない。**
    ///
    /// 隣接だけを見ていた最初の実装は、実機で 1 件も発火しなかった
    /// （2026-07-31 のログ: `file(A)` が round 24・25・28 に出たが、26 の
    /// `grep` で数えが切れて 3 回目が素通しした）。
    #[test]
    fn an_interleaved_call_does_not_clear_the_count() {
        let sd = serde_json::json!({ "pattern": "a", "replacement": "b" });
        let grep = serde_json::json!({ "pattern": "a" });
        let mut guard = RepeatGuard::default();

        guard.observe("sd", &sd, "対象がありません");
        guard.observe("grep", &grep, "一致なし");
        guard.observe("sd", &sd, "対象がありません");

        assert!(
            guard.blocks("sd", &sd),
            "呼び出しごとに数えるので、挟まれても 2 回は 2 回"
        );
        assert!(!guard.blocks("grep", &grep), "挟まった側は 1 回のまま");
    }

    /// 1 周に並列で複数本呼ばれても同じ（実機の主な形）。
    ///
    /// 2026-07-31 のログ: round 2 と round 3 で同じ `file(B)` が呼ばれ、
    /// round 3 は 3 本の並列呼び出しだった。隣接判定はここで必ず切れる。
    #[test]
    fn parallel_calls_in_one_round_do_not_clear_the_count() {
        let target = serde_json::json!({ "op": "read", "path": "README.md" });
        let other = serde_json::json!({ "op": "read", "path": "CLAUDE.md" });
        let third = serde_json::json!({ "op": "read", "path": "failures.md" });
        let body = "（12,045 字の本文）";
        let mut guard = RepeatGuard::default();

        // round 2
        guard.observe("file", &target, body);
        // round 3（並列 3 本。同じ読み直しが 1 本目に混ざる）
        guard.observe("file", &target, body);
        guard.observe("file", &other, "別の本文");
        guard.observe("file", &third, "また別の本文");

        assert!(guard.blocks("file", &target), "round 5 の 3 回目は実行しない");
        assert!(!guard.blocks("file", &other), "他の読み込みは巻き添えにしない");
    }

    /// 成功でも同じことが起きる。同じ入力に同じ出力が返るなら 3 回目に新しい
    /// 情報は無い（同梱ツールは失敗も `Ok` の本文で返すため、失敗と成功を
    /// 区別する材料がそもそも無い）。
    #[test]
    fn identical_successes_are_blocked_too() {
        let args = serde_json::json!({ "pattern": "fn main" });
        let mut guard = RepeatGuard::default();

        guard.observe("grep", &args, "src/main.rs:1: fn main() {");
        guard.observe("grep", &args, "src/main.rs:1: fn main() {");

        assert!(guard.blocks("grep", &args));
    }

    /// 結果が変われば止めない。**同じ操作が実を結んでいる**（追記が進む・
    /// 待っていた状態が変わる）ので、繰り返し自体は正当。
    ///
    /// 隣接を捨てた後もここは守る必要がある。「呼び出しごとの通算回数」で
    /// 数えると、実を結んでいる追記まで 3 回目で止まってしまう。
    #[test]
    fn a_changed_result_clears_the_count() {
        let args = serde_json::json!({ "op": "append", "path": "log.md" });
        let mut guard = RepeatGuard::default();

        guard.observe("file", &args, "1 行追記しました。");
        guard.observe("file", &args, "1 行追記しました。");
        guard.observe("file", &args, "2 行追記しました。");

        assert!(!guard.blocks("file", &args), "結果が変わったら数え直す");
        assert_eq!(guard.repeats("file", &args), 1);
    }

    /// 引数が変われば別の呼び出しとして数える。**別の場所を試している**のは
    /// 行き詰まりではない。数えは呼び出しごとに独立しているので、
    /// 一方を止めてももう一方は素通しする。
    #[test]
    fn each_argument_is_counted_independently() {
        let first = serde_json::json!({ "path": "a.md" });
        let second = serde_json::json!({ "path": "b.md" });
        let err = "読めません";
        let mut guard = RepeatGuard::default();

        guard.observe("file", &first, err);
        guard.observe("file", &first, err);
        guard.observe("file", &second, err);

        assert!(!guard.blocks("file", &second), "宛先を変えた失敗は繰り返しではない");
        assert!(guard.blocks("file", &first), "止まっている側だけを止める");
    }

    /// 引数の一致はキーの並びに依存しない（`serde_json::Value` の等価は
    /// 中身で決まる）。プロバイダが並べ替えて返しても同じ呼び出しと数える。
    #[test]
    fn argument_equality_ignores_key_order() {
        let a = serde_json::json!({ "op": "read", "path": "x.md" });
        let b = serde_json::json!({ "path": "x.md", "op": "read" });
        let err = "読めません";
        let mut guard = RepeatGuard::default();

        guard.observe("file", &a, err);
        guard.observe("file", &b, err);

        assert!(guard.blocks("file", &a), "キーの並びで別物にしない");
    }
}

/// 文字数で切り詰める。マルチバイト文字の途中で切らない。
fn truncate_chars(text: &str, limit: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= limit {
        return trimmed.to_owned();
    }
    let head: String = trimmed.chars().take(limit).collect();
    format!("{head}…")
}

/// 1 回の応答の行き先。
#[derive(Debug, Clone, PartialEq)]
enum Outcome {
    /// 会話終了。ユーザーへ返す。
    Finish {
        /// 本文。
        content: String,
    },
    /// 転送して会話を続ける。
    ///
    /// 宛先は複数持てる（fan-out）。かつて単一宛先の型だったときは、
    /// モデルが並列ツール呼び出しで複数へ渡そうとしても 2 本目以降が
    /// 黙って捨てられ、「みんなに挨拶して」が原理的に成立しなかった。
    Handoff {
        /// 宛先と、それぞれへ伝える内容。空にはならない（`decide` が保証）。
        deliveries: Vec<(AgentId, String)>,
    },
}

impl Outcome {
    /// このターンで実際に発した言葉。履歴へ積むのはこちら。
    ///
    /// 複数宛先のときは宛先を添えて結合する。履歴を読むのは本人（モデル）なので、
    /// 「誰に何を言ったか」が残らないと、次のターンで自分の発言を再構成できない。
    fn spoken(&self) -> String {
        match self {
            Self::Finish { content } => content.clone(),
            Self::Handoff { deliveries } => match deliveries.as_slice() {
                [(_, message)] => message.clone(),
                many => many
                    .iter()
                    .map(|(to, message)| format!("[→ {to}] {message}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            },
        }
    }
}

/// 転送先ごとのツール定義と、その逆引き。
struct HandoffTools {
    /// `(ツール名, 転送先, 表示名)`。名前からの逆引きと、説明文の生成に使う。
    entries: Vec<(String, AgentId, String)>,
}

impl HandoffTools {
    /// 接続先からツール名を導く。
    ///
    /// 名前は OpenAI Agents SDK の慣習に倣って `transfer_to_<agent>`。
    /// 関数名の長さ制限（64 文字）を超える場合と、切り詰めで衝突する場合は
    /// 連番へ退避する。名前が壊れるとモデルが呼べなくなるため、
    /// 「たぶん大丈夫」で通さない。
    ///
    /// **ツール名は ID、説明は表示名**という組み合わせを採る。関数名に使えるのは
    /// `[a-zA-Z0-9_-]` だけで、日本語の表示名は潰れて識別できなくなる。一方で
    /// 会話は表示名で流れるので、説明に名前が無いとモデルは
    /// 「ザリ・ロブステル」と `agent_2` を結び付けられない。実際にそうなっており、
    /// 宛先の取り違えと「自分で全員のセリフを書く」の原因になっていた。
    fn build(targets: &[(AgentId, String)]) -> Self {
        const MAX_TOOL_NAME: usize = 64;
        const PREFIX: &str = "transfer_to_";

        let mut entries: Vec<(String, AgentId, String)> = Vec::with_capacity(targets.len());
        for (index, (target, display)) in targets.iter().enumerate() {
            let natural = format!("{PREFIX}{target}");
            let name = if natural.len() <= MAX_TOOL_NAME
                && !entries.iter().any(|(existing, _, _)| *existing == natural)
            {
                natural
            } else {
                format!("{PREFIX}agent_{index}")
            };
            entries.push((name, target.clone(), display.clone()));
        }
        Self { entries }
    }

    /// 転送先が 1 つも無いか。
    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// この場に居る相手の一覧（表示名）。手順の説明で名簿として出す。
    fn roster(&self) -> Vec<&str> {
        self.entries
            .iter()
            .map(|(_, _, display)| display.as_str())
            .collect()
    }

    /// 委譲ツールの名前。転送ツールの `transfer_to_` を `ask_` に replace した形。
    fn ask_name(transfer_name: &str) -> String {
        transfer_name.replacen("transfer_to_", "ask_", 1)
    }

    /// 委譲（`ask_*`）のツール定義。
    ///
    /// 転送との違いは**答えの行き先**だけ。転送は制御ごと渡してユーザーへ返るが、
    /// 委譲は答えが自分に戻ってきて、自分の話を続けられる。
    fn ask_specs(&self) -> Vec<ToolSpec> {
        self.entries
            .iter()
            .map(|(name, _, display)| ToolSpec {
                name: Self::ask_name(name),
                description: format!(
                    "**{display}** に質問し、**その答えを受け取る**。\
                     答えは自分に戻ってくるので、それを踏まえて話を続けられる。\
                     相手に話を引き継いで自分は退く場合は、これではなく \
                     `transfer_to_*` を使うこと。"
                ),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "message": {
                            "type": "string",
                            "description": "相手に尋ねる内容"
                        }
                    },
                    "required": ["message"],
                    "additionalProperties": false
                }),
            })
            .collect()
    }

    /// 委譲ツール名から相手を逆引きする。
    fn resolve_ask(&self, name: &str) -> Option<&AgentId> {
        self.entries
            .iter()
            .find(|(tool, _, _)| Self::ask_name(tool) == name)
            .map(|(_, target, _)| target)
    }

    /// wire へ載せるツール定義。
    fn specs(&self) -> Vec<ToolSpec> {
        self.entries
            .iter()
            .map(|(name, _, display)| ToolSpec {
                name: name.clone(),
                description: format!(
                    "**{display}** へメッセージを渡して、会話を続ける。\
                     相手は自分で考えて返事をするので、返事を代筆しないこと。\
                     自分の応答で用が足りるなら、このツールを呼ばずに本文だけを返すこと。"
                ),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "message": {
                            "type": "string",
                            "description": "相手に伝える内容"
                        }
                    },
                    "required": ["message"],
                    "additionalProperties": false
                }),
            })
            .collect()
    }

    /// 手順の説明。
    ///
    /// OpenAI Agents SDK が `RECOMMENDED_PROMPT_PREFIX` で同種の説明を
    /// プロンプトへ足すのと同じ意図。ツールを渡すだけでは、
    /// 「呼ばない」という選択が終了を意味することがモデルに伝わらない。
    fn protocol_note(&self, tools_available: bool) -> String {
        if tools_available {
            format!(
                "## この場に居る相手\n\
                 {}\n\
                 いずれも**自分で考えて発言する別のエージェント**です。あなたが\
                 彼らの発言を書くことはありません。\n\n\
                 ## 会話の進め方\n\
                 まず、届いた発話の送り手を見てください。**あなたに話しかけてきた相手へ、\
                 あなた自身の言葉で答えるのが基本です。**\n\
                 他のエージェントの助けが要るときだけ `transfer_to_*` ツールを呼んでください。\
                 **複数の相手へ渡すときは、それぞれの `transfer_to_*` を同じ応答の中で同時に呼んでください**。\
                 全員へ並行して届きます。\n\
                 自分の応答で用が足りる場合、または相手の発言に返すべきことが残っていない場合は、\
                 **ツールを呼ばずに本文だけを返してください**。その時点で会話は終わり、\
                 結果が人間へ返ります。同じ内容を繰り返すくらいなら、会話を終えてください。\n\
                 転送・委譲が失敗したときは、その**理由**（相手が停止中・時間切れ・\
                 答えずに会話を渡した など）が結果の文字列で返ります。\
                 事前の点呼は不要です。",
                self.roster()
                    .iter()
                    .map(|name| format!("- {name}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        } else {
            format!(
                "## 会話の進め方\n\
                 応答は次のエージェントへ渡されます。会話を終えてよいと判断したら、\
                 本文の末尾に {TERMINATION_MARKER} と書いてください。その時点で会話は終わり、\
                 結果が人間へ返ります。同じ内容を繰り返すくらいなら、会話を終えてください。"
            )
        }
    }

    /// 並列委譲ツールの名前（Spec 04）。
    const PLAN: &'static str = "plan";

    /// `plan` を提示するか。
    ///
    /// **接続先 2 体以上のときだけ。** 「進行役フラグ」のような設定は足さない —
    /// トポロジーがそのまま「進行役かどうか」を決める。1 体しか繋がっていない
    /// エージェントには `ask_*` で足りるので、使えない選択肢のスキーマを
    /// 毎ターンの固定費として払わせない。
    fn offers_plan(&self) -> bool {
        self.entries.len() >= 2
    }

    /// 並列委譲（`plan`）のツール定義。
    ///
    /// `ask_*` との違いは**並列性と合流**だけ。`ask_*` は 1 体ずつ待つので
    /// 壁時計が人数倍になり、`transfer_to_*` の fan-out は並列だが答えが
    /// ユーザーへ散って戻ってこない。その中間が無かった。
    ///
    /// **宛先は `enum` で閉じる。** 自由文字列にすると、`build()` が
    /// 「ツール名は ID、説明は表示名」で解いた問題を作り直すことになる。
    /// 表示名の一意性はどこも保証していない（`World::register_agent` が
    /// 拒否するのは ID の重複だけ）ので、名前で指させると同名の 2 体を
    /// 区別できない。
    fn plan_specs(&self) -> Vec<ToolSpec> {
        if !self.offers_plan() {
            return Vec::new();
        }

        let ids: Vec<&str> = self
            .entries
            .iter()
            .map(|(_, target, _)| target.as_str())
            .collect();
        // ID と表示名の対応表。会話は表示名で流れるので、これが無いと
        // モデルは「ザリ・ロブステル」と `agent_2` を結び付けられない。
        let roster = self
            .entries
            .iter()
            .map(|(_, target, display)| format!("{target} = {display}"))
            .collect::<Vec<_>>()
            .join(" / ");

        vec![ToolSpec {
            name: Self::PLAN.to_owned(),
            description: format!(
                "複数の相手へ**並列に**頼んで、全員の答えを束ねて受け取る。\
                 相手ごとに依頼内容を変えられる。\
                 1 体ずつ順に尋ねる `ask_*` と違い、全員が同時に動くので速い。\
                 独立した調べもの・作業を配るときはこれを使うこと。\
                 次の波を出す前に、前の束ねは**自分の言葉で要約**してから頼むこと\
                 （束ね全文を引きずると入力が波のたびに膨らむ）。\
                 「会話を渡した」と返ったタスクは**リトライしないこと** — \
                 仕事は別の経路で続いており、頼み直すと同じ仕事が二重に走る。\
                 依頼先: {roster}"
            ),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "tasks": {
                        "type": "array",
                        "description": "同時に頼む依頼の一覧。同じ相手を 2 回入れないこと",
                        "items": {
                            "type": "object",
                            "properties": {
                                "to": {
                                    "type": "string",
                                    "enum": ids,
                                    "description": format!("依頼先。{roster}"),
                                },
                                "message": {
                                    "type": "string",
                                    "description": "その相手への依頼内容"
                                }
                            },
                            "required": ["to", "message"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["tasks"],
                "additionalProperties": false
            }),
        }]
    }

    /// この波の宛先として妥当か（実行時のトポロジーで見る）。
    ///
    /// 提示はターンの開始時、検証は実行時。`set_connections` は稼働中に
    /// 呼べるので、この 2 点の間に繋ぎ替えが起こりうる。
    fn is_target(&self, id: &AgentId) -> bool {
        self.entries.iter().any(|(_, target, _)| target == id)
    }

    /// 宛先の表示名。束ねの見出しに使う。
    fn display_of(&self, id: &AgentId) -> Option<&str> {
        self.entries
            .iter()
            .find(|(_, target, _)| target == id)
            .map(|(_, _, display)| display.as_str())
    }

    /// 名前からツールを逆引きする。
    fn resolve(&self, name: &str) -> Option<&AgentId> {
        self.entries
            .iter()
            .find(|(tool, _, _)| tool == name)
            .map(|(_, target, _)| target)
    }

    /// 最初の転送先。ツールを使えない経路の退避先。
    fn first(&self) -> Option<&AgentId> {
        self.entries.first().map(|(_, target, _)| target)
    }

    /// 応答から行き先を決める。
    ///
    /// 規則は OpenAI Agents SDK と同じ:
    /// **ツール呼び出しの無いテキスト出力が最終出力**。
    ///
    /// 転送要求は**全部**拾う（fan-out）。Claude / Gemini は 1 応答で複数の
    /// tool call を普通に返すので、最初の 1 本で打ち切ると残りが黙って消える。
    /// 同じ宛先への重複は先勝ちで 1 通に畳む——モデルは同じツールを
    /// 同じ引数で 2 回呼ぶことがあり、素通しにすると受け手の履歴が汚れる。
    fn decide(&self, response: &ChatResponse, tools_available: bool) -> Outcome {
        let text = response.text.clone().unwrap_or_default();

        if tools_available {
            let mut deliveries: Vec<(AgentId, String)> = Vec::new();
            for call in &response.tool_calls {
                let Some(target) = self.resolve(&call.name) else {
                    continue;
                };
                if deliveries.iter().any(|(to, _)| to == target) {
                    continue;
                }
                // 引数が欠けていても転送自体は成立させる。本文を代わりに渡す。
                let message = call
                    .args
                    .get("message")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
                    .unwrap_or_else(|| text.clone());
                deliveries.push((target.clone(), message));
            }
            if !deliveries.is_empty() {
                return Outcome::Handoff { deliveries };
            }
            return Outcome::Finish { content: text };
        }

        // ツールを使えない経路: 終了マーカーが無ければ最初の相手へ渡す。
        // 宛先を選ぶ手段が本文しか無いこの経路では、fan-out は表現できない。
        match (self.first(), text.contains(TERMINATION_MARKER)) {
            (Some(target), false) => Outcome::Handoff {
                deliveries: vec![(target.clone(), text)],
            },
            _ => Outcome::Finish {
                content: text.replace(TERMINATION_MARKER, "").trim_end().to_owned(),
            },
        }
    }
}
