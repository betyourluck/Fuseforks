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
use std::sync::{Arc, Weak};
use std::time::Duration;

use tokio::sync::{Mutex, RwLock, broadcast, mpsc, watch};
use tokio::task::JoinHandle;

use crate::compute;
use crate::config_store::ConfigStore;
use crate::error::{CoreError, CoreResult, ErrorPayload};
use crate::event::CoreEvent;
use crate::llm::{
    BackendFactory, ChatMessage, ChatRequest, ChatResponse, LlmBackend, ToolSpec,
};
use crate::model::{
    AgentId, AgentMessage, AgentSnapshot, AgentSpec, AgentStatus, ConfigFileKind, CredentialSource,
    Endpoint, ModelTemplate, ModelTemplateId, TopologyEdge,
};
use crate::rag::{RagChunk, RagIndex};
use crate::tool::{AgentTool, ToolContext, ToolRegistry};
use crate::secret::SecretStore;
use crate::world::World;

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
    reply_to: Option<tokio::sync::oneshot::Sender<String>>,
}

impl Envelope {
    /// 返信を求めない通常の配送。
    fn plain(incoming: AgentMessage) -> Self {
        Self {
            incoming,
            reply_to: None,
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
    config: OrchestratorConfig,
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
    async fn set_status(&self, id: &AgentId, status: AgentStatus) {
        let changed = {
            let mut world = self.world.write().await;
            match world.agent_mut(id) {
                Ok(record) if record.status != status => {
                    record.status = status;
                    true
                }
                _ => false,
            }
        };
        if changed {
            self.emit(CoreEvent::AgentStatusChanged {
                agent_id: id.clone(),
                status,
            });
        }
    }
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
            config,
        });

        let stats_task = spawn_stats_ticker(Arc::downgrade(&shared));

        Ok(Self {
            shared,
            tasks: Mutex::new(HashMap::new()),
            stats_task,
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
}

impl Drop for Orchestrator {
    fn drop(&mut self) {
        // 統計ティッカーは純粋な副作用タスクなので、ここは abort でよい。
        self.stats_task.abort();
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

                // 入力中表示。処理は LLM 呼び出しを含み数十秒かかりうるので、
                // 開始と終了を対で流す。終了は成功・失敗を問わず必ず流す —
                // 片方だけだと「入力中…」が出しっぱなしになる。
                shared.emit(CoreEvent::AgentTyping {
                    agent_id: agent_id.clone(),
                    active: true,
                });
                let outcome = handle_message(&shared, &agent_id, envelope).await;
                shared.emit(CoreEvent::AgentTyping {
                    agent_id: agent_id.clone(),
                    active: false,
                });

                if let Err(err) = outcome {
                    let payload = ErrorPayload::from(&err);
                    let fatal = !err.is_retryable();

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

/// 受信した発話を 1 件処理する。
///
/// 手順: プロンプト組み立て → RAG 付与 → LLM 呼び出し → 統計更新 → 記録 → 転送。
async fn handle_message(
    shared: &Arc<Shared>,
    agent_id: &AgentId,
    envelope: Envelope,
) -> CoreResult<()> {
    let Envelope { incoming, reply_to } = envelope;
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
    let (system_prompt, stable_len) = shared
        .store
        .compose_system_prompt(&spec, template.grounding_active())
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

    // 4. プロンプトを組む。順序は system → 手順 → 参照資料 → 履歴 → 今回の受信。
    let mut messages = vec![ChatMessage::system(system_prompt)];
    if !handoffs.is_empty() {
        messages.push(ChatMessage::system(handoffs.protocol_note(use_handoff_tools)));
    }

    // RAG。Rayon 側で検索するので、この待ち時間に他エージェントも進む。
    if !spec.rag_sources.is_empty() {
        let hits = shared
            .rag
            .read()
            .await
            .search(&spec.rag_sources, &incoming.content, shared.config.rag_top_k)
            .await?;
        if !hits.is_empty() {
            let context = hits
                .iter()
                .map(|h| format!("- [{}] {}", h.item.source, h.item.text))
                .collect::<Vec<_>>()
                .join("\n");
            messages.push(ChatMessage::system(format!("## 参照資料\n{context}")));
        }
    }

    // 居合わせた会話（広場ログ）。自分の履歴より前に置く — 場の背景であって、
    // 自分とのやり取りではない。受信側でオプトアウトできる（Spec 03）:
    // 毎ターン最大 12 件 × 200 字の固定費であり、場の共有が要らない役には
    // 価値が無い。false でも自分の発話は他者の広場ログに載る（受信側だけの設定）。
    if spec.hears_room_log
        && let Some(room) = compose_room_log(shared, agent_id, &shared.config).await
    {
        messages.push(ChatMessage::system(room));
    }

    // 履歴。これが無いと毎回コールドスタートになり、同じ入力に同じ出力を返し続ける。
    {
        let world = shared.world.read().await;
        if let Ok(record) = world.agent(agent_id) {
            messages.extend(record.history.iter().cloned());
        }
    }

    // 送り手の封筒。ユーザーの言葉もエージェントからの転送も同じ user ロールで
    // 届くため、名前を書かないと受信側は区別できない — 実際にユーザーの発話を
    // 「他のエージェントが話した言葉」と取り違えた。プロンプトと履歴の両方へ
    // 同じ形で入れる。履歴に入れないと、次のターンで再び出所不明になる。
    let sender_label = match &incoming.from {
        Endpoint::User => "ユーザー".to_owned(),
        Endpoint::System => "システム".to_owned(),
        Endpoint::Agent { id } => {
            let world = shared.world.read().await;
            world
                .agent(id)
                .map(|record| record.spec.name.clone())
                // 送り手が既に削除されていても発話は成立させる。ID で示す。
                .unwrap_or_else(|_| id.to_string())
        }
    };
    let attributed = format!("【送り手: {sender_label}】\n{}", incoming.content);

    // 同報の注記。「みんなへ」と呼びかけられたのに自分しか受け取っていないように
    // 見えると、各エージェントは律儀に接続先へ転送して反響が起きる（実機で観測）。
    // 転送を禁止するのではなく、「全員が既に受け取っている」という事実を与えて
    // 転送する理由そのものを消す。プロンプトキャッシュの安定プレフィックス
    // （system 先頭）には影響しない位置に差す。
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
        messages.push(ChatMessage::system(format!(
            "【同報】この発話はあなたを含む {} 体（{}）へ同時に届いています。\
             全員が同じ内容を既に受け取っており、**それぞれが自分で答えます**。\
             したがって、この内容を他のエージェントへ転送する必要はありませんし、\
             他の参加者に発言を促す必要もありません。\
             あなたは**あなた自身の分だけ**答えてください。",
            names.len(),
            names.join("、")
        )));
    }

    messages.push(ChatMessage::user(&attributed));

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

    for iteration in 0..max_tool_iterations {
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

        for call in &calls {
            // 並列委譲は 1 回の呼び出しで N 体ぶんの仕事をする。ツール実行の
            // 上限（`max_tool_iterations`）の消費も 1 回で済む。
            let result = if use_handoff_tools
                && handoffs.offers_plan()
                && call.name == HandoffTools::PLAN
            {
                Ok(run_plan(shared, agent_id, &handoffs, call, incoming.hop).await)
            } else {
                match handoffs.resolve_ask(&call.name) {
                    Some(target) if use_handoff_tools => {
                        ask_agent(shared, agent_id, target, call, incoming.hop).await
                    }
                    _ => execute_tool(shared, agent_id, call).await,
                }
            };
            shared.emit(CoreEvent::ToolInvoked {
                agent_id: agent_id.clone(),
                tool: call.name.clone(),
                ok: result.is_ok(),
            });
            let body = match result {
                Ok(text) => text,
                // 失敗しても会話を止めない。モデルが読んで次を決める。
                Err(err) => format!("ツールの実行に失敗しました: {err}"),
            };
            messages.push(ChatMessage::tool_result(&call.id, &call.name, body));
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
    if let Outcome::Finish { content } = &outcome
        && content.trim().is_empty()
        && tool_limit_hit
    {
        messages.push(ChatMessage::system(
            "ツール実行の上限に達しました。これ以上ツールは使えません。\
             ここまでのツール結果から分かったことを、最終回答としてまとめてください。\
             調査が途中なら、どこまで分かっていて何が残っているかを書いてください。",
        ));
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
        *content = if tool_limit_hit {
            // 理由を必ず添える。「失敗しました」だけでは、設定を直せば済むのか
            // ワイヤの障害なのかを利用者が判別できない。
            let reason = summary_error
                .as_deref()
                .map(|err| format!("失敗の理由: {err}。"))
                .unwrap_or_else(|| "モデルは応答しましたが本文が空でした。".to_owned());
            format!(
                "（ツール実行の上限 {max_tool_iterations} 回に達し、まとめの生成にも\
                 失敗しました。{reason}\
                 エージェント設定で上限を上げるか、依頼を小さく分けてください。）"
            )
        } else {
            "（モデルから本文が返りませんでした。もう一度頼んでみてください。）".to_owned()
        };
    }

    // 7. 統計と履歴を更新する。履歴には「実際に言ったこと」を積む。
    //    受信側は封筒（送り手名）付きで積む — プロンプトと履歴の形を揃えないと、
    //    過去のターンだけ出所不明に戻る。
    {
        let mut world = shared.world.write().await;
        if let Ok(record) = world.agent_mut(agent_id) {
            record.total_tokens += tokens;
            record.cached_tokens += cached;
            record.prompt_tokens += prompt;
            record.push_exchange(
                &attributed,
                &outcome.spoken(),
                shared.config.history_turns,
            );
        }
    }

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
                let _ = reply_to.send(content.clone());
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
        let _ = reply_to.send(format!(
            "相手はこの依頼に自分で答えず、{names} へ会話を渡しました。\
             答えはこちらへ戻りません。必要なら別の相手に頼むか、自分で進めてください。"
        ));
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

    Ok(deliver_and_wait(shared, from, to, &question, next_hop).await)
}

/// 1 件の依頼を配送し、答えを待つ（`ask` と `plan` の共通部分）。
///
/// **切り出してあるのは、2 つの経路で失敗の文言と境界を揃えるため。**
/// 別々に書くと、同じ配置で ask は通り plan は止まる、という説明できない差が
/// いずれ生まれる。`hop` の判定は呼び出し側に置く — plan では波全体で
/// 一様に決まる制約なので、タスクごとに判定すると同じ文字列が人数分並ぶ。
///
/// 戻り値は**必ず文字列**。相手が停止中でも無応答でも例外にしない
/// （ツールの失敗で会話を止めない、という既存の規律）。
async fn deliver_and_wait(
    shared: &Arc<Shared>,
    from: &AgentId,
    to: &AgentId,
    question: &str,
    next_hop: u8,
) -> String {
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
    };

    if let Err(err) = deliver_envelope(shared, to, envelope).await {
        // 相手が停止中・受信箱が飽和。会話は止めず、モデルに事実を返す。
        return format!("相手に尋ねられませんでした: {err}");
    }

    match tokio::time::timeout(shared.config.ask_timeout, rx).await {
        Ok(Ok(answer)) => answer,
        // 相手が答えずにタスクを終えた（停止・失敗）。転送で応じた場合は
        // handle_message が事実を送るので、ここへは来ない。
        Ok(Err(_)) => "相手から答えが返りませんでした。".to_owned(),
        Err(_) => "相手からの答えが時間内に返りませんでした。".to_owned(),
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

    let mut wave: Vec<(AgentId, String)> = Vec::with_capacity(tasks.len());
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
        if wave.iter().any(|(existing, _)| *existing == target) {
            return format!(
                "宛先「{to}」が同じ波に 2 回あります。1 回の plan で同じ相手へ頼めるのは 1 件です。\
                 2 件目は次の波で頼んでください。何も配送していません。"
            );
        }
        wave.push((target, message.to_owned()));
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

    // 3. 並列配送。JoinSet で各タスクを実行時へ載せる — ここが `ask_*` の
    //    直列委譲との唯一の構造的な差で、壁時計が人数倍にならない理由。
    //    並列なのは**配送**であって実行ではない。各エージェントの受信箱は
    //    1 本なので、ワーカーが別の仕事で塞がっていればその分だけ待つ。
    let mut set = tokio::task::JoinSet::new();
    for (index, (target, message)) in wave.iter().enumerate() {
        let shared = Arc::clone(shared);
        let from = from.clone();
        let target = target.clone();
        let message = message.clone();
        set.spawn(async move {
            let answer = deliver_and_wait(&shared, &from, &target, &message, next_hop).await;
            (index, answer)
        });
    }

    let mut answers: Vec<Option<String>> = vec![None; wave.len()];
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok((index, answer)) => answers[index] = Some(answer),
            // タスク自体が落ちた（パニック）。1 件の異常で波ごと落とさない。
            Err(err) => tracing_note(&err),
        }
    }

    // 4. 束ねる。見出しは `agent_id（表示名）` — 表示名だけにしないのは、
    //    表示名の一意性がどこも保証されていないから（同名が 2 体いると
    //    どちらの答えか判別できなくなる）。順序は入力順に戻す。
    let bundle = wave
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
    eprintln!(
        "[concordia] plan bundle: agent={from} tasks={} chars={}",
        wave.len(),
        bundle.chars().count()
    );
    bundle
}

/// `JoinSet` のタスク異常を握り潰さずに記録する。
///
/// このクレートはログ基盤を持たない（GUI 層に一切依存しない制約）ので、
/// 標準エラーへ 1 行出すに留める。**黙って捨てない**ことだけが目的。
fn tracing_note(err: &tokio::task::JoinError) {
    eprintln!("[concordia] plan のタスクが異常終了しました: {err}");
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
            Endpoint::System => "システム".to_owned(),
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
                 結果が人間へ返ります。同じ内容を繰り返すくらいなら、会話を終えてください。",
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
