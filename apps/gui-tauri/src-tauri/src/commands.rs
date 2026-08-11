//! Tauri IPC コマンド。
//!
//! この層は**薄い転送層**に徹する。判断はすべて [`fuseforks_core::Orchestrator`] 側にあり、
//! ここでやるのは引数の受け取りと戻り値の受け渡しだけ。
//! ロジックがこちらへ滲むと、GUI を起動しないと検証できない挙動が生まれる。
//!
//! エラーは [`CoreError`] のまま返す。`Serialize` 実装が
//! `{ code, message, detail, agentId, retryable }` へ落とすので、
//! フロントは `code` で分岐し `message` を表示できる。

use std::collections::HashMap;

use fuseforks_core::model::{
    AgentId, AgentMessage, AgentRole, AgentRoleId, AgentSnapshot, AgentSpec, ConfigFileKind,
    ModelTemplate, ModelTemplateId, TopologyEdge,
};
use fuseforks_core::world::TopologyPosition;
use fuseforks_core::{CoreError, CoreResult};
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
) -> CoreResult<Vec<fuseforks_core::plan::PlanWaveRecord>> {
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

// ---- 役職 (Spec 14) ---------------------------------------------------------

/// 登録済みの役職一覧。
#[tauri::command]
pub async fn list_roles(state: State<'_, AppState>) -> CoreResult<Vec<AgentRole>> {
    Ok(state.orchestrator.list_roles().await)
}

/// 役職を登録または更新する。
///
/// **既存のサーヴァントには何も起きない**（role_contract 凍結 4 — 流し込みの
/// 発火点は新規作成ただ 1 つ）。中身はコピー済みなので、変わるのは表示名を
/// 参照しているバッジと顔ぶれだけ。
#[tauri::command]
pub async fn upsert_role(state: State<'_, AppState>, role: AgentRole) -> CoreResult<()> {
    state.orchestrator.upsert_role(role).await
}

/// 役職を削除する。**参照中でも拒まない**（モデルテンプレートとの決定的な差）。
///
/// 役職はコピー済みなので、消してもサーヴァントの動作は変わらない —
/// バッジと顔ぶれの `[...]` が消えるだけ（role_contract 凍結 5）。
#[tauri::command]
pub async fn delete_role(state: State<'_, AppState>, role_id: AgentRoleId) -> CoreResult<()> {
    state.orchestrator.remove_role(&role_id).await
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

// ---- 外の LLM から依頼を受ける扉（Spec 25） ------------------------------------

/// 扉の現在の状態を返す。
///
/// **合鍵をそのまま返す。** ここは他の秘密（API キー）と扱いが逆で、
/// `model_credential_exists` が値を返さないのに対し、こちらは**画面に出して
/// クライアントの設定へ貼ってもらう**ためにある値。存在を隠す意味が無い。
#[tauri::command]
pub async fn mcp_host_status(
    state: State<'_, AppState>,
) -> CoreResult<crate::mcp_server::McpHostStatus> {
    let last_error = state
        .mcp_server_error
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();
    Ok(state.mcp_server.lock().await.status(last_error))
}

/// 扉の ON / OFF とポートを設定して、その場で反映する。
///
/// **ON にした時点で合鍵を生成する**（`mcp_server_contract` 凍結 3）。
///
/// # Errors
/// 設定ファイルが読めず保存できない場合 [`CoreError::ConfigIo`]。
/// **ポートを掴めなかったのはエラーにしない** — 設定は保存されており、
/// 直すのはポート番号なので、状態の `lastError` として画面へ出す。
#[tauri::command]
pub async fn set_mcp_host(
    state: State<'_, AppState>,
    enabled: bool,
    port: u16,
) -> CoreResult<crate::mcp_server::McpHostStatus> {
    let mut manager = state.mcp_server.lock().await;
    let last_error = manager
        .apply(enabled, port)
        .await
        .map_err(|reason| CoreError::ConfigIo {
            path: crate::mcp_server::CONFIG_FILE.to_owned(),
            source: std::io::Error::other(reason),
        })?;
    if let Ok(mut slot) = state.mcp_server_error.lock() {
        *slot = last_error.clone();
    }
    Ok(manager.status(last_error))
}

/// 合鍵を作り直す（開いていれば新しい鍵で開き直す）。
///
/// # Errors
/// 設定ファイルが読めず保存できない場合 [`CoreError::ConfigIo`]。
#[tauri::command]
pub async fn regenerate_mcp_host_token(
    state: State<'_, AppState>,
) -> CoreResult<crate::mcp_server::McpHostStatus> {
    let mut manager = state.mcp_server.lock().await;
    let last_error = manager
        .regenerate_token()
        .await
        .map_err(|reason| CoreError::ConfigIo {
            path: crate::mcp_server::CONFIG_FILE.to_owned(),
            source: std::io::Error::other(reason),
        })?;
    if let Ok(mut slot) = state.mcp_server_error.lock() {
        *slot = last_error.clone();
    }
    Ok(manager.status(last_error))
}

/// 外部クライアントの呼び名（未設定なら `null` = 名乗りをそのまま使う）。
#[tauri::command]
pub async fn get_external_name(state: State<'_, AppState>) -> CoreResult<Option<String>> {
    Ok(state.orchestrator.external_name().await)
}

/// 外部クライアントの呼び名を設定する。`null` で未設定へ戻す。
///
/// **次のターンの封筒から効く。** 過去の履歴と会話ログは直さない
/// （利用者の呼び名と同じ規律）。
#[tauri::command]
pub async fn set_external_name(
    state: State<'_, AppState>,
    name: Option<String>,
) -> CoreResult<()> {
    state.orchestrator.set_external_name(name.as_deref()).await
}

/// 外部クライアントのアイコン（WebP バイト列）を返す。未設定なら `null`。
#[tauri::command]
pub async fn get_external_icon(state: State<'_, AppState>) -> CoreResult<Option<Vec<u8>>> {
    state.orchestrator.external_icon().await
}

/// 外部クライアントのアイコンを設定する。検証は他のアイコンと同じ述語を通る。
#[tauri::command]
pub async fn set_external_icon(state: State<'_, AppState>, data: Vec<u8>) -> CoreResult<()> {
    state.orchestrator.set_external_icon(&data).await
}

/// 外部クライアントのアイコンを削除する。
#[tauri::command]
pub async fn clear_external_icon(state: State<'_, AppState>) -> CoreResult<()> {
    state.orchestrator.clear_external_icon().await
}

/// 外部からの依頼を受ける窓口（未設定なら `None`）。
#[tauri::command]
pub async fn get_reception(state: State<'_, AppState>) -> CoreResult<Option<AgentId>> {
    Ok(state.orchestrator.reception().await)
}

/// 窓口を差し替える。`None` で未設定へ戻す。
///
/// # Errors
/// 指定したエージェントが未登録の場合 [`CoreError::AgentNotFound`]。
#[tauri::command]
pub async fn set_reception(state: State<'_, AppState>, agent_id: Option<AgentId>) -> CoreResult<()> {
    state.orchestrator.set_reception(agent_id.as_ref()).await
}

// ---- MCP ---------------------------------------------------------------------

/// `mcp.json` の宣言を返す。
#[tauri::command]
pub async fn read_mcp_config(state: State<'_, AppState>) -> CoreResult<fuseforks_core::McpConfig> {
    state.orchestrator.mcp_config().await
}

/// `mcp.json` を書き、その場で接続し直す。
#[tauri::command]
pub async fn write_mcp_config(
    state: State<'_, AppState>,
    config: fuseforks_core::McpConfig,
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
) -> CoreResult<Vec<fuseforks_core::McpServerStatus>> {
    Ok(state.orchestrator.mcp_statuses().await)
}

/// エージェント別 MCP の状態（Spec 02）。停止中は「未接続」が返る。
#[tauri::command]
pub async fn agent_mcp_status(
    state: State<'_, AppState>,
    agent_id: AgentId,
) -> CoreResult<fuseforks_core::AgentMcpStatus> {
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

// ---- 村の黒板 ----------------------------------------------------------------

/// 村の黒板（work_dir の `blackboard/`）の付箋を読む。
///
/// **GUI に「内容を書く」経路は無い**（契約の凍結）。削除だけは在る（下記）。
#[tauri::command]
pub async fn list_blackboard(
    state: State<'_, AppState>,
) -> CoreResult<Vec<fuseforks_core::BlackboardNote>> {
    state.orchestrator.read_blackboard().await
}

/// 付箋を 1 枚**ごみ箱へ移す**（2026-08-12 の UI 追加）。
///
/// **完全削除はしない。** `file` ツールの remove と同じ規律で、ごみ箱が
/// 使えない環境では消さずに失敗を返す。**取り消せるからこそ、画面側で
/// 個別削除に確認を付けていない。**
///
/// `dir` は一覧が返した `BlackboardNote.dir` をそのまま渡す。コア側が
/// 「いまサーヴァントが向いている work_dir のどれか」であることを検査する。
#[tauri::command]
pub async fn delete_blackboard_note(
    dir: String,
    name: String,
    state: State<'_, AppState>,
) -> CoreResult<()> {
    state.orchestrator.delete_blackboard_note(&dir, &name).await
}

/// 付箋を全部ごみ箱へ移す。戻り値は移した枚数。
#[tauri::command]
pub async fn clear_blackboard(state: State<'_, AppState>) -> CoreResult<usize> {
    state.orchestrator.clear_blackboard().await
}

// ---- 村の設定（Spec 13） -------------------------------------------------------

/// トークン予算の天井（実効トークン建て）。`null` = 天井なし。
#[tauri::command]
pub async fn get_token_budget(state: State<'_, AppState>) -> CoreResult<Option<u64>> {
    Ok(state.orchestrator.token_budget().await)
}

/// トークン予算の天井を差し替える。メモリの `World` を変えてから `world.json` へ
/// 書き戻すので、**次の依頼から効く**（settings_contract の即時反映）。
/// `0` は `INVALID_TOKEN_BUDGET` で拒否（UI 側の入力検査との二重化）。
#[tauri::command]
pub async fn set_token_budget(
    state: State<'_, AppState>,
    ceiling: Option<u64>,
) -> CoreResult<()> {
    state.orchestrator.set_token_budget(ceiling).await
}

/// UI の表示言語（`"ja"` / `"en"`）。bootstrap が初回に OS から確定済み。
#[tauri::command]
pub async fn get_language(state: State<'_, AppState>) -> CoreResult<fuseforks_core::world::Language> {
    Ok(state.orchestrator.language().await)
}

/// UI の表示言語を差し替える。未知の値は serde の段階で弾かれる。
/// コアは言語で分岐しないため、システムプロンプトは変わらない。
#[tauri::command]
pub async fn set_language(
    state: State<'_, AppState>,
    language: fuseforks_core::world::Language,
) -> CoreResult<()> {
    state.orchestrator.set_language(language).await
}

// ---- コマンドの承認（Spec 20） ------------------------------------------------

/// 全サーヴァントの判断待ち要求。読めなかった個体は `broken: true` で返る。
#[tauri::command]
pub async fn list_command_requests(
    state: State<'_, AppState>,
) -> CoreResult<Vec<fuseforks_core::CommandPolicyView>> {
    Ok(state.orchestrator.command_policies().await)
}

/// 判断待ちの 1 件を承認して `allow` へ入れる。
///
/// **粒度は `open` だけで指定する。** パターン文字列を受け取らないのは、
/// 受け取ると「粒度は機械が決めない」が**「粒度を GUI が何でも決められる」へ
/// 反転する**ため（`*` 1 文字も送れてしまう）。
#[tauri::command]
pub async fn approve_command(
    state: State<'_, AppState>,
    agent_id: AgentId,
    command: String,
    args: Vec<String>,
    open: bool,
) -> CoreResult<fuseforks_core::ApprovalOutcome> {
    state
        .orchestrator
        .approve_command(&agent_id, &command, &args, open)
        .await
}

/// 判断待ちの 1 件を却下して `deny` へ入れる。
#[tauri::command]
pub async fn reject_command(
    state: State<'_, AppState>,
    agent_id: AgentId,
    command: String,
    args: Vec<String>,
    open: bool,
) -> CoreResult<fuseforks_core::ApprovalOutcome> {
    state
        .orchestrator
        .reject_command(&agent_id, &command, &args, open)
        .await
}
// ---- 利用者（Spec 19） --------------------------------------------------------

/// 利用者の呼び名を返す。未設定なら `null`。
///
/// **未設定を既定値へ倒さない** — 画面が「まだ設定していない」ことを示せるように
/// するため（`language` と違い、未設定が正常な状態）。
#[tauri::command]
pub async fn get_user_name(state: State<'_, AppState>) -> CoreResult<Option<String>> {
    Ok(state.orchestrator.user_name().await)
}

/// 利用者の呼び名を設定する。`null` で既定（「ユーザー」）へ戻す。
///
/// 次のターンの封筒から効く。**過去の履歴と会話ログは直さない**
/// （`user_identity_contract` 凍結 8）。
#[tauri::command]
pub async fn set_user_name(
    state: State<'_, AppState>,
    name: Option<String>,
) -> CoreResult<()> {
    state.orchestrator.set_user_name(name.as_deref()).await
}

/// 利用者のアイコン（WebP バイト列）を返す。未設定なら `null`。
#[tauri::command]
pub async fn get_user_icon(state: State<'_, AppState>) -> CoreResult<Option<Vec<u8>>> {
    state.orchestrator.user_icon().await
}

/// 利用者のアイコンを設定する。検証はエージェントのアイコンと同じ述語を通る。
#[tauri::command]
pub async fn set_user_icon(state: State<'_, AppState>, data: Vec<u8>) -> CoreResult<()> {
    state.orchestrator.set_user_icon(&data).await
}

/// 利用者のアイコンを削除する。
#[tauri::command]
pub async fn clear_user_icon(state: State<'_, AppState>) -> CoreResult<()> {
    state.orchestrator.clear_user_icon().await
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

/// 飛行中のターンを協調的に打ち切る（Spec 10）。
///
/// 切るのはターンであってエージェントではない — 稼働は降ろさず、会話も
/// 履歴も残る。飛行中のターンが無ければ何もしない（成功）。
#[tauri::command]
pub async fn interrupt_turn(state: State<'_, AppState>, agent_id: AgentId) -> CoreResult<()> {
    state.orchestrator.interrupt_turn(&agent_id).await;
    Ok(())
}

/// 村の飛行中ターンを全部打ち切る（Spec 10）。冪等 — 飛行中が 0 でも成功。
#[tauri::command]
pub async fn interrupt_all(state: State<'_, AppState>) -> CoreResult<()> {
    state.orchestrator.interrupt_all().await;
    Ok(())
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
    attachments: Option<Vec<AttachmentPayload>>,
) -> CoreResult<()> {
    // 添付は base64 で届く（Spec 23）。復号はここで 1 回、検証と保存はコアが行う。
    let mut uploads = Vec::new();
    for payload in attachments.unwrap_or_default() {
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&payload.data_base64)
            .map_err(|_| fuseforks_core::CoreError::InvalidAttachment {
                reason: "画像データを読み取れません（base64 の復号に失敗）".to_owned(),
            })?;
        uploads.push(fuseforks_core::AttachmentUpload {
            file_name: payload.file_name,
            bytes,
        });
    }
    state
        .orchestrator
        .send_user_message_with_attachments(
            &agent_id,
            &content,
            co_recipients.as_deref().unwrap_or(&[]),
            uploads,
        )
        .await
}

/// 入力欄のパス補完に渡すファイル一覧（Spec 24）。
///
/// 作業フォルダ起点の相対パス（`/` 区切り・ソート済み）と、上限で打ち切ったか。
/// **候補の種別は載せない** — 将来サーヴァントの `@` 言及を足すときも、
/// その候補は `AgentSnapshot` からフロントのローカル状態だけで組めるので、
/// この IPC には `"file"` 以外が永久に入らない（Spec 24 D2）。
#[tauri::command]
pub async fn list_work_dir_files(
    state: State<'_, AppState>,
    agent_id: AgentId,
) -> CoreResult<fuseforks_core::WorkDirListing> {
    state.orchestrator.list_work_dir_files(&agent_id).await
}

/// 添付画像の実体（WebP バイト列）を返す（Spec 23。表示用）。
/// `null` は「保持期間を過ぎて削除された」— UI はプレースホルダの枠を出す。
#[tauri::command]
pub async fn read_attachment(
    state: State<'_, AppState>,
    id: String,
) -> CoreResult<Option<Vec<u8>>> {
    state.orchestrator.read_attachment(&id).await
}

/// IPC で届く添付画像 1 枚（Spec 23）。
///
/// UI 層が WebP へ変換・縮小した後のデータで、コア側の
/// `AttachmentStore::save` がもう一度検証する（IPC から届くバイト列を
/// 検証なしで書かない）。
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentPayload {
    /// 元ファイル名（表示用）。
    pub file_name: String,
    /// WebP データの base64。
    pub data_base64: String,
}

/// 会話をリセットする（新規チャット）。消えるのは会話ログと履歴だけで、
/// 稼働状態・統計・Memory.md・個別 MCP 接続は残る。
///
/// **Spec 12 で意味が変わった**: 今の会話は捨てられず、閉じて新しい会話が開く
/// （前の会話はディスクに残り、一覧から戻れる）。飛行中のターンがあると
/// `SESSION_SWITCH_BLOCKED` で失敗する — 答えが別の会話へ着地するのを防ぐため。
#[tauri::command]
pub async fn reset_conversation(state: State<'_, AppState>) -> CoreResult<()> {
    state.orchestrator.reset_conversation().await
}

// ---- 会話（セッション。Spec 12） ---------------------------------------------

/// 保存されている会話の一覧（`updatedAt` の新しい順）。
#[tauri::command]
pub async fn list_sessions(
    state: State<'_, AppState>,
) -> CoreResult<Vec<fuseforks_core::SessionSummary>> {
    state.orchestrator.list_sessions().await
}

/// いま開いている会話の ID。保存先が開けていない村では空文字。
#[tauri::command]
pub async fn current_session(state: State<'_, AppState>) -> CoreResult<String> {
    Ok(state.orchestrator.current_session())
}

/// 保存されている会話を開き直す。
///
/// 飛行中のターンがあると `SESSION_SWITCH_BLOCKED` で失敗する —
/// 答えが別の会話へ着地するのを防ぐため。
#[tauri::command]
pub async fn resume_session(state: State<'_, AppState>, session_id: String) -> CoreResult<()> {
    state.orchestrator.resume_session(&session_id).await
}

/// 分岐できる地点（その会話のユーザー発話）を古い順で返す。
#[tauri::command]
pub async fn list_fork_points(
    state: State<'_, AppState>,
    session_id: String,
) -> CoreResult<Vec<fuseforks_core::ForkPoint>> {
    state.orchestrator.list_fork_points(&session_id).await
}

/// 会話を `at_seq` **まで含めて**複製し、複製した側を開く。元は不変のまま残る。
#[tauri::command]
pub async fn fork_session(
    state: State<'_, AppState>,
    session_id: String,
    at_seq: u64,
) -> CoreResult<String> {
    state.orchestrator.fork_session(&session_id, at_seq).await
}

/// 会話を消す。開いている会話を消した場合は次の会話へ切り替わる。
#[tauri::command]
pub async fn delete_session(state: State<'_, AppState>, session_id: String) -> CoreResult<()> {
    state.orchestrator.delete_session(&session_id).await
}

/// いまの会話を要約して続ける（Spec 12 P4）。要約できたサーヴァント数を返す。
///
/// **人が押したときだけ走る。** 自動では要約しない — 要約は LLM 呼び出し
/// = トークンで、`token_budget` の天井と競合する。
#[tauri::command]
pub async fn summarize_session(state: State<'_, AppState>) -> CoreResult<usize> {
    state.orchestrator.summarize_session().await
}

/// 会話を JSONL で書き出し、**書き出し先のパス**を返す。
///
/// 保存先（`sessions.redb`）はバイナリなので、人が読める出口が無いと診断が
/// grep できなくなる。書き出し先はワークスペース配下の `exports/` に固定する —
/// ファイル選択ダイアログのために plugin を足すより、置き場所を 1 つ決めて
/// パスを画面に出すほうが、依存も操作も少ない。
#[tauri::command]
pub async fn export_session(
    state: State<'_, AppState>,
    session_id: String,
) -> CoreResult<String> {
    let dir = state.workspace.join("exports");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|err| CoreError::ConfigIo {
            path: "exports".to_owned(),
            source: err,
        })?;
    let dest = dir.join(format!("{session_id}.jsonl"));
    state.orchestrator.export_session(&session_id, &dest).await?;
    Ok(dest.display().to_string())
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
    pub task: fuseforks_core::schedule::ScheduledTask,
    /// 次回の発火予定時刻（epoch ミリ秒）。求まらない場合は `null`。
    pub next_due_ms: Option<u64>,
    /// 再現規則の日本語表記（「毎週 木曜 17:00」）。配送本文の由来と同じ関数。
    pub recurrence_label: String,
    /// 前判定がこの端末で承認済みか（Spec 28）。**前判定が無ければ常に真**。
    ///
    /// 偽の行は発火しても `unapproved` で消化される。**画面がその理由を出せる
    /// ようにここへ載せる** — 出さないと「動かないが理由が分からない」になる。
    pub probe_approved: bool,
    /// 直近 1 回の判定の結末（Spec 28 D8）。まだ 1 度も走っていなければ `null`。
    ///
    /// **プロセス寿命**（再起動で消える）。再起動後の診断は `fuseforks.log` の
    /// `schedule probe:` 行が担う — 第 2 の永続ファイルは作らない。
    pub last_probe: Option<fuseforks_core::schedule_probe::ProbeReport>,
}

impl ScheduleView {
    /// 現在時刻で `next_due` を評価して 1 行に整える。
    fn of(
        task: fuseforks_core::schedule::ScheduledTask,
        approvals: &crate::probe_approvals::ApprovalStore,
        village_id: &str,
        reports: &std::collections::HashMap<String, fuseforks_core::schedule_probe::ProbeReport>,
    ) -> Self {
        let last_probe = reports.get(&task.id).cloned();
        let next_due_ms = task
            .next_due(&chrono::Local::now())
            .and_then(|due| u64::try_from(due.timestamp_millis()).ok());
        let recurrence_label = task.recurrence.label_ja();
        let probe_approved = task.probe.as_ref().is_none_or(|probe| {
            fuseforks_core::orchestrator::ProbeApprovals::is_approved(
                approvals,
                &probe.approval_key(village_id),
            )
        });
        Self {
            task,
            next_due_ms,
            recurrence_label,
            probe_approved,
            last_probe,
        }
    }
}

/// 登録済みの予定（登録順）。
#[tauri::command]
pub async fn list_schedules(state: State<'_, AppState>) -> CoreResult<Vec<ScheduleView>> {
    let village_id = state.orchestrator.village_id().await;
    let reports = state.orchestrator.probe_reports().await;
    Ok(state
        .orchestrator
        .schedules()
        .await
        .into_iter()
        .map(|task| ScheduleView::of(task, &state.probe_approvals, &village_id, &reports))
        .collect())
}

/// 予定を登録する。
///
/// **前判定を伴う登録は、ここで承認も書く**（Spec 28 D10）。書いた人 =
/// 承認した人なので、追加の確認ダイアログは出さない — 自分がいま書いた
/// コマンドへの同意をもう一度問うのは、同じ意図の 2 つ目のスイッチになる。
///
/// **順序は 承認 → schedules。** 逆にすると、schedules だけ書けて承認が
/// 書けなかったとき、正当な前判定が `unapproved` で 1 回ぶんの発火を失う。
/// 承認だけ書けた残骸は**何も参照しない鍵**なので無害で、次の保存の掃除が拾う。
#[tauri::command]
pub async fn create_schedule(
    state: State<'_, AppState>,
    to: AgentId,
    message: String,
    recurrence: fuseforks_core::schedule::Recurrence,
    options: Option<fuseforks_core::schedule::ScheduleOptions>,
) -> CoreResult<ScheduleView> {
    let options = options.unwrap_or_default();
    let village_id = state.orchestrator.village_id().await;

    if let Some(probe) = &options.probe {
        // **人のクリックだけがここへ到達する。** モデルは IPC を呼べないので、
        // `schedules.json` を `file write` で書いても承認は付かない。
        state
            .probe_approvals
            .approve(probe.approval_key(&village_id))
            .map_err(|reason| fuseforks_core::CoreError::ConfigIo {
                path: crate::probe_approvals::APPROVALS_FILE.to_owned(),
                source: std::io::Error::other(reason),
            })?;
    }

    let task = state
        .orchestrator
        .create_schedule(to, message, recurrence, options)
        .await?;

    // 使われなくなった承認を落とす（**時計ではなく参照で**肥大化を止める）。
    // 失敗しても登録は成立しているので、警告に留める。
    if let Err(reason) = state
        .probe_approvals
        .retain_for(&state.orchestrator.schedules().await, &village_id)
    {
        fuseforks_core::note!("WARN probe approvals: 掃除に失敗しました: {reason}");
    }

    let reports = state.orchestrator.probe_reports().await;
    Ok(ScheduleView::of(
        task,
        &state.probe_approvals,
        &village_id,
        &reports,
    ))
}

/// 既存の予定の前判定を、この端末で実行してよいと承認する（Spec 28 D10）。
///
/// **配られた村・手で書いた `schedules.json` の前判定はここを通るまで走らない。**
/// 画面がコマンド行の原文（`command` / `args` / `cwd`）を出したうえで、
/// 人が中身を見て押す。
///
/// # Errors
/// 該当 ID が無い、前判定を持たない、または承認ファイルへ書けない場合。
#[tauri::command]
pub async fn approve_schedule_probe(state: State<'_, AppState>, id: String) -> CoreResult<()> {
    let village_id = state.orchestrator.village_id().await;
    let tasks = state.orchestrator.schedules().await;
    let task = tasks
        .iter()
        .find(|task| task.id == id)
        .ok_or_else(|| fuseforks_core::CoreError::ScheduleNotFound(id.clone()))?;
    let probe = task
        .probe
        .as_ref()
        .ok_or_else(|| fuseforks_core::CoreError::InvalidSchedule {
            reason: "この予定は前判定を持ちません".to_owned(),
        })?;

    state
        .probe_approvals
        .approve(probe.approval_key(&village_id))
        .map_err(|reason| fuseforks_core::CoreError::ConfigIo {
            path: crate::probe_approvals::APPROVALS_FILE.to_owned(),
            source: std::io::Error::other(reason),
        })
}

/// 予定を削除する。復元はできない。
///
/// **前判定の承認も一緒に落とす。** ここが承認を取り消す唯一の経路で、
/// 残すと「予定を消したのに端末には承認が残る」状態になる — そのあと
/// **GUI を通らない経路**（配られた村・手書きの `schedules.json`）で同じ
/// `command` / `args` / `cwd` が入ってくると、人が押していないのに走る。
///
/// **順序は schedules → 承認。** [`create_schedule`] と逆なのは、守る向きが
/// 逆だから — あちらは「承認だけ書けた残骸は無害」なので承認を先に書く。
/// こちらは**予定だけ消えて承認が残るほうが危険**なので、予定を消してから掃除する。
#[tauri::command]
pub async fn delete_schedule(state: State<'_, AppState>, id: String) -> CoreResult<()> {
    state.orchestrator.delete_schedule(&id).await?;

    // 掃除に失敗しても削除は成立しているので、警告に留める（create 側と同じ）。
    let village_id = state.orchestrator.village_id().await;
    if let Err(reason) = state
        .probe_approvals
        .retain_for(&state.orchestrator.schedules().await, &village_id)
    {
        fuseforks_core::note!("WARN probe approvals: 掃除に失敗しました: {reason}");
    }
    Ok(())
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
