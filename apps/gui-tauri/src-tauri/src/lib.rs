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
/// オーケストレーターの組み立ては `setup` から**バックグラウンドへ逃がす**。
/// 以前はここで `block_on` していたが、初期化には MCP サーバーの接続
/// （外部コマンドの子プロセス起動）が含まれ、10 秒を超えることがある。
/// その間ウィンドウが 1 枚も出ないと、起動したのか失敗したのか区別できない。
/// ウィンドウを先に出し、フロントは `boot_status` を訊きながら
/// 「初期起動中…」の覆いを見せる。
///
/// [`state::AppState`] は初期化が成功した瞬間に manage される。それまでの
/// 失敗理由は [`state::BootError`]（起動直後に manage 済み）へ入るので、
/// `boot_status` は常に「まだ・できた・失敗した」のどれかを答えられる。
/// 2 つ目の起動は [`tauri_plugin_single_instance`] が殺す（Spec 07 P0）。
/// 同じワークスペースを掴んだプロセスが 2 つ並ぶと、予定が両方で発火し、
/// `lastConsumedDueMs` を競って書くため消化の記録が壊れる。
///
/// **プラグインは登録順に走るので、これを最初に登録する**（プラグインの要件）。
/// コア側には何も足さない — プロセスの一意性は OS/フレームワークの層の仕事で、
/// ここでロックファイルを自作すると強制終了後の残留を自分で面倒みることになる。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default();

    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // このコールバックは**生き残っている側**で走る（2 つ目のプロセスは
            // プラグインが既に終了させている）。ここでやるのは「起動しなかった」
            // ことを利用者に伝えることだけ — 無言だと二度押ししたようにしか
            // 見えず、動くはずのものが動かない理由が画面のどこにも無い。
            let Some(window) = app
                .get_webview_window("main")
                .or_else(|| app.webview_windows().into_values().next())
            else {
                agent_core::note!("2 つ目の起動を止めたが、前面に出すウィンドウが無い");
                return;
            };

            // 3 つとも失敗を握り潰す。最小化の解除が効かない環境でも
            // 前面化は試す価値があり、ここで早期 return すると
            // 「一部の環境でだけ無反応」という一番読めない形になる。
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
        }));
    }

    builder
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            app.manage(state::BootError::default());

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                match state::build_state(&handle).await {
                    Ok(app_state) => {
                        state::spawn_event_bridge(
                            handle.clone(),
                            std::sync::Arc::clone(&app_state.orchestrator),
                        );
                        handle.manage(app_state);
                    }
                    Err(err) => {
                        // ウィンドウは既に出ている。ここで panic せず理由を残し、
                        // フロントの覆いに「初期化に失敗した」と表示させる。
                        agent_core::note!("初期化に失敗しました: {err}");
                        let slot = handle.state::<state::BootError>();
                        if let Ok(mut guard) = slot.0.lock() {
                            *guard = Some(err.to_string());
                        }
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // 起動ハンドシェイク
            commands::boot_status,
            // 参照系
            commands::list_agents,
            commands::list_topology,
            commands::list_topology_positions,
            commands::list_messages,
            commands::list_plan_waves,
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
            commands::set_topology_position,
            commands::upsert_model_template,
            commands::delete_model_template,
            // 設定ファイル
            commands::read_agent_config,
            commands::write_agent_config,
            // アイコン
            commands::get_agent_icon,
            commands::set_agent_icon,
            commands::clear_agent_icon,
            // 村の条例
            commands::read_ordinance,
            commands::write_ordinance,
            // 村の黒板
            commands::list_blackboard,
            // MCP
            commands::read_mcp_config,
            commands::write_mcp_config,
            commands::reload_mcp,
            commands::list_mcp_servers,
            commands::agent_mcp_status,
            // ライフサイクルと配送
            commands::start_agent,
            commands::stop_agent,
            commands::interrupt_turn,
            commands::interrupt_all,
            commands::set_agent_running,
            commands::send_user_message,
            commands::reset_conversation,
            commands::index_rag_chunk,
            // 会話（セッション。Spec 12）
            commands::list_sessions,
            commands::current_session,
            commands::resume_session,
            commands::list_fork_points,
            commands::fork_session,
            commands::delete_session,
            commands::export_session,
            commands::summarize_session,
            // 予定（Spec 07）
            commands::list_schedules,
            commands::create_schedule,
            commands::delete_schedule,
            commands::set_schedule_enabled,
        ])
        .run(tauri::generate_context!())
        .expect("Tauri アプリケーションの起動に失敗しました");
}
