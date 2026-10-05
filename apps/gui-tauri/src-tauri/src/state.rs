//! アプリ状態の組み立てと、コアイベントの Tauri への中継。
//!
//! この層の役割は 2 つだけ:
//! 1. Tauri にしか無い 2 つ（データの置き場と版）を [`build_host`] へ渡して
//!    [`Host`] を起こす
//! 2. `broadcast` で流れてくる [`CoreEvent`] をウィンドウへ転送する
//!
//! ここが **fuseforks-host と Tauri の唯一の接点**であり、ホスト側もコア側も
//! Tauri を知らない。組み立ての配線（同梱ツール・扉・前判定の承認・Jev・単価表の
//! 取得元）は `fuseforks-host` の `boot.rs` が 1 実装で持つ（Spec 64 P1）。

use std::sync::Arc;

use fuseforks_core::{CoreEvent, Orchestrator};
use fuseforks_host::{build_host, Host, HostBootOptions, HostPaths};
use tauri::{AppHandle, Emitter, Manager};

/// フロントエンドが購読するイベント名。
pub const CORE_EVENT: &str = "core://event";

/// Tauri の管理状態。
pub struct AppState {
    /// 開いた村（組み立て済みの部品の束）。IPC はここを読む。
    pub host: Host,
}

/// バックグラウンド初期化の失敗理由。
///
/// [`AppState`] は初期化が**成功するまで manage されない**ため、失敗を運ぶ器が
/// 別に要る。こちらは起動直後（初期化の開始前）に manage しておき、
/// `boot_status` コマンドが「まだか・失敗したか」を常に答えられるようにする。
#[derive(Default)]
pub struct BootError(pub std::sync::Mutex<Option<String>>);

/// アプリ起動時に村を組み立てる。
///
/// **GUI が渡すのは Tauri にしか無いものだけ** — データの置き場（`app_data_dir`）と
/// 版（`package_info`。CI がタグから書き換える側）。残りは `build_host` が決める。
///
/// # Errors
/// データの置き場を解決・作成できない場合、または保存済み `world.json` が壊れている場合。
pub async fn build_state(app: &AppHandle) -> Result<AppState, Box<dyn std::error::Error>> {
    let paths = HostPaths::new(app.path().app_data_dir()?);
    let host = build_host(
        &paths,
        HostBootOptions {
            app_version: app.package_info().version.to_string(),
            // GUI は常に扉を設定どおりに開く（開かないのは `ask` / `check` だけ）。
            open_door: true,
        },
    )
    .await?;
    Ok(AppState { host })
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
