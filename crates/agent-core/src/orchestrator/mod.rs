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
    /// 委譲（`ask_*`）で相手の答えを待つ上限。
    ///
    /// 委譲は相手の応答を**待ってブロックする**。相互に委譲し合う配置では
    /// 待ち合わせが起きうるので、必ず戻る上限が要る。`max_hops` は深さしか
    /// 縛らず、待ちの時間は縛らない。
    pub ask_timeout: Duration,
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
    ) -> Self {
        Self {
            incoming,
            reply_to: None,
            cancel: None,
            budget,
            participants,
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
            let note = if skipped > 0 {
                format!(
                    "（停止中の {skipped} 体は要約していません。起動してからもう一度押すと、\
                     その相手も要約されます）"
                )
            } else {
                String::new()
            };
            // **文言を経路で分ける。** 予定の完了後に走った要約を「押しました」の
            // 文面で出すと、押していない人が自分の操作だと読む。
            let headline = if only.is_some() {
                format!("予定の完了後に {done} 体の記憶を要約しました。")
            } else {
                format!("稼働中の {done} 体の記憶を要約しました。")
            };
            self.record(AgentMessage::new(
                Endpoint::System,
                Endpoint::User,
                format!(
                    "{headline}以後のやり取りは要約を踏まえて続きます\
                     （元のやり取りは消えていません — 「会話一覧」の書き出しから\
                     読めます）{note}"
                ),
                0,
            ))
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
    fn persist(&self, record: &SessionRecord) {
        let Some(store) = self.sessions.as_ref() else {
            return;
        };
        let session_id = self.current_session();
        if let Err(err) = store.append(&session_id, record) {
            note!("WARN session store: 会話 `{session_id}` へ保存できませんでした: {err}");
        }
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

/// 起動時に開くセッションを決め、会話ログと履歴を戻す（Spec 12 P2）。
///
/// 既定は**最新セッション**（`updatedAt` で判定）。読めない・0 件なら警告を
/// 1 行出して新規セッションを作る — **起動が止まる経路は作らない**（D1）。
/// セッションを 1 つも用意できなかったときだけ `None` を返し、呼び出し側は
/// 保存なしで起動する。
///
/// 復元は 2 層を**別々に**戻す。会話ログ（`Shared.log`）は末尾 `log_capacity` 件、
/// 履歴（`AgentRecord.history`）は `exchange` から `history_turns` 往復。
/// **片方から他方は作れない** — 会話ログだけ戻すと画面は正しいのに全員が
/// 健忘症で始まり、その 2 つは画面上区別が付かない。
fn open_session_at_boot(
    sessions: &SessionStore,
    world: &mut World,
    log_capacity: usize,
    history_turns: usize,
) -> Option<(String, Vec<AgentMessage>, BTreeMap<AgentId, String>)> {
    let existing = match sessions.latest_session() {
        Ok(found) => found,
        Err(err) => {
            note!("WARN session store: 会話の一覧を読めませんでした（新しい会話で始めます）: {err}");
            None
        }
    };

    let session_id = match existing {
        Some(id) => id,
        None => match sessions.create_session(None) {
            Ok(id) => id,
            Err(err) => {
                note!(
                    "WARN session store: 会話を作れませんでした（この起動では会話は\
                     再起動で消えます）— {err}"
                );
                return None;
            }
        },
    };

    // 会話ログ: リングと同じ形（末尾 log_capacity 件）で画面へ戻す。
    let log = match sessions.tail_messages(&session_id, log_capacity) {
        Ok(messages) => messages,
        Err(err) => {
            note!("WARN session store: 会話ログを読めませんでした（画面は空で始まります）: {err}");
            Vec::new()
        }
    };

    // 履歴: ここが S1 の本丸。画面ではなく、次のターンで LLM へ渡る側。
    let mut restored_agents = 0usize;
    let mut orphaned = 0usize;
    let mut summaries = BTreeMap::new();
    match sessions.restore_histories(&session_id, history_turns) {
        Ok(restored) => {
            for (agent_id, history) in restored.histories {
                match world.agent_mut(&agent_id) {
                    Ok(record) => {
                        record.history = history;
                        restored_agents += 1;
                    }
                    // 会話の後で消されたエージェント。履歴の行き先が無いので捨てる。
                    Err(_) => orphaned += 1,
                }
            }
            // 要約は履歴とは別の口で戻る（Spec 12 P4）。可変文脈へ差す側の材料で、
            // 履歴の中には置かない。
            summaries = restored.summaries;
        }
        Err(err) => {
            note!(
                "WARN session store: 履歴を復元できませんでした（エージェントは前回の\
                 話を覚えていない状態で始まります）: {err}"
            );
        }
    }
    if orphaned > 0 {
        note!("session: 復元した履歴のうち {orphaned} 体分は、該当エージェントが居ないため捨てました");
    }
    note!(
        "session: {session_id} を開きました（発話 {} 件 / 履歴 {restored_agents} 体 / \
         要約 {} 体）",
        log.len(),
        summaries.len()
    );

    Some((session_id, log, summaries))
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
        // world.json の有無は load の**前**に見る（load は不在を空の世界として
        // 返すため、後からは新規と空を区別できない）。
        let fresh_world = !store.world_exists();
        let persisted = store.load_world().await?;
        let mut world = World::from_persisted(persisted.clone());

        // トークン予算の既定値は**新規の村にだけ**書く（Spec 11 の ceiling 契約。
        // 既存の村へ黙って天井を足すと、昨日まで完走していた依頼が今日から
        // 止まる — それはパッチでやってよい変更ではない）。下の正規化
        // 書き戻しが world.json への実書き込みを担う。
        if fresh_world {
            world.set_token_budget(Some(crate::budget::DEFAULT_CEILING));
        }

        // 言語が未確定（新規の村 / 追記前の村 / 手編集の不正値）なら OS から
        // 確定する（Spec 13 の settings_contract — 「自動」の選択肢は無く、
        // 初回に ja / en のどちらかへ確定して保存する）。下の正規化書き戻しが
        // world.json への実書き込みを担う。コアはこの値で分岐しない。
        if world.language().is_none() {
            world.set_language(crate::world::Language::from_os_locale(
                sys_locale::get_locale().as_deref(),
            ));
        }

        // 「unset なのに秘密が実在する」テンプレートは keyring へ昇格させる。
        // clear_credential は秘密の削除と unset への遷移を一体で行うので、
        // この組み合わせは正規の操作では作れない——過去の巻き戻り事故（failures.md #16）が
        // ディスクへ固定された状態である。放置するとユーザーはキーを貼り直すまで
        // 接続できず、しかもテンプレートの画面は「登録済み」と表示する（矛盾が見えない）。
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

        // 天井なしの村は起動のたびに WARN で可視化する（安全装置は opt-in でも、
        // 危険な状態を警告で見せれば実質 opt-out に近づく）。次の道を書く —
        // 警告だけ出して直し方を言わないのは #44 で払った代償の再演になる。
        if world.token_budget().is_none() {
            note!(
                "WARN token budget: この村に天井がありません — world.json の \
                 tokenBudget に 1000000（推奨）を設定すると、依頼 1 つあたりの\
                 トークン消費に自動の上限が掛かります"
            );
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

        // 会話の保存先を開き、最後に開いていた会話を戻す（Spec 12）。
        // **開けなくても起動は止めない** — 会話が戻らないことは、アプリが開かない
        // 理由にならない（D1 のフォールバックと同じ規律）。
        let (sessions, session_id, restored_log, restored_summaries) =
            match SessionStore::open(store.root().join("sessions.redb")) {
                Ok(sessions) => {
                    match open_session_at_boot(
                        &sessions,
                        &mut world,
                        config.log_capacity,
                        config.history_turns,
                    ) {
                        Some((id, log, summaries)) => (Some(sessions), id, log, summaries),
                        None => (None, String::new(), Vec::new(), BTreeMap::new()),
                    }
                }
                Err(err) => {
                    note!(
                        "WARN session store: 会話を保存できません（この起動では会話は\
                         再起動で消えます）— {err}"
                    );
                    (None, String::new(), Vec::new(), BTreeMap::new())
                }
            };

        let (events, _) = broadcast::channel(config.event_capacity);

        // 添付の置き場（Spec 23）。起動時に保持期間と容量の GC を掛ける（D9）。
        // 失敗しても起動は止めない — 消せなかった古いファイルは次の起動でまた
        // 候補になるだけで、会話の正しさには関わらない。
        let attachments = crate::attachment::AttachmentStore::new(store.root());
        match attachments.gc(std::time::SystemTime::now()).await {
            Ok(report) if report.removed > 0 || report.remaining_files > 0 => {
                note!(
                    "attachment gc: removed={} remaining={} bytes={}",
                    report.removed,
                    report.remaining_files,
                    report.remaining_bytes,
                );
            }
            Ok(_) => {}
            Err(err) => note!("attachment gc failed: {err}"),
        }

        // 村の識別子（Spec 28）。**予定のティッカーが回り始める前**に確定させる —
        // 発火の途中で解決すると、識別子が未確定の窓で承認鍵が組めない。
        // 読めない・書けない村では空文字にして続ける（**空は承認鍵が一致しない側**
        // なので、前判定が走らなくなるだけで危険側へは倒れない）。
        let village_id = match store.village_id().await {
            Ok(id) => id,
            Err(err) => {
                note!("WARN village_id を確定できません（前判定は実行されません）: {err}");
                String::new()
            }
        };

        let shared = Arc::new(Shared {
            world: RwLock::new(world),
            mailboxes: RwLock::new(HashMap::new()),
            events,
            factory,
            backends: RwLock::new(HashMap::new()),
            secrets,
            store,
            attachments,
            log: RwLock::new(restored_log),
            tools: RwLock::new(ToolRegistry::new()),
            mcp: RwLock::new(crate::mcp::McpManager::default()),
            agent_mcp: RwLock::new(HashMap::new()),
            schedules: RwLock::new(schedules),
            probe_approvals: RwLock::new(None),
            village_id: RwLock::new(village_id),
            plan_waves: RwLock::new(PlanWaveStore::default()),
            turns: Mutex::new(HashMap::new()),
            turn_seq: std::sync::atomic::AtomicU64::new(1),
            schedules_blocked,
            sessions,
            summaries: RwLock::new(restored_summaries),
            session_id: std::sync::RwLock::new(session_id),
            external_gate: tokio::sync::Semaphore::new(1),
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
    /// エージェントを新規登録する。
    ///
    /// **`spec.role_id` が指す役職があれば、ここで既定値を流し込む**（Spec 14）。
    /// `role_contract` 凍結 4 のとおり、**流し込みの発火点はこの 1 箇所だけ** —
    /// `update_agent` も `upsert_role` も既存の個体には触らない。ゆえに
    /// 「既存の個体は変わらない」があらゆる操作について成立する。
    ///
    /// 役職が引けないときは**流し込まずに作る**（`role_id` は残す）。存在しない
    /// 役職を指したまま作成そのものを拒むと、村を配った先で役職が欠けている
    /// だけで新規作成ができなくなる。
    pub async fn create_agent(&self, mut spec: AgentSpec) -> CoreResult<AgentSnapshot> {
        let id = spec.id.clone();

        // 流し込みは登録の前。register_agent が id の安全性と重複を弾くので、
        // 弾かれる spec に対してファイルを書きに行かずに済む。
        let construct = {
            let world = self.shared.world.read().await;
            match spec.role_id.clone().and_then(|rid| world.role(&rid).ok().cloned()) {
                Some(role) => {
                    let dropped = role
                        .defaults
                        .apply_to(&mut spec, |tid| world.template(tid).is_ok());
                    // **黙って落とさない。** 人が今まさに操作している最中なので、
                    // 黙ると「入れたはずの設定が入っていない」が見えない。
                    if !dropped.is_empty() {
                        crate::note!(
                            "role apply: 役職 `{}` の {} は参照先が無いため入れませんでした",
                            role.name,
                            dropped.join(" / ")
                        );
                    }
                    role.defaults.construct.clone()
                }
                None => String::new(),
            }
        };

        {
            let mut world = self.shared.world.write().await;
            world.register_agent(spec)?;
        }

        // Construct.md は `AgentSpec` の欄ではないので、ここでしか書けない。
        // **登録の後**に書くのは、id の検査を通った後でないとディレクトリを
        // 作る先が確定しないため。書き込みの失敗で登録を巻き戻さない —
        // 個体は既に村に居り、本文は設定ダイアログから書き直せる。
        if !construct.trim().is_empty() {
            if let Err(err) = self
                .shared
                .store
                .write_config(&id, ConfigFileKind::Construct, &construct)
                .await
            {
                crate::note!("role apply: Construct.md を書けませんでした: {err}");
            }
        }

        self.persist().await?;
        self.shared.emit(CoreEvent::TopologyChanged);
        self.snapshot(&id).await
    }

    // ---- 役職 (Spec 14) -----------------------------------------------------

    /// 登録済みの役職一覧。
    pub async fn list_roles(&self) -> Vec<AgentRole> {
        self.shared.world.read().await.roles()
    }

    /// 役職を登録または更新する。
    ///
    /// **既存のサーヴァントには何も起きない**（`role_contract` 凍結 4）。
    /// 中身はコピー済みなので、変わるのは `name` を参照している表示だけ。
    pub async fn upsert_role(&self, role: AgentRole) -> CoreResult<()> {
        // 改名は**その役職を持つ全個体**の表示を動かすので、影響範囲を先に取る
        // （機構 7 の発火は「表示名が変わったか」の 1 点で、操作の種類では分けない）。
        let affected = self.holders_of(&role.id, &role.name).await;
        {
            let mut world = self.shared.world.write().await;
            world.upsert_role(role);
        }
        for (id, before) in affected {
            let after = {
                let world = self.shared.world.read().await;
                world
                    .agent(&id)
                    .ok()
                    .and_then(|record| world.role_label(record.spec.role_id.as_ref()))
                    .map(str::to_owned)
            };
            self.announce_role_change(&id, before.as_deref(), after.as_deref())
                .await;
        }
        self.persist().await
    }

    /// その役職を持つ個体と、**変更前の**表示名の対。
    ///
    /// `new_name` は使わない（比較は `announce_role_change` が変更後に行う）が、
    /// 呼び出し側の意図を型で示すために受ける。
    async fn holders_of(&self, role_id: &AgentRoleId, _new_name: &str) -> Vec<(AgentId, Option<String>)> {
        let world = self.shared.world.read().await;
        world
            .snapshots()
            .into_iter()
            .filter(|snapshot| snapshot.role_id.as_ref() == Some(role_id))
            .map(|snapshot| {
                let before = world.role_label(snapshot.role_id.as_ref()).map(str::to_owned);
                (snapshot.id, before)
            })
            .collect()
    }

    /// 役職を削除する。**参照中でも拒まない**（`remove_template` との決定的な差）。
    ///
    /// 役職はコピー済みなので、消してもサーヴァントの動作は変わらない —
    /// バッジと顔ぶれの `[...]` が消えるだけ（`role_contract` 凍結 5）。
    pub async fn remove_role(&self, id: &AgentRoleId) -> CoreResult<()> {
        let affected = self.holders_of(id, "").await;
        {
            let mut world = self.shared.world.write().await;
            world.remove_role(id)?;
        }
        // 削除後は引けなくなるので after は必ず None。
        for (agent_id, before) in affected {
            self.announce_role_change(&agent_id, before.as_deref(), None)
                .await;
        }
        self.persist().await
    }

    /// エージェント定義を差し替える。
    ///
    /// 稼働中でも受け付ける。次の発話から新しい設定が反映される
    /// （プロンプトはメッセージごとに組み直すため）。
    pub async fn update_agent(&self, spec: AgentSpec) -> CoreResult<AgentSnapshot> {
        let id = spec.id.clone();
        // 役職表示（Spec 14）。**設定は 1 欄も流し込まない** — 流し込みの発火点は
        // 新規作成ただ 1 つ（role_contract 凍結 4）。ここで見るのは表示だけ。
        let (before, after) = {
            let mut world = self.shared.world.write().await;
            let before = world
                .agent(&id)
                .ok()
                .and_then(|record| world.role_label(record.spec.role_id.as_ref()))
                .map(str::to_owned);
            world.update_agent(spec)?;
            let after = world
                .agent(&id)
                .ok()
                .and_then(|record| world.role_label(record.spec.role_id.as_ref()))
                .map(str::to_owned);
            (before, after)
        };
        self.announce_role_change(&id, before.as_deref(), after.as_deref())
            .await;
        self.persist().await?;
        self.shared.emit(CoreEvent::TopologyChanged);
        self.snapshot(&id).await
    }

    /// 役職表示が変わったことを System 行 1 本で場に流す（Spec 14 機構 7）。
    ///
    /// **判定は「表示名が変わったか」の 1 点。** 付与・変更（改名を含む）・削除を
    /// 操作の種類で分けない — 分けると「改名だけ通知が出ない」のような穴が空く。
    /// 他のサーヴァントから見れば、どれも「あの個体の役職表示が変わった」で同じ事象。
    ///
    /// **これは保証ではない。** 届くのは `compose_presence_notices` 経由なので、
    /// 広場ログをオプトアウトした個体には届かず、窓から押し出されれば消える。
    /// 「自己申告する」を仕様の約束にはしない（Spec 14 機構 7）。
    async fn announce_role_change(&self, id: &AgentId, before: Option<&str>, after: Option<&str>) {
        if before == after {
            return;
        }
        let name = {
            let world = self.shared.world.read().await;
            match world.agent(id) {
                Ok(record) => record.spec.name.clone(),
                // 個体が消えていれば知らせる相手の話題も消えている。
                Err(_) => return,
            }
        };
        let text = match (before, after) {
            (None, Some(now)) => format!("{id}（{name}）の役職が「{now}」になりました"),
            (Some(_), Some(now)) => format!("{id}（{name}）の役職が「{now}」になりました"),
            (Some(was), None) => format!("{id}（{name}）の役職「{was}」が外れました"),
            (None, None) => return,
        };
        // 入退室通知と同じ経路（from: System / to: User。record のみで配送しない）。
        self.shared
            .record(AgentMessage::new(Endpoint::System, Endpoint::User, text, 0))
            .await;
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

    // ---- 村の設定 (Spec 13) -------------------------------------------------

    /// トークン予算の天井（実効トークン建て）を返す。`None` = 天井なし。
    pub async fn token_budget(&self) -> Option<u64> {
        self.shared.world.read().await.token_budget()
    }

    /// トークン予算の天井を差し替え、`world.json` へ書き戻す。
    ///
    /// 天井は依頼のたびに `new_root_budget` が `World` から読むので、
    /// **次の依頼から効く**。再起動は要らない（`settings_contract` の即時反映 —
    /// `world.json` は所有者ではなく投影）。
    ///
    /// # Errors
    /// - `Some(0)` は [`CoreError::InvalidTokenBudget`]。読み込み時の
    ///   `Some(0) → None` 正規化は外部編集の遡及回収であって、この経路で
    ///   受け付けて黙って倒すと「保存したのに別の値になる」
    pub async fn set_token_budget(&self, ceiling: Option<u64>) -> CoreResult<()> {
        if ceiling == Some(0) {
            return Err(CoreError::InvalidTokenBudget);
        }
        self.shared.world.write().await.set_token_budget(ceiling);
        self.persist().await
    }

    /// UI の表示言語。bootstrap が必ず確定させるので、未確定は起こらない
    /// （防御の既定は従来の見た目 = 日本語）。
    pub async fn language(&self) -> crate::world::Language {
        self.shared
            .world
            .read()
            .await
            .language()
            .unwrap_or(crate::world::Language::Ja)
    }

    /// UI の表示言語を差し替え、`world.json` へ書き戻す。
    ///
    /// コアはこの値で分岐しない（settings_contract の案 A）ので、
    /// プロンプト・バックエンド・履歴のどれにも触らない — 変わるのは
    /// `World` の 1 フィールドと投影だけ。
    pub async fn set_language(&self, language: crate::world::Language) -> CoreResult<()> {
        self.shared.world.write().await.set_language(language);
        self.persist().await
    }

    /// 利用者の呼び名（Spec 19）。`None` = 未設定。
    ///
    /// 未設定を既定値へ倒さずそのまま返すのは、**画面が「未設定である」ことを
    /// 示せるようにする**ため（`language` と違い、こちらは未設定が正常な状態）。
    pub async fn user_name(&self) -> Option<String> {
        self.shared
            .world
            .read()
            .await
            .user_name()
            .map(str::to_owned)
    }

    /// 利用者のアイコン（WebP バイト列）。未設定なら `None`（Spec 19）。
    pub async fn user_icon(&self) -> CoreResult<Option<Vec<u8>>> {
        self.shared.store.read_user_icon().await
    }

    /// 利用者のアイコンを保存する（Spec 19）。
    ///
    /// # Errors
    /// WebP でない・サイズ上限超過の場合 [`CoreError::InvalidIcon`]。
    /// 検証は**エージェントのアイコンと同じ述語**を通る（`icon_contract`）。
    pub async fn set_user_icon(&self, bytes: &[u8]) -> CoreResult<()> {
        self.shared.store.write_user_icon(bytes).await
    }

    /// 利用者のアイコンを削除する。未設定でも成功（Spec 19）。
    pub async fn clear_user_icon(&self) -> CoreResult<()> {
        self.shared.store.delete_user_icon().await
    }

    /// 利用者の呼び名を差し替え、`world.json` へ書き戻す。`None` で既定へ戻す。
    ///
    /// 次のターンの封筒から効く（`attribute_sender` は呼び出しのたびに `World`
    /// から引く）。**過去の履歴と会話ログの `【送り手: 旧名】` は直さない** —
    /// 残り香を消す機構は作らない（`user_identity_contract` 凍結 8）。
    ///
    /// # Errors
    /// 書式が受け入れ条件を満たさない場合 [`CoreError::InvalidUserName`]。
    /// **拒否したときはメモリもファイルも触らない。**
    pub async fn set_user_name(&self, name: Option<&str>) -> CoreResult<()> {
        self.shared.world.write().await.set_user_name(name)?;
        self.persist().await
    }

    /// 外部クライアントの呼び名（Spec 25）。`None` = 未設定（名乗りへ落ちる）。
    pub async fn external_name(&self) -> Option<String> {
        self.shared
            .world
            .read()
            .await
            .external_name()
            .map(str::to_owned)
    }

    /// 外部クライアントの呼び名を差し替える。`None` で未設定へ戻す（Spec 25）。
    ///
    /// # Errors
    /// 書式が受け入れ条件を満たさない場合 [`CoreError::InvalidUserName`]。
    pub async fn set_external_name(&self, name: Option<&str>) -> CoreResult<()> {
        self.shared.world.write().await.set_external_name(name)?;
        self.persist().await
    }

    /// 外部クライアントのアイコン（WebP バイト列）。未設定なら `None`（Spec 25）。
    pub async fn external_icon(&self) -> CoreResult<Option<Vec<u8>>> {
        self.shared.store.read_external_icon().await
    }

    /// 外部クライアントのアイコンを保存する（Spec 25）。
    ///
    /// # Errors
    /// WebP でない・サイズ上限超過の場合 [`CoreError::InvalidIcon`]。
    /// **検証はエージェント・利用者と同じ述語**（`icon_contract`）を通る。
    pub async fn set_external_icon(&self, bytes: &[u8]) -> CoreResult<()> {
        self.shared.store.write_external_icon(bytes).await
    }

    /// 外部クライアントのアイコンを削除する（Spec 25）。
    pub async fn clear_external_icon(&self) -> CoreResult<()> {
        self.shared.store.delete_external_icon().await
    }

    /// 外部からの依頼を受ける窓口（Spec 25 D2）。`None` = 未設定。
    pub async fn reception(&self) -> Option<AgentId> {
        self.shared.world.read().await.reception().cloned()
    }

    /// 窓口を差し替える。`None` で未設定へ戻す（Spec 25 D2）。
    ///
    /// # Errors
    /// 指定したエージェントが未登録の場合 [`CoreError::AgentNotFound`]。
    pub async fn set_reception(&self, agent_id: Option<&AgentId>) -> CoreResult<()> {
        self.shared.world.write().await.set_reception(agent_id)?;
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

    // ---- コマンドの承認（Spec 20） ---------------------------------------------

    /// 全サーヴァントの `run.json` を読む。承認画面の投影用。
    ///
    /// **壊れている個体は `Err` を畳んで飛ばす**（既定を返さない）。既定を返すと
    /// 画面には「判断待ちゼロ・許可ゼロ」に見え、**壊れていることが分からない**。
    /// 読めなかった事実は `broken` に載せて画面へ運ぶ。
    pub async fn command_policies(&self) -> Vec<CommandPolicyView> {
        let mut views = Vec::new();
        for snapshot in self.snapshots().await {
            let view = match self.shared.store.read_command_policy(&snapshot.id).await {
                Ok(policy) => CommandPolicyView {
                    agent_id: snapshot.id,
                    name: snapshot.name,
                    pending: policy.pending,
                    broken: false,
                },
                Err(_) => CommandPolicyView {
                    agent_id: snapshot.id,
                    name: snapshot.name,
                    pending: Vec::new(),
                    broken: true,
                },
            };
            views.push(view);
        }
        views
    }

    /// `pending` の 1 件を承認して `allow` へ入れる（Spec 20）。
    ///
    /// **粒度は `open` だけで決まる。** パターン文字列を外から受け取らないのは、
    /// 受け取ると「粒度は機械が決めない」が**「粒度を GUI が何でも決められる」へ
    /// 反転する**ため（`*` 1 文字も送れてしまう）。
    ///
    /// `allow` の 1 件目が入ると、**次のターンからそのサーヴァントは実際に
    /// コマンドを実行できるようになる**。**提示はその前から起きている** —
    /// `run` は `enabledTools` にチェックがあれば `allow` が空でも提示される
    /// （2026-08-06 の撤回。提示が承認を待つと、要求が積めず**承認する対象が
    /// 生まれない**閉じた輪になっていた）。
    pub async fn approve_command(
        &self,
        id: &AgentId,
        command: &str,
        args: &[String],
        open: bool,
    ) -> CoreResult<ApprovalOutcome> {
        self.shared
            .store
            .update_command_policy(id, |policy| policy.approve(command, args, open))
            .await
    }

    /// `pending` の 1 件を却下して `deny` へ入れる（Spec 20）。
    pub async fn reject_command(
        &self,
        id: &AgentId,
        command: &str,
        args: &[String],
        open: bool,
    ) -> CoreResult<ApprovalOutcome> {
        self.shared
            .store
            .update_command_policy(id, |policy| policy.reject(command, args, open))
            .await
    }

    // ---- 村の黒板 -------------------------------------------------------------

    /// 村の黒板（work_dir の `blackboard/`）を読む。GUI 投影用・読み取り専用。
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

    /// 入力欄のパス補完へ渡すファイル一覧（Spec 24）。
    ///
    /// 作業フォルダが**未設定なら空の一覧**を返す。UI はそもそも
    /// `AgentSnapshot.workDir` を持っているので、未設定のときは呼ばずに
    /// 理由を出せる（`AgentSettingsDialog` の `noWorkDirWarn` と同じ形）—
    /// **判断に必要な情報を既に持っている層で判断する。**
    ///
    /// **囲いはここに無い**（Spec 24 Notes 5）。返すのは候補であって権限ではなく、
    /// 挿入されたパスを実際に読むのは `file` / `grep` で、あちらが
    /// `resolve_in_work_dir` で境界を守る。**ここに検査を足すと同じ規律が
    /// 2 箇所に生える**（「参照…」ボタンで選ばれたパスを検査しないのと同じ判断）。
    pub async fn list_work_dir_files(&self, id: &AgentId) -> CoreResult<WorkDirListing> {
        let work_dir = {
            let world = self.shared.world.read().await;
            world.agent(id)?.spec.work_dir.clone()
        };
        let Some(work_dir) = work_dir else {
            return Ok(WorkDirListing {
                paths: Vec::new(),
                truncated: false,
            });
        };
        // 走査は同期 I/O なので blocking へ逃がす。20,000 件の走査で
        // ランタイムのワーカーを塞ぐと、その間ほかのエージェントのターンが止まる。
        let (paths, truncated) = tokio::task::spawn_blocking(move || {
            crate::tools::fs::relative_file_paths(std::path::Path::new(&work_dir))
        })
        .await
        .unwrap_or_else(|_| (Vec::new(), false));
        Ok(WorkDirListing { paths, truncated })
    }

    /// 添付画像の実体（WebP バイト列）を読む（Spec 23。表示用）。
    ///
    /// `None` は「保持期間を過ぎて削除された」（D9）— エラーではなく
    /// 通常の答えで、UI はプレースホルダの枠を出す。
    ///
    /// # Errors
    /// id が UUID の字種でない場合 [`CoreError::UnsafeIdentifier`]。
    pub async fn read_attachment(&self, id: &str) -> CoreResult<Option<Vec<u8>>> {
        self.shared.attachments.read(id).await
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
        if let Some(existing) = tasks.get(id) {
            // 稼働中なら断る。**ただし「登録がある」と「走っている」は別**
            // （失敗して自分から降りたタスクの登録は残る）。
            if !existing.join.is_finished() {
                return Err(CoreError::AlreadyRunning {
                    agent_id: id.to_string(),
                });
            }

            // 失敗して降りた残骸をここで回収する（reap）。`agent_loop` は
            // `tasks` に手が届かない（Orchestrator が持つ）ので自分の登録を
            // 消せず、停止経路の `stop_agent` に当たる後始末が失敗経路には
            // 無かった。回収しないと ON が `AlreadyRunning` で弾かれ、
            // **画面のトグルは OFF に見えたままアプリを再起動するまで戻せない**。
            if let Some(dead) = tasks.remove(id) {
                let _ = dead.join.await; // 完了済みなので即座に返る
            }

            // `stop_agent` が join のあとに畳むものを、ここで畳む。
            // 個別 MCP を落とさずに起動し直すと、子プロセスが 1 世代ぶん残る。
            if let Some(state) = self.shared.agent_mcp.write().await.remove(id) {
                state.manager.shutdown().await;
            }
            let had_error = {
                let mut world = self.shared.world.write().await;
                match world.agent_mut(id) {
                    Ok(record) => {
                        // 失敗した瞬間までを稼働時間に含める。畳まないと
                        // `started_at` が残り、停止しているのにカードの
                        // 稼働時間が増え続ける。
                        if let Some(started) = record.started_at.take() {
                            record.accumulated_uptime_secs += started.elapsed().as_secs();
                        }
                        record.last_error.is_some()
                    }
                    Err(_) => false,
                }
            };

            // 回収したことを 1 行残す。**これが無いと、失敗からの復帰は
            // 「再起動の行が無いのに次のターンが始まっている」という
            // 不在からの推測でしか読めない** — 沈黙を根拠に使う形になる
            // （failures.md #77 の一般化 1）。
            note!("agent reaped: agent={id} had_error={had_error}");
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
                // **履歴は消さない**（Spec 12 で変更。それ以前はここで clear していた）。
                //
                // 旧い規律は「起動は新しい会話の開始として扱う」で、会話を始め直す
                // 手段が他に無かった時期の代用だった。会話の寿命がセッションに
                // なった今、始め直しは「新規チャット」= 新しいセッションが担う。
                // ここで消すと、**再起動して開いた会話の履歴を、エージェントを
                // 起動した瞬間に捨てることになる** — 起動時に自動起動しない契約と
                // 組み合わさって、S1（続きから始められる）が原理的に成立しない。
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
        self.send_user_message_with_attachments(to, content, co_recipients, Vec::new())
            .await
    }

    /// 添付画像つきのユーザー発話を投入する（Spec 23）。
    ///
    /// 画像はここで検証して `{workspace}/attachments/` へ保存し、発話には
    /// **参照だけ**を載せる。上限は 1 発話 1 枚（D5）。
    ///
    /// # Errors
    /// 2 枚以上・検証に落ちる画像は [`CoreError::InvalidAttachment`]
    /// （何も書かず、発話も投入しない）。
    pub async fn send_user_message_with_attachments(
        &self,
        to: &AgentId,
        content: &str,
        co_recipients: &[AgentId],
        uploads: Vec<AttachmentUpload>,
    ) -> CoreResult<()> {
        if uploads.len() > 1 {
            return Err(CoreError::InvalidAttachment {
                reason: "1 つの発話に添付できる画像は 1 枚までです".to_owned(),
            });
        }
        // 保存は発話の記録より**前**。検証に落ちたら発話ごと拒否する —
        // 「画像なしで送信されました」は、送った人の意図と黙って食い違う。
        let mut attachments = Vec::with_capacity(uploads.len());
        for upload in &uploads {
            attachments.push(
                self.shared
                    .attachments
                    .save(&upload.file_name, &upload.bytes)
                    .await?,
            );
        }

        let mut message = AgentMessage::new(
            Endpoint::User,
            Endpoint::Agent { id: to.clone() },
            content,
            0,
        );
        message.attachments = attachments;
        if co_recipients.len() >= 2 {
            message.co_recipients = co_recipients.to_vec();
        }
        self.shared.record(message.clone()).await;
        // 因果の根 — 予算はここで生まれる（Spec 11）。同報は宛先ごとに
        // このメソッドが呼ばれるので、宛先ごとに独立した予算になる（契約どおり）。
        let budget = new_root_budget(&self.shared).await;
        // 利用者の発話は参加者を数えない — 自動要約は予定の発火だけの機能で、
        // 人が話している間に履歴を畳むのは「押していない操作」になる。
        deliver(&self.shared, to, message, budget, None).await
    }

    // ---- 外部からの依頼（Spec 25） -------------------------------------------

    /// 外部の MCP クライアントからの依頼を窓口へ渡し、答えを待つ（Spec 25）。
    ///
    /// **オーケストレーションの機構は 1 つも増えない。** 外部の呼び出しは
    /// 構造的に「あるサーヴァントが別のサーヴァントへ `ask` する」のと同じで、
    /// [`deliver_and_wait`] をそのまま通る（待ち方も失敗の分類も既存のまま）。
    /// 増えるのは送り手が [`Endpoint::External`] であることだけ。
    ///
    /// # 因果の根
    ///
    /// 外部依頼は**予算の根の 3 種類目**（ユーザー発話 / 予定の発火 / これ）。
    /// hop は 0 から始まり、予算プールも新品になる。**だからこそ `max_hops` と
    /// トークンの天井は、扉を通る閉路を塞げない** — 塞ぐのは冒頭の同時 1 本の
    /// ゲートで、それが唯一の歯止め（`mcp_server_contract` 凍結 5）。
    ///
    /// # Errors
    /// - 窓口が未設定 [`CoreError::ExternalReceptionUnset`]
    /// - 窓口が削除済み [`CoreError::AgentNotFound`]
    /// - 窓口が停止中 [`CoreError::NotRunning`]
    /// - 別の外部依頼を処理中 [`CoreError::ExternalBusy`]
    pub async fn ask_external(&self, client: &str, message: &str) -> CoreResult<String> {
        // D7 — 同時 1 本。**待たずに即断る**（待つと閉路のデッドロックが
        // ask_timeout ぶん居座り、呼ぶ側からは「重い依頼」と区別が付かない）。
        // permit はこの関数を抜けるまで握る = 答えが返るまで次を通さない。
        let _permit = self
            .shared
            .external_gate
            .try_acquire()
            .map_err(|_| CoreError::ExternalBusy)?;

        let to = {
            let world = self.shared.world.read().await;
            let Some(to) = world.reception().cloned() else {
                return Err(CoreError::ExternalReceptionUnset);
            };
            // 窓口が削除されていれば「見つからない」を返す。**「未設定」へ
            // 畳まない** — 設定し直すのと初めて設定するのでは人の次の手が違う。
            world.agent(&to)?;
            to
        };
        // 停止中は黙って待たない（S6）。受信箱の有無が稼働の判定
        // （「ここに居る = 送信できる」の不変条件）。
        if !self.shared.mailboxes.read().await.contains_key(&to) {
            return Err(CoreError::NotRunning {
                agent_id: to.to_string(),
            });
        }

        // 自己申告の名乗りはここで 1 回だけ正規化する。**プロンプトへ入る前**で
        // なければ意味がない（`mcp_server_contract` 凍結 6）。
        let from = Endpoint::External {
            client: crate::world::normalize_client_name(client),
        };
        let budget = new_root_budget(&self.shared).await;
        // 因果の根なので親トークンを持たない。打ち切りは既存の
        // `interrupt_turn` / `interrupt_all` が窓口のターンに効く。
        let cancel = tokio_util::sync::CancellationToken::new();
        let (answer, _state) = deliver_and_wait(
            &self.shared,
            &from,
            &to,
            message,
            0,
            &cancel,
            budget.as_ref(),
            // 外部依頼は予定ではないので参加者を数えない（自動要約の対象外）。
            None,
        )
        .await;
        // 分類は捨てる（`ask` と同じ）。**失敗も文字列で返る** — 相手が
        // 答えなかった・時間切れだったは会話の事実であって、扉の故障ではない。
        Ok(answer)
    }

    // ---- 予定（Spec 07） -----------------------------------------------------

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
) -> CoreResult<()> {
    deliver_envelope(shared, to, Envelope::plain(message, budget, participants)).await
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

/// 呼び名が未設定のときに封筒へ入る名前（`user_identity_contract` 凍結 2）。
///
/// **言語に追従させない。** 会話ペインの表示名（`chat.you`）は追従するが、
/// こちらは追従しない — この非対称は Spec 19 の起票時からある既存の挙動で、
/// 揃えると既存の村の履歴の途中で送り手名が変わる。
pub const DEFAULT_USER_LABEL: &str = "ユーザー";

/// 受信した発話へ送り手の封筒を付ける。
///
/// ユーザーの言葉もエージェントからの転送も同じ user ロールで届くため、名前を
/// 書かないと受信側は区別できない — 実際にユーザーの発話を「他のエージェントが
/// 話した言葉」と取り違えた。**プロンプトと履歴の両方へ同じ形で入れる**ので、
/// 組み立てはこの 1 箇所に置く（2 箇所で組むと、片方だけ直したときに
/// 過去のターンだけ出所不明に戻る）。
async fn attribute_sender(shared: &Arc<Shared>, incoming: &AgentMessage) -> String {
    let sender_label = match &incoming.from {
        // 利用者が呼び名を設定していればそれを使う（Spec 19）。
        // **設定していないときは言語に追従させず既定のまま** — 追従させると
        // 既存の en の村で封筒が変わり、同じ会話の履歴の途中で送り手名が
        // 切り替わる（履歴は保存済みの文字列なので遡って直らない）。
        Endpoint::User => {
            let world = shared.world.read().await;
            world.user_name().unwrap_or(DEFAULT_USER_LABEL).to_owned()
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
            format!("{}（外部クライアント）", external_label(&world, client))
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
    crate::sender_envelope::wrap(&sender_label, &body)
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

/// このターンの受信に付いた添付参照を、ワイヤへ載せる形へ展開する（Spec 23）。
///
/// 読むのは `incoming.attachments` だけ — 履歴の発話は文字列なので、
/// **画像がプロンプトへ載るのはこの 1 ターン限り**（D1）が構造で成立する。
/// 読めなかった参照は抜いて返し、数の差は呼び出し側が本文で断る。
async fn load_turn_attachments(
    shared: &Arc<Shared>,
    agent_id: &AgentId,
    incoming: &AgentMessage,
) -> Vec<crate::llm::ImageAttachment> {
    use base64::Engine as _;
    let mut loaded = Vec::with_capacity(incoming.attachments.len());
    for reference in &incoming.attachments {
        match shared.attachments.read(&reference.id).await {
            Ok(Some(bytes)) => loaded.push(crate::llm::ImageAttachment {
                // 置き場の実体は常に WebP（保存時の検証が保証する）。
                // JPEG はワイヤ上のフォールバック（D3）でしか現れない。
                media_type: crate::llm::ImageMediaType::Webp,
                data: base64::engine::general_purpose::STANDARD.encode(&bytes),
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

/// 委譲・転送で添付が落ちることを、届く本文の先頭で断る（Spec 23 D6）。
///
/// `ask` / `plan` / `transfer_to_*` は画像を運ばない。黙って落とすと、
/// 転送先がなぜ画像を見られないのか誰にも診断できない — 「歯止めの先に
/// 道を書く」（#44）と Spec 12 P4 の「飛ばした相手が居たら必ず書く」の同型。
fn note_dropped_attachment(message: &str, incoming_had_attachments: bool) -> String {
    if incoming_had_attachments {
        format!("（画像 1 枚は転送されません）\n\n{message}")
    } else {
        message.to_owned()
    }
}
