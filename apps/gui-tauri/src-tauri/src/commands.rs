//! Tauri IPC コマンド。
//!
//! この層は**薄い転送層**に徹する。判断はすべて [`agent_core::Orchestrator`] 側にあり、
//! ここでやるのは引数の受け取りと戻り値の受け渡しだけ。
//! ロジックがこちらへ滲むと、GUI を起動しないと検証できない挙動が生まれる。
//!
//! エラーは [`CoreError`] のまま返す。`Serialize` 実装が
//! `{ code, message, detail, agentId, retryable }` へ落とすので、
//! フロントは `code` で分岐し `message` を表示できる。

use std::collections::HashMap;

use agent_core::model::{
    AgentId, AgentMessage, AgentSnapshot, AgentSpec, ConfigFileKind, ModelTemplate,
    ModelTemplateId, TopologyEdge,
};
use agent_core::world::TopologyPosition;
use agent_core::{CoreError, CoreResult, RagChunk};
use tauri::State;

use crate::state::AppState;

// ---- 起動ハンドシェイク -------------------------------------------------------

/// 起動の進み具合。フロントの「初期起動中…」の覆いが判断に使う。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootStatus {
    /// 初期化が完了し、全コマンドが使える状態か。
    pub ready: bool,
    /// 初期化の失敗理由。`null` なら失敗していない（進行中か完了）。
    pub error: Option<String>,
}

/// 初期化が終わったかを答える。
///
/// **このコマンドだけは `State<AppState>` を要求しない。** 初期化はバックグラウンド
/// で走っており、完了までは `AppState` が manage されていない — その間に
/// `State<AppState>` を引くコマンドを呼ぶと抽出の段階で失敗する。フロントは
/// これで `ready` を確認してから他のコマンドを呼び始める契約。
#[tauri::command]
pub fn boot_status(app: tauri::AppHandle) -> BootStatus {
    use tauri::Manager;
    let ready = app.try_state::<AppState>().is_some();
    let error = app
        .try_state::<crate::state::BootError>()
        .and_then(|slot| slot.0.lock().ok().and_then(|guard| guard.clone()));
    BootStatus { ready, error }
}

// ---- 参照系 -----------------------------------------------------------------

/// 登録済みエージェントを表示順で返す。
#[tauri::command]
pub async fn list_agents(state: State<'_, AppState>) -> CoreResult<Vec<AgentSnapshot>> {
    Ok(state.orchestrator.snapshots().await)
}

/// トポロジーの全辺を返す。
#[tauri::command]
pub async fn list_topology(state: State<'_, AppState>) -> CoreResult<Vec<TopologyEdge>> {
    Ok(state.orchestrator.edges().await)
}

/// 接続マップの保存済みノード座標を返す。
#[tauri::command]
pub async fn list_topology_positions(
    state: State<'_, AppState>,
) -> CoreResult<HashMap<AgentId, TopologyPosition>> {
    Ok(state.orchestrator.topology_positions().await.into_iter().collect())
}

/// メッセージログを返す。`limit` 指定時は末尾からその件数。
#[tauri::command]
pub async fn list_messages(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> CoreResult<Vec<AgentMessage>> {
    Ok(state.orchestrator.message_log(limit).await)
}

/// plan 波の記録を返す（Spec 08 — 波ペイン。古い順・実行中の波も含む）。
#[tauri::command]
pub async fn list_plan_waves(
    state: State<'_, AppState>,
) -> CoreResult<Vec<agent_core::plan::PlanWaveRecord>> {
    Ok(state.orchestrator.list_plan_waves().await)
}

/// エージェント別のトークン消費量を返す（Rayon で集計）。
#[tauri::command]
pub async fn token_usage(state: State<'_, AppState>) -> CoreResult<HashMap<AgentId, u64>> {
    state.orchestrator.token_usage_by_agent().await
}

/// 登録済みモデルテンプレートを返す。
#[tauri::command]
pub async fn list_model_templates(state: State<'_, AppState>) -> CoreResult<Vec<ModelTemplate>> {
    Ok(state.orchestrator.templates().await)
}

/// 登録済み RAG ソース名を返す。
#[tauri::command]
pub async fn list_rag_sources(state: State<'_, AppState>) -> CoreResult<Vec<String>> {
    Ok(state.orchestrator.rag_sources().await)
}

/// RAG を検索する（右ペインのプレビュー用）。
#[tauri::command]
pub async fn search_rag(
    state: State<'_, AppState>,
    sources: Vec<String>,
    query: String,
    top_k: usize,
) -> CoreResult<Vec<RagChunk>> {
    state.orchestrator.search_rag(&sources, &query, top_k).await
}

// ---- 定義の編集 -------------------------------------------------------------

/// エージェントを登録する。
#[tauri::command]
pub async fn create_agent(
    state: State<'_, AppState>,
    spec: AgentSpec,
) -> CoreResult<AgentSnapshot> {
    state.orchestrator.create_agent(spec).await
}

/// エージェント定義を差し替える。
#[tauri::command]
pub async fn update_agent(
    state: State<'_, AppState>,
    spec: AgentSpec,
) -> CoreResult<AgentSnapshot> {
    state.orchestrator.update_agent(spec).await
}

/// エージェントを削除する。稼働中なら停止してから消す。
#[tauri::command]
pub async fn delete_agent(state: State<'_, AppState>, agent_id: AgentId) -> CoreResult<()> {
    state.orchestrator.delete_agent(&agent_id).await
}

/// 接続先を差し替える（グラフ上の辺の付け替え）。
#[tauri::command]
pub async fn set_connections(
    state: State<'_, AppState>,
    agent_id: AgentId,
    targets: Vec<AgentId>,
) -> CoreResult<()> {
    state.orchestrator.set_connections(&agent_id, targets).await
}

/// 左ペインの並び順を確定する。
#[tauri::command]
pub async fn reorder_agents(state: State<'_, AppState>, order: Vec<AgentId>) -> CoreResult<()> {
    state.orchestrator.reorder_agents(&order).await
}

/// 接続マップ上で移動したノードの座標を保存する。
#[tauri::command]
pub async fn set_topology_position(
    state: State<'_, AppState>,
    agent_id: AgentId,
    position: TopologyPosition,
) -> CoreResult<()> {
    state
        .orchestrator
        .set_topology_position(&agent_id, position)
        .await
}

/// モデルテンプレートを登録または更新する。
#[tauri::command]
pub async fn upsert_model_template(
    state: State<'_, AppState>,
    template: ModelTemplate,
) -> CoreResult<()> {
    state.orchestrator.upsert_template(template).await
}

/// モデルテンプレートを削除する。参照中のエージェントが居れば拒否される。
#[tauri::command]
pub async fn delete_model_template(
    state: State<'_, AppState>,
    template_id: ModelTemplateId,
) -> CoreResult<()> {
    state.orchestrator.remove_template(&template_id).await
}

// ---- 設定ファイル -----------------------------------------------------------

/// 設定ファイル（`SKILL.md` など）を読む。未作成なら空文字。
#[tauri::command]
pub async fn read_agent_config(
    state: State<'_, AppState>,
    agent_id: AgentId,
    kind: ConfigFileKind,
) -> CoreResult<String> {
    state.orchestrator.read_config(&agent_id, kind).await
}

/// 設定ファイルを書く。
#[tauri::command]
pub async fn write_agent_config(
    state: State<'_, AppState>,
    agent_id: AgentId,
    kind: ConfigFileKind,
    content: String,
) -> CoreResult<()> {
    state
        .orchestrator
        .write_config(&agent_id, kind, &content)
        .await
}

/// モデルテンプレートの API キーを OS の資格情報ストアへ登録する。
///
/// 併せてテンプレートの取得元を `keyring` に切り替える。
#[tauri::command]
pub async fn set_model_credential(
    state: State<'_, AppState>,
    template_id: ModelTemplateId,
    secret: String,
) -> CoreResult<()> {
    state.orchestrator.set_credential(&template_id, &secret).await
}

/// モデルテンプレートの API キーを資格情報ストアから削除する。
#[tauri::command]
pub async fn clear_model_credential(
    state: State<'_, AppState>,
    template_id: ModelTemplateId,
) -> CoreResult<()> {
    state.orchestrator.clear_credential(&template_id).await
}

/// API キーが登録済みかどうかだけを返す。**値は返さない。**
///
/// 表示のために値を取り出すと、秘密が UI 層のメモリへ載る理由が無いのに載る。
#[tauri::command]
pub async fn model_credential_exists(
    state: State<'_, AppState>,
    template_id: ModelTemplateId,
) -> CoreResult<bool> {
    state.orchestrator.has_credential(&template_id)
}

/// ワークスペースのパスを返す。「フォルダを開く」導線で使う。
#[tauri::command]
pub async fn workspace_path(state: State<'_, AppState>) -> CoreResult<String> {
    Ok(state.workspace.display().to_string())
}

// ---- MCP ---------------------------------------------------------------------

/// `mcp.json` の宣言を返す。
#[tauri::command]
pub async fn read_mcp_config(state: State<'_, AppState>) -> CoreResult<agent_core::McpConfig> {
    state.orchestrator.mcp_config().await
}

/// `mcp.json` を書き、その場で接続し直す。
#[tauri::command]
pub async fn write_mcp_config(
    state: State<'_, AppState>,
    config: agent_core::McpConfig,
) -> CoreResult<()> {
    state.orchestrator.set_mcp_config(&config).await
}

/// MCP サーバーへ接続し直す。設定を変えずに再試行したいときに使う。
#[tauri::command]
pub async fn reload_mcp(state: State<'_, AppState>) -> CoreResult<()> {
    state.orchestrator.reload_mcp().await
}

/// 各 MCP サーバーの接続状態。
#[tauri::command]
pub async fn list_mcp_servers(
    state: State<'_, AppState>,
) -> CoreResult<Vec<agent_core::McpServerStatus>> {
    Ok(state.orchestrator.mcp_statuses().await)
}

/// エージェント別 MCP の状態（Spec 02）。停止中は「未接続」が返る。
#[tauri::command]
pub async fn agent_mcp_status(
    state: State<'_, AppState>,
    agent_id: AgentId,
) -> CoreResult<agent_core::AgentMcpStatus> {
    state.orchestrator.agent_mcp_status(&agent_id).await
}

// ---- 村の条例 ----------------------------------------------------------------

/// 村の条例（全エージェント共通の規則）を読む。未設定なら空文字。
#[tauri::command]
pub async fn read_ordinance(state: State<'_, AppState>) -> CoreResult<String> {
    state.orchestrator.read_ordinance().await
}

/// 村の条例を書く。次の発話からすべてのエージェントに反映される。
#[tauri::command]
pub async fn write_ordinance(state: State<'_, AppState>, content: String) -> CoreResult<()> {
    state.orchestrator.write_ordinance(&content).await
}

// ---- アイコン ----------------------------------------------------------------

/// エージェントのアイコン（WebP バイト列）を返す。未設定なら `null`。
#[tauri::command]
pub async fn get_agent_icon(
    state: State<'_, AppState>,
    agent_id: AgentId,
) -> CoreResult<Option<Vec<u8>>> {
    state.orchestrator.agent_icon(&agent_id).await
}

/// エージェントのアイコンを設定する。
///
/// UI 側が png / jpg を **WebP へ変換してから**送る契約。コアは WebP の
/// マジック番号とサイズ上限だけを検証し、通らないバイト列は書かない。
#[tauri::command]
pub async fn set_agent_icon(
    state: State<'_, AppState>,
    agent_id: AgentId,
    data: Vec<u8>,
) -> CoreResult<()> {
    state.orchestrator.set_agent_icon(&agent_id, &data).await
}

/// エージェントのアイコンを削除する。
#[tauri::command]
pub async fn clear_agent_icon(
    state: State<'_, AppState>,
    agent_id: AgentId,
) -> CoreResult<()> {
    state.orchestrator.clear_agent_icon(&agent_id).await
}

// ---- ライフサイクルと配送 ---------------------------------------------------

/// エージェントを起動する。
#[tauri::command]
pub async fn start_agent(state: State<'_, AppState>, agent_id: AgentId) -> CoreResult<()> {
    state.orchestrator.start_agent(&agent_id).await
}

/// エージェントを停止する。
#[tauri::command]
pub async fn stop_agent(state: State<'_, AppState>, agent_id: AgentId) -> CoreResult<()> {
    state.orchestrator.stop_agent(&agent_id).await
}

/// トグルスイッチ 1 つで起動・停止を切り替える。
///
/// 「既に稼働中」「稼働していない」は、トグル操作の文脈では失敗ではなく
/// 望む状態が既に成立しているだけなので握り潰す。それ以外の失敗は伝える。
#[tauri::command]
pub async fn set_agent_running(
    state: State<'_, AppState>,
    agent_id: AgentId,
    running: bool,
) -> CoreResult<AgentSnapshot> {
    let result = if running {
        state.orchestrator.start_agent(&agent_id).await
    } else {
        state.orchestrator.stop_agent(&agent_id).await
    };

    match result {
        Ok(()) => {}
        Err(CoreError::AlreadyRunning { .. }) | Err(CoreError::NotRunning { .. }) => {}
        Err(err) => return Err(err),
    }

    state.orchestrator.snapshot(&agent_id).await
}

/// ユーザー発話をエージェントへ投入する。
///
/// `co_recipients` は同報の全宛先（受信者自身を含む）。同報時に UI が渡すと、
/// 受信者のプロンプトに「全員が既に受け取っている」注記が入り、反響を防ぐ。
/// 省略（単独宛）なら注記は付かない。
#[tauri::command]
pub async fn send_user_message(
    state: State<'_, AppState>,
    agent_id: AgentId,
    content: String,
    co_recipients: Option<Vec<AgentId>>,
) -> CoreResult<()> {
    state
        .orchestrator
        .send_user_message_broadcast(
            &agent_id,
            &content,
            co_recipients.as_deref().unwrap_or(&[]),
        )
        .await
}

/// 会話をリセットする（新規チャット）。消えるのは会話ログと履歴だけで、
/// 稼働状態・統計・Memory.md・個別 MCP 接続は残る。
#[tauri::command]
pub async fn reset_conversation(state: State<'_, AppState>) -> CoreResult<()> {
    state.orchestrator.reset_conversation().await;
    Ok(())
}

/// RAG 索引に断片を追加する（動作確認用の投入口）。
#[tauri::command]
pub async fn index_rag_chunk(state: State<'_, AppState>, chunk: RagChunk) -> CoreResult<()> {
    state.orchestrator.index_rag_chunk(chunk).await;
    Ok(())
}

// ---- 予定（Spec 07） -----------------------------------------------------------

/// UI の一覧 1 行ぶんの予定。
///
/// `next_due_ms` と `recurrence_label` はコア側で算出して同梱する。
/// フロントにカレンダー計算を持たせない（真実が 2 箇所できる）ため、
/// 日本語表記も配送本文と同じ `Recurrence::label_ja` から取る。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleView {
    /// 予定そのもの（camelCase で平坦化）。
    #[serde(flatten)]
    pub task: agent_core::schedule::ScheduledTask,
    /// 次回の発火予定時刻（epoch ミリ秒）。求まらない場合は `null`。
    pub next_due_ms: Option<u64>,
    /// 再現規則の日本語表記（「毎週 木曜 17:00」）。配送本文の由来と同じ関数。
    pub recurrence_label: String,
}

impl ScheduleView {
    /// 現在時刻で `next_due` を評価して 1 行に整える。
    fn of(task: agent_core::schedule::ScheduledTask) -> Self {
        let next_due_ms = task
            .next_due(&chrono::Local::now())
            .and_then(|due| u64::try_from(due.timestamp_millis()).ok());
        let recurrence_label = task.recurrence.label_ja();
        Self {
            task,
            next_due_ms,
            recurrence_label,
        }
    }
}

/// 登録済みの予定（登録順）。
#[tauri::command]
pub async fn list_schedules(state: State<'_, AppState>) -> CoreResult<Vec<ScheduleView>> {
    Ok(state
        .orchestrator
        .schedules()
        .await
        .into_iter()
        .map(ScheduleView::of)
        .collect())
}

/// 予定を登録する。
#[tauri::command]
pub async fn create_schedule(
    state: State<'_, AppState>,
    to: AgentId,
    message: String,
    recurrence: agent_core::schedule::Recurrence,
) -> CoreResult<ScheduleView> {
    let task = state
        .orchestrator
        .create_schedule(to, message, recurrence)
        .await?;
    Ok(ScheduleView::of(task))
}

/// 予定を削除する。復元はできない。
#[tauri::command]
pub async fn delete_schedule(state: State<'_, AppState>, id: String) -> CoreResult<()> {
    state.orchestrator.delete_schedule(&id).await
}

/// 予定の一時停止・再開。
#[tauri::command]
pub async fn set_schedule_enabled(
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> CoreResult<()> {
    state.orchestrator.set_schedule_enabled(&id, enabled).await
}
