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

mod bootstrap;
mod runtime;
mod settings;
mod lifecycle;
mod turn;
use turn::{connect_agent_mcp, handle_message, record_failed_turn};
mod delegation;
use delegation::{HandoffTools, Outcome, ask_agent, deliver_and_wait, run_plan};
mod context;
use context::{compose_presence_notices, compose_room_log, read_room_log, room_log_tool_spec};
mod schedules;
use schedules::spawn_schedule_ticker;
mod sessions;

use std::collections::HashMap;
use std::collections::BTreeMap;
use std::sync::{Arc, Weak};
use std::time::Duration;

use tokio::sync::{Mutex, RwLock, broadcast, mpsc, watch};
use tokio::task::JoinHandle;

use crate::budget::BudgetPool;
use crate::compute;
use crate::command::{ApprovalOutcome, CommandPolicyView};
use crate::config_store::ConfigStore;
use crate::session_store::{ForkPoint, Record as SessionRecord, SessionStore, SessionSummary};
// 診断の 1 行はここを通す（stderr とログファイルの両方へ出る）。
use crate::note;
use crate::error::{CoreError, CoreResult, ErrorPayload};
use crate::event::CoreEvent;
use crate::llm::{
    BackendFactory, ChatMessage, ChatRequest, ChatResponse, LlmBackend, Role, ToolSpec,
};
use crate::model::{
    AgentId, AgentMessage, AgentRole, AgentRoleId, AgentSnapshot, AgentSpec, AgentStatus,
    ConfigFileKind, CredentialSource, Endpoint, ModelTemplate, ModelTemplateId, TopologyEdge,
    WorkDirListing,
};
use crate::plan::{PlanTaskAnnounced, PlanTaskState, PlanWaveRecord, PlanWaveStore};
use crate::schedule::{Recurrence, ScheduleOptions, ScheduledTask, Tick};
use crate::schedule_probe::{
    Judgement, ProbeError, ProbeOutcome, ScheduleProbe, SessionMode, compose_body, judge,
};
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
    // 委譲の待ち時間（旧 `ask_timeout`）は Spec 44 で `World::ask_timeout()` へ
    // 移した — 既定 600 秒はあちらの 1 箇所に住み、二重定義を作らない。
    // 輪の解放も時計の仕事ではなくなった（`Envelope.waiting` の構造検出）。
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
    /// 依頼の因果が共有するトークン予算（Spec 11）。**`cancel` と独立** —
    /// cancel を持たない配送（転送・予定発火）でも予算は運ぶ。
    ///
    /// 根（ユーザー発話の宛先ごと / 予定の発火ごと）で生まれ、因果内の
    /// 全配送が**同一の Arc** を指す。転送先・波の配送で新しいプールを
    /// 作ってはならない（天井が蒸発する — delegation-fanout race、
    /// `token_budget` 契約の pool）。`None` = 天井なしの村。
    budget: Option<Arc<BudgetPool>>,
    /// この因果で**待って答えを返し終えた**個体（Spec 28 の `summarizeAfter`）。
    ///
    /// 予算と同じ経路を運ばれるが、**予算とは別の Arc**。理由は
    /// **書き込む場所が違う**こと — 予算はどの配送でも消費されるのに対し、
    /// ここへ入るのは [`deliver_and_wait`] が**答えを受け取った**ときだけ。
    /// `handoff` は待たないので入らない（渡した先が終わったかを観測する点が
    /// 無いため、要約の対象にしない）。
    ///
    /// **予算プールに相乗りさせない。** `handoff` も同一の予算 Arc を継承する
    /// ので（`a_handoff_inherits_the_same_budget_pool` が凍結）、予算の伝播で
    /// 代用すると `handoff` 先まで対象に入る。
    ///
    /// `None` = 参加者を数える必要のない因果（利用者の発話・要約しない予定）。
    participants: Option<Participants>,
    /// この因果で**答えを待ってブロック中**の個体の連鎖（Spec 44 — 輪の検出）。
    ///
    /// **順序つき・末尾が直近の依頼主。** `ask` / `plan` の配送は自分を末尾に
    /// 追加し、転送は末尾を除き（`HandedOff` が配送より前にその待ちを解くため。
    /// 空なら空のまま）、新しい因果の根（利用者発話 / 予定の発火 /
    /// Spec 43 の dispatch / 束ねの配送）は空。
    ///
    /// **判定は [`delegation::deliver_and_wait`] の入口 1 箇所だけ** — 輪 =
    /// 待ちの循環はそこでしか生まれない。転送の配送では判定しない（転送は
    /// 待たない。判定を足すと、ブロック中の個体への健全な転送 = 受信箱で
    /// 順番を待つだけの形を壊す）。詳細は `ask_cycle_contract`。
    waiting: Vec<AgentId>,
}

/// 因果に参加して答えを返し終えた個体の集合（Spec 28）。
///
/// `std::sync::Mutex` なのは、入れるのが `await` を跨がない 1 行だから。
type Participants = Arc<std::sync::Mutex<std::collections::HashSet<AgentId>>>;

/// 送信時に受け取る添付画像の生データ（Spec 23）。
///
/// UI 層が WebP へ変換した後のバイト列で、検証と保存はコア側
/// （[`crate::attachment::AttachmentStore::save`]）がもう一度行う。
/// IPC から届くバイト列を検証なしでディスクへ書かないため。
#[derive(Debug, Clone)]
pub struct AttachmentUpload {
    /// 元ファイル名（表示用。パス解決には使わない）。
    pub file_name: String,
    /// WebP のバイト列。
    pub bytes: Vec<u8>,
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
    /// どれも「依頼元ターン」を持たないので、キャンセルの手掛かりは持たないが、
    /// **予算は持つ**（根で生まれたか、転送元のターンから引き継いだもの）。
    fn plain(
        incoming: AgentMessage,
        budget: Option<Arc<BudgetPool>>,
        participants: Option<Participants>,
        waiting: Vec<AgentId>,
    ) -> Self {
        Self {
            incoming,
            reply_to: None,
            cancel: None,
            budget,
            participants,
            waiting,
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
    /// 添付画像の置き場（Spec 23）。`{workspace}/attachments/`。
    attachments: crate::attachment::AttachmentStore,
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
    /// 前判定の実行が端末で承認されているかを答える口（Spec 28）。
    ///
    /// **未設定なら 1 件も承認されていない扱い**（fail closed）。承認の実体は
    /// GUI 層が `{app_data_dir}` に持つので、コア単体（テスト・ヘッドレス）では
    /// 前判定つきの予定は走らない。**それが安全側**で、
    /// 「注入し忘れたら全部走る」の逆を選んである。
    probe_approvals: RwLock<Option<Arc<dyn ProbeApprovals>>>,
    /// この村の識別子（`{workspace}/village_id`）。承認鍵に混ざる。
    ///
    /// 起動時に 1 度だけ解決して持つ — **発火のたびにファイルを読むと、
    /// 途中で差し替えられた識別子で承認が通る窓**ができる。
    village_id: RwLock<String>,
    /// plan 実行の観測記録（Spec 08 — 波ペイン）。リングバッファでプロセス寿命。
    /// ファイルへは書かない — 再起動生存は別 Spec の管轄。
    plan_waves: RwLock<PlanWaveStore>,
    /// 承認済みの波の実行者（Spec 43 — **ターンの外の実行形**。凍結 8）。
    /// key = plan_id・値は (進行役, root cancel token)。
    ///
    /// `interrupt_turn`（進行役一致）と `interrupt_all` がここを切る —
    /// 波の実行者はターンではないので `turns` の網に掛からない。
    /// 完了時に実行者自身が自分を外す。
    wave_runs: Mutex<HashMap<u64, (AgentId, tokio_util::sync::CancellationToken)>>,
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
    /// 会話の保存先（Spec 12）。開けなかった場合だけ `None`。
    ///
    /// **`None` でも村は普通に動く** — 保存できないことは会話を止める理由に
    /// ならない（起動が止まる経路は作らない、D1 と同じ規律）。その代わり
    /// 起動時に WARN を 1 行出す。
    sessions: Option<SessionStore>,
    /// エージェント別の**現役の**要約（Spec 12 P4）。手動要約のときだけ入る。
    ///
    /// **履歴（`AgentRecord.history`）の中には置かない。** 置くと、送った文字列と
    /// 保存した文字列が食い違う（#45）。ここに持って**可変文脈の畳みへ相乗り**
    /// させれば、できた `sent_user_turn` がそのまま `exchange` として保存される。
    /// 復元時は `restore_histories` が返す最新の要約で埋め直す。
    summaries: RwLock<BTreeMap<AgentId, String>>,
    /// いま開いているセッションの ID（Spec 12）。
    ///
    /// **`std::sync::RwLock` で持つ。** 書き込み点（`record` / `push_exchange`）は
    /// どちらも `world` の write guard を握った同期文脈にあり、そこから
    /// `await` する経路を作りたくない。中身は短い文字列で、ロックは
    /// clone するあいだしか握らない。
    session_id: std::sync::RwLock<String>,
    /// 外部からの依頼の同時実行を 1 本に絞るゲート（Spec 25 D7）。
    ///
    /// **外部入口は新しい因果の根**で、hop は 0 から始まり予算プールも新品に
    /// なる。ゆえに `max_hops` もトークンの天井も**扉を通る閉路を塞げない**
    /// （通るたびにリセットされる）。同時 1 本にすると、村が自分自身を MCP
    /// サーバーとして登録した閉路は 2 周目で必ず busy に当たって切れ、窓口が
    /// 自分のターンの完了を待つデッドロックも即座に解ける。併走による予算の
    /// 二重消費も同じ 1 つの機構で消える。
    external_gate: tokio::sync::Semaphore,
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
    /// **前判定が走行中の予定 ID**（Spec 28）。
    ///
    /// `timeoutSecs` の上限は 3600 秒あるので、5 分ごとの予定では probe が
    /// 次の予定時刻をまたぐ。走行中に来た発火は**消化せずスキップ**する
    /// （宛先が飛行中のときと同じ倒し方）。**これが無いとプロセスが積み上がり、
    /// 村ごと重くなる。**
    ///
    /// 粒度が予定 ID なのは、外す契機が probe の完了という**予定単位の事実**
    /// だから（`in_flight` が `AgentId` 単位なのは、外す契機に使える信号
    /// `AgentTyping` がエージェント単位でしか来ないため — 事情が逆）。
    probing: Mutex<std::collections::HashSet<String>>,
    /// 根のターンの完了を待っている要約（Spec 28 の `summarizeAfter`）。
    ///
    /// キーは**発火の宛先**（因果の根）。`AgentTyping { active: false }` を
    /// 受けた時点で取り出し、集めた参加者ごと要約へ渡す。
    ///
    /// **`in_flight` と同じ合図に相乗りしている。** 別の完了検知を作ると、
    /// 片方だけが取りこぼす形が生まれる（イベントの取りこぼしで `in_flight` が
    /// fail open で空にされるとき、こちらだけ残ると要約が永遠に待つ）。
    /// ゆえに**取りこぼしでは一緒に捨てる** — 要約は次の発火でやり直せる。
    pending_summaries: Mutex<HashMap<AgentId, Participants>>,
    /// 根のターンの完了を待っている後判定（Spec 46）。
    ///
    /// キーは**発火の宛先**（因果の根）。`pending_summaries` と**同じ合図に
    /// 相乗り**するが、**受け手は排他** — 後判定つきの予定は配送時にこちらへ
    /// だけ登録し、要約は後判定ループが確定してから走らせる（D2 の直列。
    /// 両方へ登録すると試行ごとに要約が発火して課金が試行回数ぶん増える）。
    ///
    /// 取りこぼし（Lagged）では `pending_summaries` と一緒に捨てる —
    /// 完了の合図が来ない以上、待ち続けても起点は二度と来ない。
    /// 検収は次の発火がやり直す（毒タスクの残余として契約に明記済み）。
    pending_acceptances: Mutex<HashMap<AgentId, schedules::AcceptancePending>>,
    /// 予定ごとの**直近 1 回**の検収の結末（Spec 46）。プロセス寿命。
    ///
    /// `last_probe` と同じ器・同じ理由 — 不一致・失敗は会話ログへ流さないが
    /// 沈黙にもしない。再起動後の診断は `fuseforks.log` の `acceptance:` 行。
    last_acceptance: Mutex<HashMap<String, crate::schedule_probe::ProbeReport>>,
    /// 予定ごとの**直近 1 回**の判定の結末（Spec 28 D8）。
    ///
    /// **画面のためだけに持つ。** 不一致・失敗は会話ログへ流さない（5 分ごとの
    /// 監視で毎回 System 行が出ると本物の通知が埋まる）が、**沈黙にもしない** —
    /// 特に `error` / `timeout` は人が直せるので、どこかに出す価値がある。
    ///
    /// プロセス寿命（波の記録と同じ）。**再起動後は `fuseforks.log` の
    /// `schedule probe:` 行が診断を担う**ので、第 2 の永続ファイルは作らない。
    last_probe: Mutex<HashMap<String, crate::schedule_probe::ProbeReport>>,
}

/// 前判定コマンドの実行が端末で承認されているかを答える。
///
/// **実装は GUI 層に置く**（`{app_data_dir}/probe_approvals.json`）。コアは
/// workspace しか知らないので、**承認を workspace の中に置くと承認ごと配布され、
/// 防御にならない**（攻撃者が書けるファイルに承認を書く形になる）。
///
/// **読むだけの口。** 書き込みは `tauri::command` の層に閉じてあり、
/// ここから承認を足す経路は無い — モデルが `schedules.json` を `file write` で
/// 書いても承認は付かない（Spec 28 D10）。
pub trait ProbeApprovals: Send + Sync {
    /// この鍵の前判定を、この端末で実行してよいか。
    fn is_approved(&self, key: &str) -> bool;
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

    /// ログへ追記し、保存先へ書き、[`CoreEvent::MessageSent`] を発行する。
    ///
    /// `Shared.log` の [`OrchestratorConfig::log_capacity`] は**メモリ上の上限で
    /// あって保存の上限ではない**（Spec 12）。リングから落ちた発話もファイルには
    /// 残り、起動時に末尾 `log_capacity` 件だけを読み戻す。
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
        self.persist(&SessionRecord::message(message.clone()));
        self.emit(CoreEvent::MessageSent { message });
    }

    /// いま開いているセッションの ID。
    fn current_session(&self) -> String {
        match self.session_id.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// 開いているセッションを差し替える。以後の書き込みは新しい側へ着地する。
    fn set_session(&self, session_id: &str) {
        match self.session_id.write() {
            Ok(mut guard) => *guard = session_id.to_owned(),
            Err(poisoned) => *poisoned.into_inner() = session_id.to_owned(),
        }
    }

    async fn summarize_agents(&self, only: Option<&std::collections::HashSet<AgentId>>) -> CoreResult<usize> {
        let store = self.session_store()?;
        let session_id = self.current_session();

        // 覆う境界は**書く前に**取る。書いた後だと自分自身を覆う要約になり、
        // 「coversUpToSeq < 自身の seq」が破れる。
        let Some(covers_up_to_seq) = store.last_seq(&session_id)? else {
            return Ok(0);
        };

        // 材料を先に集める。LLM 呼び出しの間、world のロックは持たない。
        //
        // **対象は稼働中のサーヴァントだけ。** 要約の目的は「以後のプロンプトを
        // 短くする」ことで、停止中の個体には以後のターンが無い。それでも呼べば、
        // **参加していない個体のぶんまで押した人がトークンを払う**ことになる。
        // 履歴は停止しても消えない（Spec 12 P2）ので、起動して会話へ加わってから
        // 押せばそのとき要約される — 取り逃がしにはならない。
        let mut skipped = 0usize;
        let targets: Vec<(AgentId, String, ModelTemplate, Vec<ChatMessage>)> = {
            let world = self.world.read().await;
            world
                .snapshots()
                .into_iter()
                .filter_map(|snapshot| {
                    // **対象の指定があればそれだけ**（Spec 28 D7 — 予定の因果に
                    // 参加して答えを返し終えた個体）。指定が無ければ稼働中の全員
                    // （人が「要約して続ける」を押した従来の経路）。
                    if only.is_some_and(|ids| !ids.contains(&snapshot.id)) {
                        return None;
                    }
                    let record = world.agent(&snapshot.id).ok()?;
                    if record.history.is_empty() {
                        return None;
                    }
                    if !record.status.is_active() {
                        skipped += 1;
                        return None;
                    }
                    let template = world.template(&record.spec.model_template_id).ok()?.clone();
                    Some((
                        snapshot.id.clone(),
                        record.spec.name.clone(),
                        template,
                        record.history.clone(),
                    ))
                })
                .collect()
        };

        let mut done = 0usize;
        for (agent_id, name, template, history) in targets {
            // 要約の要約を許す（`coversUpToSeq` が単調増加するので循環しない）。
            let previous = self.summaries.read().await.get(&agent_id).cloned();

            // 畳む前の通数と文字数。ログに出す — 「4 往復を要約に置き換えた」のか
            // 「40 往復ぶんか」で、要約の効きの読み方がまるで違う。
            //
            // **文字数のほうが本体**（2026-08-03 の実機で判明）。要約が畳んだ履歴より
            // 大きくなることが実際に起きる（opus が 1 往復を 1,163 字に書き広げた）。
            // しかも要約は滑る窓と違って落ちないので、**膨らんだぶんは以後の全ターンに
            // 乗り続ける**。通数だけでは、その損得が 1 行から読めない。
            let history_msgs = history.len();
            let history_chars: usize = history.iter().map(|m| m.content.chars().count()).sum();
            let mut messages = Vec::with_capacity(history_msgs + 3);
            messages.push(ChatMessage::system(SUMMARY_SYSTEM));
            if let Some(previous) = &previous {
                messages.push(ChatMessage::user(format!(
                    "## 前回までの経緯（既存の要約）\n{previous}"
                )));
                messages.push(ChatMessage::assistant("承知しました。"));
            }
            messages.extend(history);
            messages.push(ChatMessage::user(SUMMARY_INSTRUCTION));

            let backend = match self.backend_for(&template).await {
                Ok(backend) => backend,
                Err(err) => {
                    note!("WARN summarize: {agent_id}（{name}）のバックエンドを組めません: {err}");
                    continue;
                }
            };
            let request = ChatRequest {
                model: template.model.clone(),
                messages,
                // 要約にツールは要らない。提示すると呼びに行く個体が出て、
                // 1 回で終わるはずの呼び出しがループになる。
                tools: Vec::new(),
                tool_choice: crate::llm::ToolChoice::None,
                temperature: template.temperature,
                max_tokens: template.max_output_tokens,
                effort: template.effort,
                cacheable_prefix_len: 0,
            };
            let response = match backend.chat(request).await {
                Ok(response) => response,
                Err(err) => {
                    note!("WARN summarize: {agent_id}（{name}）の要約に失敗しました: {err}");
                    continue;
                }
            };

            // **使ったトークンは必ず数える。** 要約はターンループの外で走る
            // LLM 呼び出しなので、ここで積まないと**押した人が払った分がどの
            // 数字にも現れない** — カードの累計にも、村の集計にも、ログにも。
            // 失敗した呼び出しぶんも課金されるが、それは response が無いので
            // 数えられない（プロバイダが usage を返さない経路と同じ扱い）。
            let usage = response.usage;
            if let Ok(record) = self.world.write().await.agent_mut(&agent_id) {
                record.total_tokens += usage.total();
                record.prompt_tokens += usage.prompt;
                record.cached_tokens += usage.cache_read;
            }
            let text = response.text.unwrap_or_default();
            // ターンの `turn:` 行と対になる 1 行。要約は会話ログに 1 行しか
            // 残らないので、**何にいくら掛かったか**はここにしか出ない。
            let summary_chars = text.chars().count();
            note!(
                "summarize: agent={agent_id} model={} covers_up_to={covers_up_to_seq} \
                 folded_msgs={history_msgs} folded_chars={history_chars} \
                 summary_chars={summary_chars} prompt={} cached={} total={}",
                template.model,
                usage.prompt,
                usage.cache_read,
                usage.total()
            );
            // 短くならなかった要約は、以後の全ターンで払い続ける固定費になる
            // （履歴は滑る窓で落ちるが、要約は落ちない）。**機構では止めない** —
            // 止めると「要約したのに畳まれない」状態が画面から読めなくなる。
            // 計器だけ置いて、頻度を見てから決める。
            if summary_chars >= history_chars {
                note!(
                    "WARN summarize: {agent_id}（{name}）の要約が元の履歴より長くなりました\
                     （{history_chars} 字 → {summary_chars} 字）。この会話ではプロンプトは\
                     短くなりません — 畳む履歴が増えてから押すほうが効きます"
                );
            }

            if text.trim().is_empty() {
                note!("WARN summarize: {agent_id}（{name}）の要約が空でした（履歴は畳みません）");
                continue;
            }

            self.persist(&SessionRecord::summary(
                &agent_id,
                text.clone(),
                covers_up_to_seq,
            ));
            self.summaries
                .write()
                .await
                .insert(agent_id.clone(), text);
            // 畳むのは**保存が済んでから**。先に畳むと、保存に失敗した瞬間に
            // 履歴も要約も無い状態になる。
            if let Ok(record) = self.world.write().await.agent_mut(&agent_id) {
                record.history.clear();
            }
            done += 1;
        }

        if done > 0 || skipped > 0 {
            // 飛ばした相手が居るなら**必ず言う**。黙って対象外にすると、
            // 「要約したのに次のターンが短くならない」個体の理由が画面から消える。
            // 次の道も書く（#44 の規律）— 起動してから押せば要約される。
            // System 行は記録時の言語で書く（Spec 35 D6）。
            let language = self
                .world
                .read()
                .await
                .language()
                .unwrap_or(crate::world::Language::Ja);
            let note = match (skipped > 0, language) {
                (true, crate::world::Language::Ja) => format!(
                    "（停止中の {skipped} 体は要約していません。起動してからもう一度押すと、\
                     その相手も要約されます）"
                ),
                (true, crate::world::Language::En) => format!(
                    " ({skipped} stopped agent(s) were not summarized. Start them and \
                     press again to include them.)"
                ),
                (false, _) => String::new(),
            };
            // **文言を経路で分ける。** 予定の完了後に走った要約を「押しました」の
            // 文面で出すと、押していない人が自分の操作だと読む。
            let headline = match (only.is_some(), language) {
                (true, crate::world::Language::Ja) => {
                    format!("予定の完了後に {done} 体の記憶を要約しました。")
                }
                (false, crate::world::Language::Ja) => {
                    format!("稼働中の {done} 体の記憶を要約しました。")
                }
                (true, crate::world::Language::En) => {
                    format!("Summarized the memory of {done} agent(s) after the scheduled run finished.")
                }
                (false, crate::world::Language::En) => {
                    format!("Summarized the memory of {done} running agent(s).")
                }
            };
            let body = match language {
                crate::world::Language::Ja => format!(
                    "{headline}以後のやり取りは要約を踏まえて続きます\
                     （元のやり取りは消えていません — 「会話一覧」の書き出しから\
                     読めます）{note}"
                ),
                crate::world::Language::En => format!(
                    "{headline} Future turns continue from the summary. (Nothing is \
                     lost — the original exchanges can be read via the conversation \
                     list's export.){note}"
                ),
            };
            self.record(AgentMessage::new(Endpoint::System, Endpoint::User, body, 0))
                .await;
        }
        Ok(done)
    }


    /// 会話の保存先。開けていなければエラー。
    fn session_store(&self) -> CoreResult<&SessionStore> {
        self.sessions.as_ref().ok_or_else(|| CoreError::SessionStore {
            path: self.store.root().join("sessions.redb").display().to_string(),
            operation: "open",
            reason: "会話の保存先が開けていません".to_owned(),
        })
    }

    /// 会話を開き、投影（広場ログ・履歴・要約）を張り直す。
    ///
    /// **`Orchestrator::switch_to` の本体。** 予定の発火（Spec 28 の
    /// `sessionMode: fresh`）は `Orchestrator` を持たずに切り替える必要があるので、
    /// 規律を 2 箇所に書かないためにここへ置いてある。
    ///
    /// # Errors
    /// 保存先が開けていない、または読み込みに失敗した場合。
    async fn open_session(&self, session_id: &str) -> CoreResult<()> {
        let store = self.session_store()?;

        self.log.write().await.clear();
        self.world.write().await.clear_histories();
        // ここで差し替える。以後の書き込みは新しい側へ着地する。
        self.set_session(session_id);

        let messages = store.tail_messages(session_id, self.config.log_capacity)?;
        let restored = store.restore_histories(session_id, self.config.history_turns)?;
        {
            let mut world = self.world.write().await;
            for (agent_id, history) in restored.histories {
                if let Ok(record) = world.agent_mut(&agent_id) {
                    record.history = history;
                }
            }
        }
        // 要約も差し替える（Spec 12 P4）。前の会話の要約が残ると、開いた会話とは
        // 無関係な「これまでの経緯」が次のターンへ混ざる。
        *self.summaries.write().await = restored.summaries;
        *self.log.write().await = messages;

        self.emit(CoreEvent::ConversationCleared);
        self.emit(CoreEvent::SessionSwitched {
            session_id: session_id.to_owned(),
        });
        Ok(())
    }

    /// レコードを 1 件保存する。**失敗しても村は止めない。**
    ///
    /// 保存できないこと（ディスク満杯・権限・ファイル破損）は、会話を続けない
    /// 理由にならない。代わりに WARN を 1 行出す — 黙って落とすと、再開したとき
    /// 初めて欠落に気づくことになる。
    ///
    /// redb の書き込みは同期で 1 本に直列化されるが、1 件の追記は短い
    /// （実測 40,000 件で 271 ms = 1 件あたり約 7 マイクロ秒）。**トランザクションを
    /// `await` を跨いで持たない**という契約は、この関数が同期で閉じることで守られる。
    ///
    /// 戻り値は**書けたか**。保存先を持たない村・書けなかった村では偽で、
    /// 呼び出し側は「保存されたことを前提にする後続」（Spec 39 の `TurnRecorded`）を
    /// 抑える。ここで `Err` にしないのは Spec 12 の規律（保存の失敗は WARN 1 行で続行）。
    fn persist(&self, record: &SessionRecord) -> bool {
        let Some(store) = self.sessions.as_ref() else {
            return false;
        };
        let session_id = self.current_session();
        if let Err(err) = store.append(&session_id, record) {
            note!("WARN session store: 会話 `{session_id}` へ保存できませんでした: {err}");
            return false;
        }
        true
    }

    /// 1 往復を履歴へ積み、**同じ内容を保存先へも書く**。
    ///
    /// 押し込む側と保存側を別々に書かない。分けると片方だけ更新される経路が生まれ、
    /// 再開後の履歴が実行中の履歴と食い違う — しかもその食い違いは画面に出ない。
    /// 呼び出し側は `world` の write guard を既に握っているので、guard を受け取る形にしてある。
    fn push_exchange(&self, world: &mut World, agent_id: &AgentId, sent: &str, replied: &str) {
        // 既に消えたエージェントの往復は積まないし、保存もしない。
        // 保存だけ残ると、復元時に宛先の無い exchange が出てくる。
        let Ok(record) = world.agent_mut(agent_id) else {
            return;
        };
        record.push_exchange(sent, replied, self.config.history_turns);
        self.persist(&SessionRecord::exchange(agent_id, sent, replied));
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
            // System 行は記録時の言語で書く（Spec 35 D6）。
            let language = self
                .world
                .read()
                .await
                .language()
                .unwrap_or(crate::world::Language::Ja);
            let text = match language {
                crate::world::Language::Ja => {
                    if is_running {
                        format!("{id}（{name}）が稼働を開始しました")
                    } else if status == AgentStatus::Failed {
                        format!("{id}（{name}）が失敗により停止しました")
                    } else {
                        format!("{id}（{name}）が停止しました")
                    }
                }
                crate::world::Language::En => {
                    if is_running {
                        format!("{id} ({name}) is now running")
                    } else if status == AgentStatus::Failed {
                        format!("{id} ({name}) stopped after a failure")
                    } else {
                        format!("{id} ({name}) stopped")
                    }
                }
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

/// 手動要約（Spec 12 P4）の役割指示。
///
/// **要約を作るのは本人**（そのエージェント自身のモデル）。別のモデルに
/// 要約させると、要約の文体と本人の文体が食い違い、次のターンで「自分が書いた
/// 覚え書き」として読めなくなる。
const SUMMARY_SYSTEM: &str = "あなたはこれから、自分自身の会話履歴を要約します。\
     要約は次のターン以降のあなた自身への覚え書きになります。";

/// 手動要約の最終指示。**要約だけを返させる**（前置きが混ざると、そのまま
/// 次のターンの文脈に載る）。
const SUMMARY_INSTRUCTION: &str = "ここまでのやり取りを、あなたが続きを進めるための覚え書きとしてまとめてください。\n\
     - 決まったこと・まだ決まっていないこと・次にやることを落とさない\n\
     - 固有名詞（ファイル名・ID・数値・URL）はそのまま残す\n\
     - 挨拶や相槌は落とす\n\
     - 箇条書きで、自分宛ての覚え書きとして書く\n\
     要約の本文だけを出力してください（前置きも後書きも要りません）。";


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

    /// 承認待ちの計画を、人が承認した最終形で配送する（Spec 43 — 編集窓の実行側）。
    ///
    /// `tasks` が配送の真実 — 提案との差分は取らない（D4。人が最後に見て押した
    /// 形が唯一の真実）。検証は run_plan と同じ 1 実装（凍結 3）を **dispatch
    /// 時点の接続**へ掛ける。配送は新しい因果の根（凍結 5 — 天井は村の
    /// `tokenBudget`、cancel は新しい root token で `interrupt_all` /
    /// 進行役への `interrupt_turn` が切る）。
    ///
    /// # Errors
    /// - [`CoreError::PlanWaveNotPending`] — 承認待ちの波ではない
    /// - [`CoreError::NotRunning`] — 進行役が停止中（D9。自動起動はしない）
    /// - [`CoreError::PlanDispatchInvalid`] — 検証に落ちた（空・非接続・重複）
    pub async fn dispatch_plan_wave(
        &self,
        plan_id: u64,
        tasks: Vec<crate::plan::PlanTaskInput>,
    ) -> CoreResult<()> {
        let Some((coordinator, wave)) = self.shared.plan_waves.read().await.proposal(plan_id)
        else {
            return Err(CoreError::PlanWaveNotPending { plan_id });
        };

        // D9 — 開始時点の門。束ねの届け先が居ないまま撒かない。
        if self
            .shared
            .mailboxes
            .read()
            .await
            .get(&coordinator)
            .is_none()
        {
            return Err(CoreError::NotRunning {
                agent_id: coordinator.to_string(),
            });
        }

        if tasks.is_empty() {
            return Err(CoreError::PlanDispatchInvalid {
                detail: "tasks が空です".to_owned(),
            });
        }

        // 検証と表示名の解決は dispatch 時点の world で行う（提示と dispatch の
        // 間に繋ぎ替えは起こりうる — run_plan の「検証は今」と同じ規律）。
        let (connected, displays) = {
            let world = self.shared.world.read().await;
            let record = world.agent(&coordinator)?;
            let connected = record.spec.connected_agents.clone();
            let displays: HashMap<AgentId, String> = tasks
                .iter()
                .map(|task| {
                    let display = world
                        .agent(&task.to)
                        .map(|r| r.spec.name.clone())
                        .unwrap_or_else(|_| task.to.to_string());
                    (task.to.clone(), display)
                })
                .collect();
            (connected, displays)
        };

        let mut accepted: Vec<(AgentId, String)> = Vec::with_capacity(tasks.len());
        for (index, task) in tasks.into_iter().enumerate() {
            let position = index + 1;
            match delegation::check_wave_target(
                &task.to,
                accepted.iter().map(|(existing, _)| existing),
                |t| connected.contains(t),
            ) {
                Some(delegation::WaveTaskDefect::NotConnected) => {
                    return Err(CoreError::PlanDispatchInvalid {
                        detail: format!(
                            "{position} 件目の宛先「{}」は進行役の接続先ではありません",
                            task.to
                        ),
                    });
                }
                Some(delegation::WaveTaskDefect::Duplicate) => {
                    return Err(CoreError::PlanDispatchInvalid {
                        detail: format!("宛先「{}」が同じ波に 2 回あります", task.to),
                    });
                }
                None => {}
            }
            accepted.push((task.to, task.message));
        }

        let announced: Vec<(AgentId, u32)> = accepted
            .iter()
            .map(|(to, message)| (to.clone(), message.chars().count() as u32))
            .collect();
        let started_at_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
        self.shared
            .plan_waves
            .write()
            .await
            .dispatch_wave(plan_id, &announced, started_at_ms);

        let to_list = accepted
            .iter()
            .map(|(target, _)| target.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let msg_chars: usize = accepted
            .iter()
            .map(|(_, message)| message.chars().count())
            .sum();
        // 実行の起点（人の操作）と配送の印を対で出す — dispatch: と wave: の
        // to= / msg_chars= が提示時（plan pending:）と違えば、編集がそのまま
        // 走った証拠になる（検収 2）。
        crate::note!(
            "plan dispatch: agent={coordinator} plan_id={plan_id} wave={wave} tasks={} to=[{to_list}] msg_chars={msg_chars}",
            accepted.len(),
        );
        crate::note!(
            "plan wave: agent={coordinator} wave={wave} tasks={} to=[{to_list}] msg_chars={msg_chars}",
            accepted.len(),
        );
        self.shared.emit(CoreEvent::PlanWaveStarted {
            plan_id,
            agent_id: coordinator.clone(),
            wave,
            tasks: announced
                .iter()
                .map(|(to, msg_chars)| crate::plan::PlanTaskAnnounced {
                    to: to.clone(),
                    msg_chars: *msg_chars,
                })
                .collect(),
            started_at_ms,
        });

        // 新しい因果の根（凍結 5）。天井の式は増やさない — ユーザー発話・
        // 予定の発火と同じ new_root_budget。
        let budget = new_root_budget(&self.shared).await;
        let cancel = tokio_util::sync::CancellationToken::new();
        self.shared
            .wave_runs
            .lock()
            .await
            .insert(plan_id, (coordinator.clone(), cancel.clone()));
        tokio::spawn(delegation::run_dispatched_wave(
            Arc::clone(&self.shared),
            coordinator,
            plan_id,
            wave,
            accepted,
            displays,
            budget,
            cancel,
        ));
        Ok(())
    }

    /// 承認待ちの計画を破棄する（Spec 43）。配送は一度も起きない。
    ///
    /// 破棄の事実は System 行として会話に残る（D3 — 進行役は次の依頼で
    /// 別の計画を立てられる）。
    ///
    /// # Errors
    /// [`CoreError::PlanWaveNotPending`] — 承認待ちの波ではない。
    pub async fn discard_plan_wave(&self, plan_id: u64) -> CoreResult<()> {
        let Some((coordinator, wave)) = self.shared.plan_waves.read().await.proposal(plan_id)
        else {
            return Err(CoreError::PlanWaveNotPending { plan_id });
        };
        self.shared.plan_waves.write().await.discard_wave(plan_id);
        crate::note!("plan discard: agent={coordinator} plan_id={plan_id} wave={wave}");
        self.shared.emit(CoreEvent::PlanWaveDiscarded { plan_id });

        let language = self
            .shared
            .world
            .read()
            .await
            .language()
            .unwrap_or(crate::world::Language::Ja);
        let notice = match language {
            crate::world::Language::Ja => format!("計画（波 {wave}）は破棄されました。"),
            crate::world::Language::En => format!("The plan (wave {wave}) was discarded."),
        };
        self.shared
            .record(AgentMessage::new(Endpoint::System, Endpoint::User, notice, 0))
            .await;
        Ok(())
    }

    /// エージェント別トークン消費量を集計する。
    ///
    /// 集計は Rayon プールで走るため、ログが数万件でも UI の IPC 応答は詰まらない。
    pub async fn token_usage_by_agent(&self) -> CoreResult<HashMap<AgentId, u64>> {
        let log = self.shared.log.read().await.clone();
        compute::spawn_rayon(move || compute::aggregate_token_usage(&log)).await
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



/// 因果の根の予算を作る（Spec 11）。村に天井が無ければ `None`。
///
/// 呼ぶのは根の 2 箇所（ユーザー発話の宛先ごと / 予定の発火ごと）だけ。
/// 因果の途中で呼ぶと予算が分裂して天井が蒸発する — 途中は必ず
/// 封筒・ターンからの引き継ぎで運ぶ。
async fn new_root_budget(shared: &Shared) -> Option<Arc<BudgetPool>> {
    shared
        .world
        .read()
        .await
        .token_budget()
        .map(|ceiling| Arc::new(BudgetPool::new(ceiling)))
}

/// 宛先の受信箱へ届ける。
///
/// `try_send` を使うのは背圧を可視化するため。`send().await` にすると、
/// 詰まった受信箱を待つあいだ送信側のエージェントまで停止して連鎖的に固まる。
async fn deliver(
    shared: &Shared,
    to: &AgentId,
    message: AgentMessage,
    budget: Option<Arc<BudgetPool>>,
    participants: Option<Participants>,
    waiting: Vec<AgentId>,
) -> CoreResult<()> {
    deliver_envelope(
        shared,
        to,
        Envelope::plain(message, budget, participants, waiting),
    )
    .await
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
    // 直前の 1 呼び出しの実効 milli（Spec 38 D1(b)）。**この個体の稼働と
    // 同じだけ生きる** — ターンをまたいで持ち越すのが目的なので、ターンの中
    // （`run_turn`）に置くと各ターンの初回ラウンドが永久に床のままになる。
    // 停止で消えるのは仕様（次の起動は床から。契約 reservation の初回床）。
    let last_call_milli = Arc::new(std::sync::atomic::AtomicU64::new(0));
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
                            Endpoint::External { client } => format!("external:{client}"),
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

                let outcome =
                    handle_message(&shared, &agent_id, envelope, &turn, &last_call_milli)
                        .await;

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

/// 外部クライアントの表示名を解決する（Spec 25）。
///
/// 人が設定した呼び名があればそれ、無ければ**呼び出し側の名乗り**へ落ちる。
///
/// **1 実装に閉じる。** 封筒（`attribute_sender`）と広場ログの名前解決
/// （`endpoint_label`）が同じ規則を通す必要があり、2 箇所で組むと
/// 「画面では設定した名前、プロンプトでは名乗り」という食い違いが生まれる
/// （Spec 19 の `attribute_sender` を 1 箇所に置いた理由と同じ）。
///
/// **設定するとモデルが読む名前が自己申告から人の決めた値へ変わる** —
/// `clientInfo.name` は攻撃者が書ける唯一の新しい経路（`mcp_server_contract`
/// 凍結 6）なので、設定はその経路を**閉じる**側に効く。
fn external_label<'a>(world: &'a World, client: &'a str) -> &'a str {
    world.external_name().unwrap_or(client)
}

/// 呼び名が未設定のときに封筒へ入る名前（`user_identity_contract` 凍結 2。
/// **Spec 35 D5 で部分改訂** — 言語で決まる: ja = これ / en = [`DEFAULT_USER_LABEL_EN`]）。
///
/// 旧凍結「言語に追従させない」の理由は「既存の村の履歴の途中で送り手名が
/// 変わる」だった。**言語は初回に確定して再判定しないので、新規の英語村には
/// この理由が当たらない** — 新規村は最初から `User` で一貫する。途中で
/// 切り替えた村は切り替え以後の封筒だけが変わり、それは System 行と同じ
/// 「記録時の言語」として許容する（settings_contract 層 3）。
pub const DEFAULT_USER_LABEL: &str = "ユーザー";

/// [`DEFAULT_USER_LABEL`] の英語（Spec 35 D5）。
pub const DEFAULT_USER_LABEL_EN: &str = "User";

/// 受信した発話へ送り手の封筒を付ける。
///
/// ユーザーの言葉もエージェントからの転送も同じ user ロールで届くため、名前を
/// 書かないと受信側は区別できない — 実際にユーザーの発話を「他のエージェントが
/// 話した言葉」と取り違えた。**プロンプトと履歴の両方へ同じ形で入れる**ので、
/// 組み立てはこの 1 箇所に置く（2 箇所で組むと、片方だけ直したときに
/// 過去のターンだけ出所不明に戻る）。
async fn attribute_sender(shared: &Arc<Shared>, incoming: &AgentMessage) -> String {
    // 封筒の語と既定ラベルは村の言語で決まる（Spec 35 D5）。組み立ての瞬間の
    // 言語 = 記録時の言語で、履歴に焼かれた後は変わらない（D6 と同じ扱い）。
    let language = shared
        .world
        .read()
        .await
        .language()
        .unwrap_or(crate::world::Language::Ja);
    let sender_label = match &incoming.from {
        // 利用者が呼び名を設定していればそれを使う（Spec 19）。
        // 未設定の既定は言語で決まる（凍結 2 の部分改訂。const の doc が正）。
        Endpoint::User => {
            let world = shared.world.read().await;
            world
                .user_name()
                .unwrap_or(language.pick(DEFAULT_USER_LABEL, DEFAULT_USER_LABEL_EN))
                .to_owned()
        }
        // 表示は UI と同じ「Fuseforks」。プロンプトと画面で同じ送り手が
        // 違う名前になると、利用者とエージェントの会話が噛み合わない。
        Endpoint::System => "Fuseforks".to_owned(),
        Endpoint::Agent { id } => {
            let world = shared.world.read().await;
            world
                .agent(id)
                .map(|record| record.spec.name.clone())
                // 送り手が既に削除されていても発話は成立させる。ID で示す。
                .unwrap_or_else(|_| id.to_string())
        }
        // 外部の MCP クライアント（Spec 25 D6）。**名前だけでは足りない** —
        // 呼び名は利用者も自由に付けられるので、`【送り手: Claude Code】` は
        // 「そういう呼び名の人間」と読める。相手が人間でないことが分かって
        // 初めて「噛み砕いた説明も聞き返しも要らない」という判断ができるので、
        // 種別を明示する。**乗るのは外部依頼のターンだけ**で、全員の毎ターンに
        // 乗る固定費ではない。
        Endpoint::External { client } => {
            let world = shared.world.read().await;
            match language {
                crate::world::Language::Ja => {
                    format!("{}（外部クライアント）", external_label(&world, client))
                }
                crate::world::Language::En => {
                    format!("{} (external client)", external_label(&world, client))
                }
            }
        }
    };
    // **本文が封筒を名乗れないようにしてから組み立てる**（Spec 26 D1）。
    // 素の本文を渡すと、本文が書いた封筒と本物が並ぶ — 2026-08-07 の実機で、
    // 受け取った個体が本物のほうを「自己申告」として切り捨てた。
    //
    // **無害化は全 `Endpoint` に掛ける**（D2）。外部だけに掛けると、
    // サーヴァントの発話が封筒構文を引用して広場ログ経由で別の個体へ届く
    // 経路が残る（実機で観測済み — ミュゼの回答が
    // `claude-code（外部クライアント）` を含んでザリの広場ログに乗った）。
    let (body, defused) = crate::sender_envelope::defuse(&incoming.content);
    if defused > 0 {
        // **`from` を出す**。origin 別に数えないと「通常は出ない」が判定に
        // ならない — 利用者が会話ログを貼り付ける経路は正当で、日常的に出る。
        crate::note!(
            "sender envelope escaped: from={} count={defused}",
            endpoint_kind(&incoming.from)
        );
    }
    crate::sender_envelope::wrap(&sender_label, &body, language)
}

/// 計器へ出す送り手の種別。**表示名ではなく種別**（誰が名乗ったかではなく、
/// どの経路から来たかを数えるため）。
///
/// 網羅 match にしてあるので、`Endpoint` に variant が増えたらここが落ちる。
fn endpoint_kind(from: &Endpoint) -> &'static str {
    match from {
        Endpoint::User => "user",
        Endpoint::System => "system",
        Endpoint::Agent { .. } => "agent",
        Endpoint::External { .. } => "external",
    }
}

/// このターンの受信に付いた添付参照を、ワイヤへ載せる形へ展開する
/// （Spec 23 = 画像 / Spec 36 = 音声・動画・PDF）。
///
/// 読むのは `incoming.attachments` だけ — 履歴の発話は文字列なので、
/// **添付がプロンプトへ載るのはこの 1 ターン限り**（D1）が構造で成立する。
/// 読めなかった参照は抜いて返し、数の差は呼び出し側が本文で断る。
async fn load_turn_attachments(
    shared: &Arc<Shared>,
    agent_id: &AgentId,
    incoming: &AgentMessage,
) -> Vec<crate::llm::PromptAttachment> {
    use base64::Engine as _;
    let mut loaded = Vec::with_capacity(incoming.attachments.len());
    for reference in &incoming.attachments {
        match shared.attachments.read(&reference.id).await {
            Ok(Some(bytes)) => loaded.push(crate::llm::PromptAttachment {
                // 形式は**保存時に検証で確定したもの**を参照から引く
                // （利用者のファイル名は見ない）。JPEG はワイヤ上の
                // フォールバック（Spec 23 D3）でしか現れないので、ここには出ない。
                media_type: reference.format.into(),
                data: base64::engine::general_purpose::STANDARD.encode(&bytes),
                // PDF の `input_file` / `file` part がファイル名を要求する。
                file_name: reference.file_name.clone(),
            }),
            Ok(None) => {
                note!(
                    "attachment missing: agent={agent_id} id={} file={}",
                    reference.id,
                    reference.file_name,
                );
            }
            Err(err) => {
                note!(
                    "attachment read failed: agent={agent_id} id={} err={err}",
                    reference.id,
                );
            }
        }
    }
    loaded
}

/// 添付の種別をモデルへ見せる語にする（Spec 36。**二言語** — Spec 35）。
///
/// [`AttachmentKind::as_str`] はログ用の安定した識別子で、こちらは
/// **モデルと利用者が読む語**。2 つを兼ねると、ログの grep 語が翻訳で動く。
pub(super) fn attachment_kind_label(
    kind: crate::attachment::AttachmentKind,
    language: crate::world::Language,
) -> &'static str {
    use crate::attachment::AttachmentKind as K;
    use crate::world::Language as L;
    match (kind, language) {
        (K::Image, L::Ja) => "画像",
        (K::Audio, L::Ja) => "音声",
        (K::Video, L::Ja) => "動画",
        (K::Pdf, L::Ja) => "PDF",
        (K::Image, L::En) => "image",
        (K::Audio, L::En) => "audio",
        (K::Video, L::En) => "video",
        (K::Pdf, L::En) => "PDF",
    }
}

/// 委譲・転送で添付が落ちることを、届く本文の先頭で断る（Spec 23 D6 /
/// Spec 36 で種別語へ一般化）。
///
/// `ask` / `plan` / `transfer_to_*` は**宛先が運べる種別でも**添付を渡さない
/// （Spec 36 D6 の分業 — `carries` を読むのは送信入口だけ）。根拠は D1 と同じ
/// 因果で、転送で渡すと 1 回の添付が N ターンの支払いに化け、「払う量と人の
/// 操作の 1 対 1 対応」が切れる。
///
/// 黙って落とすと、転送先がなぜ添付を見られないのか誰にも診断できない —
/// 「歯止めの先に道を書く」（#44）と Spec 12 P4 の「飛ばした相手が居たら
/// 必ず書く」の同型。**実機では診断だけでなく回復にも効いた**（送る側が
/// 断り書きを読んで、画像の内容をテキストへ書き起こして渡した）。
fn note_dropped_attachment(
    message: &str,
    dropped: Option<crate::attachment::AttachmentKind>,
    language: crate::world::Language,
) -> String {
    let Some(kind) = dropped else {
        return message.to_owned();
    };
    let label = attachment_kind_label(kind, language);
    match language {
        crate::world::Language::Ja => format!("（{label} 1 件は転送されません）\n\n{message}"),
        crate::world::Language::En => {
            format!("(the attached {label} is not forwarded)\n\n{message}")
        }
    }
}
