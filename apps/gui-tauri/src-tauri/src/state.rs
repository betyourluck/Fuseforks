//! アプリ状態の組み立てと、コアイベントの Tauri への中継。
//!
//! この層の役割は 2 つだけ:
//! 1. [`Orchestrator`] をワークスペースのパスとバックエンドを与えて起こす
//! 2. `broadcast` で流れてくる [`CoreEvent`] をウィンドウへ転送する
//!
//! ここが **agent-core と Tauri の唯一の接点**であり、コア側は Tauri を知らない。

use std::sync::Arc;

use agent_core::{
    ConfigStore, CoreEvent, DiffTool, FdTool, FileTool, GrepTool, HttpBackendFactory,
    KeyringSecretStore, Orchestrator, OrchestratorConfig, RagTool, RememberTool, RunTool, SdTool,
    SecretStore, YqTool,
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

/// バックグラウンド初期化の失敗理由。
///
/// [`AppState`] は初期化が**成功するまで manage されない**ため、失敗を運ぶ器が
/// 別に要る。こちらは起動直後（初期化の開始前）に manage しておき、
/// `boot_status` コマンドが「まだか・失敗したか」を常に答えられるようにする。
#[derive(Default)]
pub struct BootError(pub std::sync::Mutex<Option<String>>);

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

    // 診断ログの出口を開く。**失敗しても起動は止めない** — ログが書けないことは
    // アプリが動かない理由にならず、stderr への出力は残る。
    // 置き場をワークスペース直下にするのは、「フォルダを開く」導線でそのまま
    // 辿り着けるから（不具合の報告時に場所を説明せずに済む）。
    if let Err(err) = agent_core::open_log(&workspace.join("concordia.log")) {
        eprintln!("[concordia] ログファイルを開けませんでした（stderr のみ）: {err}");
    }

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
        .register_tool(Arc::new(RememberTool::new(store.clone())))
        .await;
    orchestrator.register_tool(Arc::new(GrepTool)).await;
    orchestrator.register_tool(Arc::new(FdTool)).await;
    orchestrator.register_tool(Arc::new(DiffTool)).await;
    orchestrator.register_tool(Arc::new(SdTool)).await;
    orchestrator.register_tool(Arc::new(YqTool)).await;
    orchestrator.register_tool(Arc::new(FileTool)).await;

    // 見出し索引（Spec 18）。宣言フォルダは各エージェントの rag_sources から
    // 呼び出しの瞬間に解決される。**宣言が空でも登録しておく** — 提示するかは
    // spec_for が個体ごとに決める（run と同じで、ここで出し分けない）。
    orchestrator.register_tool(Arc::new(RagTool)).await;

    // コマンド実行（Spec 15 rev4）。**ポリシーはエージェント別の
    // `agents/{id}/run.json` に住み、呼び出しの瞬間に読む** — 起動時に
    // 読み込んで保持しない（利用者が手で直したら次のターンから効いてほしい）。
    // **登録が 0 件でも登録しておく。** 提示するかは `spec_for` が個体ごとに
    // 決める（`allow` が空なら自分を落とす）ので、ここで出し分けない。
    orchestrator
        .register_tool(Arc::new(RunTool::new(store.clone())))
        .await;

    // MCP サーバーへ接続する。**失敗してもアプリの起動は止めない。**
    // MCP サーバーは外部コマンドで、未インストール・パス違い・権限で普通に落ちる。
    // そこで起動しなくなるのは筋が悪い（各サーバーの結果は list_mcp_servers で読める）。
    // `mcp.json` 自体が壊れている場合もここで握る — 設定を直す画面へ到達できないと
    // 利用者は詰む。
    if let Err(err) = orchestrator.reload_mcp().await {
        agent_core::note!("MCP の初期接続に失敗しました: {err}");
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
