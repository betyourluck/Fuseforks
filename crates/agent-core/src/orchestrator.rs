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
//! # 無限往復の抑止
//!
//! 相互接続されたエージェントは、放っておくと際限なく往復して課金を焼き続ける。
//! 各発話は `hop` を持ち、[`OrchestratorConfig::max_hops`] に達した時点で連鎖を打ち切る。
//! 打ち切りは [`CoreEvent::HopLimitReached`] で通知する。黙って止めると
//! 「なぜ会話が終わったのか」が UI から永久に分からなくなる。

use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::Duration;

use tokio::sync::{Mutex, RwLock, broadcast, mpsc, watch};
use tokio::task::JoinHandle;

use crate::compute;
use crate::config_store::ConfigStore;
use crate::error::{CoreError, CoreResult, ErrorPayload};
use crate::event::CoreEvent;
use crate::llm::{BackendFactory, ChatMessage, ChatRequest, LlmBackend};
use crate::model::{
    AgentId, AgentMessage, AgentSnapshot, AgentSpec, AgentStatus, ConfigFileKind, CredentialSource,
    Endpoint, ModelTemplate, ModelTemplateId, TopologyEdge,
};
use crate::rag::{RagChunk, RagIndex};
use crate::secret::SecretStore;
use crate::world::World;

/// オーケストレーターの動作パラメータ。
#[derive(Debug, Clone)]
pub struct OrchestratorConfig {
    /// 1 つのユーザー入力から派生する転送の最大回数。
    pub max_hops: u8,
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
        let world = World::from_persisted(persisted.clone());

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
    pub async fn upsert_template(&self, template: ModelTemplate) -> CoreResult<()> {
        let id = template.id.clone();
        self.shared.world.write().await.upsert_template(template);
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

    // 3. RAG。Rayon 側で検索するので、この待ち時間に他エージェントも進む。
    let mut messages = vec![ChatMessage::system(system_prompt)];
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
    messages.push(ChatMessage::user(&incoming.content));

    // 4. LLM 呼び出し。
    let request = ChatRequest {
        model: template.model.clone(),
        messages,
        tools: Vec::new(),
        tool_choice: crate::llm::ToolChoice::None,
        temperature: template.temperature,
        max_tokens: template.max_output_tokens,
        effort: template.effort,
        cacheable_prefix_len: stable_len,
    };
    let backend = shared.backend_for(&template).await?;
    let response = backend.chat(request).await?;
    let content = response.text.unwrap_or_default();
    let tokens = response.usage.total();

    // 5. 統計を更新する。
    {
        let mut world = shared.world.write().await;
        if let Ok(record) = world.agent_mut(agent_id) {
            record.total_tokens += tokens;
        }
    }

    // 6. 宛先を決める。接続先が無ければユーザーへ返す。
    let next_hop = incoming.hop.saturating_add(1);
    let targets = spec.connected_agents.clone();
    let destinations: Vec<Endpoint> = if targets.is_empty() {
        vec![Endpoint::User]
    } else {
        targets
            .iter()
            .map(|id| Endpoint::Agent { id: id.clone() })
            .collect()
    };

    // 7. 記録と転送。1 回の生成を複数の宛先へ配るとき、トークンは
    //    先頭の 1 件にだけ載せる。全件に載せると同じ生成を人数分二重計上してしまう。
    for (index, destination) in destinations.into_iter().enumerate() {
        let mut outgoing = AgentMessage::new(
            Endpoint::Agent {
                id: agent_id.clone(),
            },
            destination.clone(),
            &content,
            next_hop,
        );
        outgoing.tokens = if index == 0 { tokens as u32 } else { 0 };
        shared.record(outgoing.clone()).await;

        let Some(target) = destination.agent_id() else {
            continue;
        };

        if next_hop >= shared.config.max_hops {
            shared.emit(CoreEvent::HopLimitReached {
                agent_id: agent_id.clone(),
                max_hops: shared.config.max_hops,
            });
            continue;
        }

        // 転送失敗（宛先停止中・受信箱飽和）は、このエージェント自身の失敗ではない。
        // 自分を Failed に落とさず、事象として通知するに留める。
        if let Err(err) = deliver(shared, target, outgoing).await {
            shared.emit(CoreEvent::AgentFailed {
                agent_id: target.clone(),
                error: ErrorPayload::from(&err),
            });
        }
    }

    Ok(())
}
