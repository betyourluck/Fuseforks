//! Concordia GUI クレート。
//!
//! ここは **agent-core の薄い外殻**である。ウィンドウの起動、IPC コマンドの登録、
//! コアイベントの中継しか行わない。オーケストレーションの判断はすべて
//! `agent-core` 側にあり、このクレートを外しても中核は単体で動く。

mod commands;
mod state;

use tauri::Manager;

/// アプリケーションを起動する。
///
/// `setup` の中でオーケストレーターを組み立てるのは、ワークスペースのパス解決に
/// `AppHandle` が要るため。ここでの失敗はウィンドウを出す前に止める
/// （空の画面を見せてから「実は初期化に失敗していました」と伝えるより誠実）。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let app_state = tauri::async_runtime::block_on(state::build_state(&handle))?;

            state::spawn_event_bridge(handle, std::sync::Arc::clone(&app_state.orchestrator));
            app.manage(app_state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // 参照系
            commands::list_agents,
            commands::list_topology,
            commands::list_messages,
            commands::token_usage,
            commands::list_model_templates,
            commands::list_rag_sources,
            commands::search_rag,
            commands::workspace_path,
            // 資格情報（値を返す経路は存在しない）
            commands::set_model_credential,
            commands::clear_model_credential,
            commands::model_credential_exists,
            // 定義の編集
            commands::create_agent,
            commands::update_agent,
            commands::delete_agent,
            commands::set_connections,
            commands::reorder_agents,
            commands::upsert_model_template,
            commands::delete_model_template,
            // 設定ファイル
            commands::read_agent_config,
            commands::write_agent_config,
            // アイコン
            commands::get_agent_icon,
            commands::set_agent_icon,
            commands::clear_agent_icon,
            // ライフサイクルと配送
            commands::start_agent,
            commands::stop_agent,
            commands::set_agent_running,
            commands::send_user_message,
            commands::index_rag_chunk,
        ])
        .run(tauri::generate_context!())
        .expect("Tauri アプリケーションの起動に失敗しました");
}
