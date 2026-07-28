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
use agent_core::{CoreError, CoreResult, RagChunk};
use tauri::State;

use crate::state::AppState;

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

/// メッセージログを返す。`limit` 指定時は末尾からその件数。
#[tauri::command]
pub async fn list_messages(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> CoreResult<Vec<AgentMessage>> {
    Ok(state.orchestrator.message_log(limit).await)
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
#[tauri::command]
pub async fn send_user_message(
    state: State<'_, AppState>,
    agent_id: AgentId,
    content: String,
) -> CoreResult<()> {
    state
        .orchestrator
        .send_user_message(&agent_id, &content)
        .await
}

/// RAG 索引に断片を追加する（動作確認用の投入口）。
#[tauri::command]
pub async fn index_rag_chunk(state: State<'_, AppState>, chunk: RagChunk) -> CoreResult<()> {
    state.orchestrator.index_rag_chunk(chunk).await;
    Ok(())
}
