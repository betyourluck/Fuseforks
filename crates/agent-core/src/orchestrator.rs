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
    pub max_tool_iterations: u8,
    /// エージェント 1 体あたりの受信箱容量。溢れたら送信側にエラーを返す（背圧）。
    pub mailbox_capacity: usize,
    /// イベントバッファの容量。UI の描画が遅れても、この範囲までは取りこぼさない。
    pub event_capacity: usize,
    /// 稼働統計を押し出す間隔。
    pub stats_interval: Duration,
    /// 保持するメッセージログの最大件数。超えた分は古いほうから捨てる。
    pub log_capacity: usize,
    /// 1 回の応答生成で RAG から引く断片数。
    pub rag_top_k: usize,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            max_hops: 8,
            history_turns: 8,
            max_tool_iterations: 6,
            mailbox_capacity: 64,
            event_capacity: 1_024,
            stats_interval: Duration::from_secs(1),
            log_capacity: 5_000,
            rag_top_k: 4,
        }
    }
}

/// タスク間で共有される状態。
struct Shared {
    world: RwLock<World>,
    /// 稼働中エージェントの受信箱。停止時に取り除かれるので、
    /// 「ここに居る = 送信できる」という不変条件が成り立つ。
    mailboxes: RwLock<HashMap<AgentId, mpsc::Sender<AgentMessage>>>,
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
    /// 実行できるツール。将来 MCP サーバーのツールもここへ入る。
    tools: RwLock<ToolRegistry>,
    config: OrchestratorConfig,
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
    pub async fn write_config(
        &self,
        id: &AgentId,
        kind: ConfigFileKind,
        content: &str,
    ) -> CoreResult<()> {
        self.shared.world.read().await.agent(id)?;
        self.shared.store.write_config(id, kind, content).await
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
        let message = AgentMessage::new(
            Endpoint::User,
            Endpoint::Agent { id: to.clone() },
            content,
            0,
        );
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
    let sender = {
        let mailboxes = shared.mailboxes.read().await;
        mailboxes.get(to).cloned()
    };

    let sender = sender.ok_or_else(|| CoreError::NotRunning {
        agent_id: to.to_string(),
    })?;

    sender.try_send(message).map_err(|err| match err {
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
    mut inbox: mpsc::Receiver<AgentMessage>,
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
                let Some(message) = received else { break };

                if let Err(err) = handle_message(&shared, &agent_id, message).await {
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
    incoming: AgentMessage,
) -> CoreResult<()> {
    // 1. 定義とテンプレートを取り出す。ロックはここで手放し、LLM 呼び出しは持たずに行う。
    let (spec, template) = {
        let world = shared.world.read().await;
        let record = world.agent(agent_id)?;
        let template = world.template(&record.spec.model_template_id)?.clone();
        (record.spec.clone(), template)
    };

    // 2. システムプロンプトを組む。安定部分の長さも同時に得る（キャッシュ境界）。
    let (system_prompt, stable_len) = shared.store.compose_system_prompt(&spec).await?;

    // 3. 転送先ごとのツールを組む。
    //    OpenAI Agents SDK は handoff を「宛先 1 つにつきツール 1 本」で表現し、
    //    `transfer_to_<agent>` という名前を使う。単一ツール + 宛先パラメータより、
    //    名前で選ばせるほうがモデルの学習分布に近い。
    let targets = spec.connected_agents.clone();
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

    // 履歴。これが無いと毎回コールドスタートになり、同じ入力に同じ出力を返し続ける。
    {
        let world = shared.world.read().await;
        if let Ok(record) = world.agent(agent_id) {
            messages.extend(record.history.iter().cloned());
        }
    }
    messages.push(ChatMessage::user(&incoming.content));

    // 5. ツールを提示する。転送用と実行用を 1 つの集合としてモデルへ渡す。
    //    モデルから見れば「次に何をするか」の選択肢はどちらも同じ粒度で、
    //    転送だけ別扱いにする理由が無い。区別するのは受け取った後の私たち。
    let mut specs = if use_handoff_tools {
        handoffs.specs()
    } else {
        Vec::new()
    };
    let executable = shared.tools.read().await.specs();
    specs.extend(executable.iter().cloned());
    let use_tools = !specs.is_empty() && template.use_tools;

    // 6. 実行ループ。
    //    規則は OpenAI Agents SDK と同じ:
    //    ツールを呼んだら実行して結果を積み、もう一度呼ぶ。
    //    ツールを呼ばないテキスト出力が出たら、それが最終出力。
    let backend = shared.backend_for(&template).await?;
    let mut tokens = 0u64;
    let mut outcome = Outcome::Finish {
        content: String::new(),
    };

    for iteration in 0..shared.config.max_tool_iterations.max(1) {
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

        let response = backend.chat(request).await?;
        tokens += response.usage.total();

        // 転送の要求は「会話を渡す」ことなので、ここでループを抜ける。
        // 結果が返ってくる種類の操作ではない。
        outcome = handoffs.decide(&response, use_handoff_tools);
        if matches!(outcome, Outcome::Handoff { .. }) {
            break;
        }

        // 実行対象のツール呼び出しを拾う。転送用の名前はここには来ない
        // （上で Handoff として抜けている）。
        let calls: Vec<_> = response
            .tool_calls
            .iter()
            .filter(|call| executable.iter().any(|spec| spec.name == call.name))
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
            let result = execute_tool(shared, agent_id, call).await;
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
        if iteration + 1 == shared.config.max_tool_iterations.max(1) {
            shared.emit(CoreEvent::ToolLimitReached {
                agent_id: agent_id.clone(),
                max_iterations: shared.config.max_tool_iterations,
            });
        }
    }

    // 7. 統計と履歴を更新する。履歴には「実際に言ったこと」を積む。
    {
        let mut world = shared.world.write().await;
        if let Ok(record) = world.agent_mut(agent_id) {
            record.total_tokens += tokens;
            record.push_exchange(
                &incoming.content,
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
            // 会話はここで終わり。ユーザーへ返して転送しない。
            let mut outgoing = AgentMessage::new(from, Endpoint::User, content, next_hop);
            outgoing.tokens = tokens as u32;
            shared.record(outgoing).await;
            return Ok(());
        }
        Outcome::Handoff { deliveries } => deliveries,
    };

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

/// ツールを 1 本実行する。
///
/// 未知の名前でも `Err` にせず文字列を返すのは、モデルが読んで直せるようにするため。
/// ここで会話ごと落とすと、名前を打ち間違えただけでターンが終わる。
async fn execute_tool(
    shared: &Arc<Shared>,
    agent_id: &AgentId,
    call: &crate::llm::ToolCall,
) -> CoreResult<String> {
    let tool = {
        let registry = shared.tools.read().await;
        registry.get(&call.name).cloned()
    };

    let Some(tool) = tool else {
        return Ok(format!(
            "`{}` というツールはありません。提示された名前から選んでください。",
            call.name
        ));
    };

    let ctx = ToolContext {
        agent_id: agent_id.clone(),
    };
    tool.call(&ctx, &call.args).await
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
    /// `(ツール名, 転送先)`。名前からの逆引きに使う。
    entries: Vec<(String, AgentId)>,
}

impl HandoffTools {
    /// 接続先からツール名を導く。
    ///
    /// 名前は OpenAI Agents SDK の慣習に倣って `transfer_to_<agent>`。
    /// 関数名の長さ制限（64 文字）を超える場合と、切り詰めで衝突する場合は
    /// 連番へ退避する。名前が壊れるとモデルが呼べなくなるため、
    /// 「たぶん大丈夫」で通さない。
    fn build(targets: &[AgentId]) -> Self {
        const MAX_TOOL_NAME: usize = 64;
        const PREFIX: &str = "transfer_to_";

        let mut entries: Vec<(String, AgentId)> = Vec::with_capacity(targets.len());
        for (index, target) in targets.iter().enumerate() {
            let natural = format!("{PREFIX}{target}");
            let name = if natural.len() <= MAX_TOOL_NAME
                && !entries.iter().any(|(existing, _)| *existing == natural)
            {
                natural
            } else {
                format!("{PREFIX}agent_{index}")
            };
            entries.push((name, target.clone()));
        }
        Self { entries }
    }

    /// 転送先が 1 つも無いか。
    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// wire へ載せるツール定義。
    fn specs(&self) -> Vec<ToolSpec> {
        self.entries
            .iter()
            .map(|(name, target)| ToolSpec {
                name: name.clone(),
                description: format!(
                    "会話を続ける必要があるとき、`{target}` へメッセージを渡す。\
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
            "## 会話の進め方\n\
             他のエージェントへ話を渡す必要があるときだけ `transfer_to_*` ツールを呼んでください。\
             **複数の相手へ渡すときは、それぞれの `transfer_to_*` を同じ応答の中で同時に呼んでください**。\
             全員へ並行して届きます。\
             自分の応答で用が足りる場合、または相手の発言に返すべきことが残っていない場合は、\
             **ツールを呼ばずに本文だけを返してください**。その時点で会話は終わり、\
             結果が人間へ返ります。同じ内容を繰り返すくらいなら、会話を終えてください。"
                .to_owned()
        } else {
            format!(
                "## 会話の進め方\n\
                 応答は次のエージェントへ渡されます。会話を終えてよいと判断したら、\
                 本文の末尾に {TERMINATION_MARKER} と書いてください。その時点で会話は終わり、\
                 結果が人間へ返ります。同じ内容を繰り返すくらいなら、会話を終えてください。"
            )
        }
    }

    /// 名前からツールを逆引きする。
    fn resolve(&self, name: &str) -> Option<&AgentId> {
        self.entries
            .iter()
            .find(|(tool, _)| tool == name)
            .map(|(_, target)| target)
    }

    /// 最初の転送先。ツールを使えない経路の退避先。
    fn first(&self) -> Option<&AgentId> {
        self.entries.first().map(|(_, target)| target)
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
