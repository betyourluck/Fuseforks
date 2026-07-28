//! アプリ状態の組み立てと、コアイベントの Tauri への中継。
//!
//! この層の役割は 2 つだけ:
//! 1. [`Orchestrator`] をワークスペースのパスとバックエンドを与えて起こす
//! 2. `broadcast` で流れてくる [`CoreEvent`] をウィンドウへ転送する
//!
//! ここが **agent-core と Tauri の唯一の接点**であり、コア側は Tauri を知らない。

use std::sync::Arc;

use agent_core::{
    ConfigStore, CoreEvent, DiffTool, GrepTool, HttpBackendFactory, KeyringSecretStore,
    Orchestrator, OrchestratorConfig, RememberTool, SecretStore,
};
use tauri::{AppHandle, Emitter, Manager};

/// フロントエンドが購読するイベント名。
pub const CORE_EVENT: &str = "core://event";

/// Tauri の管理状態。
pub struct AppState {
    /// オーケストレーター本体。
    pub orchestrator: Arc<Orchestrator>,
    /// ワークスペースのルート。「フォルダを開く」導線で使う。
    pub workspace: std::path::PathBuf,
}

/// アプリ起動時にオーケストレーターを組み立てる。
///
/// バックエンドは [`HttpBackendFactory::echo_on_failure`] で構築する。API キーが
/// 未設定でもアプリは動くが、退避したことと理由は `BackendDegraded` イベントと
/// 応答本文の両方に現れる。`strict` にすると、キーを入れるまで画面が沈黙し、
/// 設定不備なのか実装不具合なのか切り分けられなくなる。
///
/// # Errors
/// ワークスペースのディレクトリを解決・作成できない場合、
/// または保存済み `world.json` が壊れている場合。
pub async fn build_state(app: &AppHandle) -> Result<AppState, Box<dyn std::error::Error>> {
    let workspace = app.path().app_data_dir()?.join("workspace");
    tokio::fs::create_dir_all(&workspace).await?;

    // 秘密は OS の資格情報ストアにだけ置く。ワークスペースの `world.json` は
    // 平文で保存されるため、そちらへ秘密が入る経路を持たせない。
    let secrets: Arc<dyn SecretStore> = Arc::new(KeyringSecretStore::new());
    let factory = Arc::new(HttpBackendFactory::echo_on_failure(Arc::clone(&secrets)));

    let store = ConfigStore::new(&workspace);
    let orchestrator = Orchestrator::bootstrap(
        store.clone(),
        factory,
        secrets,
        OrchestratorConfig::default(),
    )
    .await?;

    // 同梱ツール。grep / diff の探索範囲（作業フォルダ）は各エージェントの設定から
    // 実行時に解決されるため、ここでは登録するだけでよい。
    orchestrator
        .register_tool(Arc::new(RememberTool::new(store)))
        .await;
    orchestrator.register_tool(Arc::new(GrepTool)).await;
    orchestrator.register_tool(Arc::new(DiffTool)).await;

    // MCP サーバーへ接続する。**失敗してもアプリの起動は止めない。**
    // MCP サーバーは外部コマンドで、未インストール・パス違い・権限で普通に落ちる。
    // そこで起動しなくなるのは筋が悪い（各サーバーの結果は list_mcp_servers で読める）。
    // `mcp.json` 自体が壊れている場合もここで握る — 設定を直す画面へ到達できないと
    // 利用者は詰む。
    if let Err(err) = orchestrator.reload_mcp().await {
        eprintln!("[concordia] MCP の初期接続に失敗しました: {err}");
    }

    Ok(AppState {
        orchestrator: Arc::new(orchestrator),
        workspace,
    })
}

/// コアイベントをウィンドウへ中継するタスクを起こす。
///
/// `broadcast` の取りこぼし（`Lagged`）では購読を打ち切らない。UI の描画が
/// 一時的に遅れただけで、以後すべてのイベントが届かなくなるほうが害が大きい。
/// 取りこぼした事実は残さないが、後続のイベントで状態は追いつく
/// （スナップショットは常にコア側が真実なので、UI は次の更新で正しくなる）。
pub fn spawn_event_bridge(app: AppHandle, orchestrator: Arc<Orchestrator>) {
    tauri::async_runtime::spawn(async move {
        let mut rx = orchestrator.subscribe();
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let _ = app.emit(CORE_EVENT, &event);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// 型推論のためだけに使う。[`CoreEvent`] が `Serialize` であることをこの層で固定する。
const _: fn() = || {
    fn assert_serialize<T: serde::Serialize>() {}
    assert_serialize::<CoreEvent>();
};
