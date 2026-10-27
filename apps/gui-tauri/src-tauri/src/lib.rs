//! Fuseforks GUI クレート。
//!
//! ここは **fuseforks-core の薄い外殻**である。ウィンドウの起動、IPC コマンドの登録、
//! コアイベントの中継しか行わない。オーケストレーションの判断はすべて
//! `fuseforks-core` 側にあり、このクレートを外しても中核は単体で動く。

mod autostart;
mod commands;
// ホスト層（Spec 64）へ持ち上げた扉と棚。`crate::mcp_server::…` の読み替えを
// 起こさないために同じ名前で再公開する。
pub use fuseforks_host::jev_settings;
pub use fuseforks_host::mcp_server;
pub use fuseforks_host::pricing_source;
pub use fuseforks_host::probe_approvals;
mod launch;
mod startup;
mod state;
mod tray;

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
/// **例外は 1 つ（Spec 69 D0）** — サインイン時の自動起動（`--autostart`）かつ
/// 「閉じるとトレイに隠れる」が ON のときだけ、窓を見えない状態で作る。組み立てに
/// 失敗したら必ず窓を出して覆いの理由を見せ（窓もトレイも無いプロセスを残さない）、
/// 成功したらトレイを作って隠したまま動く。
///
/// [`state::AppState`] は初期化が成功した瞬間に manage される。それまでの
/// 失敗理由は [`state::BootError`]（起動直後に manage 済み）へ入るので、
/// `boot_status` は常に「まだ・できた・失敗した」のどれかを答えられる。
/// 2 つ目の起動は [`tauri_plugin_single_instance`] が殺す（Spec 07 P0）。
/// 同じワークスペースを掴んだプロセスが 2 つ並ぶと、予定が両方で発火し、
/// `lastConsumedDueMs` を競って書くため消化の記録が壊れる。
///
/// **プラグインは登録順に走るので、これを最初に登録する**（プラグインの要件）。
/// **村そのものの排他は `build_host` の村のロック（Spec 64 D3）が持つ** — OS の
/// ファイルロックなので強制終了後の残留は無く、GUI と `fuseforks-cli` の間でも効く。
/// プラグインの役目は「2 つ目の GUI を前面化して閉じる」（利用者への見せ方）で、
/// ロックの役目は「村を 2 重に開かない」（データの安全）。役目が違うので両方残す。
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
            // **トレイに隠した窓もこれで戻る**（Spec 69 P0 実測）。
            tray::show_main(app);
        }));
    }

    let app = builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            app.manage(state::BootError::default());

            // 起動と終了の端末側の設定（Spec 69 D0 の 1）。小さいので同期で読む。
            let store = startup::StartupStore::load(&app.path().app_data_dir()?);
            let autostart = startup::launched_by_autostart(std::env::args_os());
            let hidden = startup::starts_hidden(store.settings(), autostart);
            app.manage(startup::StartupState {
                store: std::sync::Mutex::new(store),
            });

            // 窓は設定ファイル（`create: false`）から作る（D0 の 2・D5 の案 A）。
            // 見える側は作った瞬間から見え、隠す側は 1 度も見えない。
            let config = app
                .config()
                .app
                .windows
                .first()
                .cloned()
                .ok_or("tauri.conf.json に窓の設定がありません")?;
            tauri::WebviewWindowBuilder::from_config(app.handle(), &config)?
                .visible(!hidden)
                .build()?;

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let outcome = state::build_state(&handle).await;
                // ログは組み立ての中で開くので、出せる時点（結果が出た後）で 1 行。
                fuseforks_core::note!(
                    "launch: source={} hidden={hidden}",
                    if autostart { "autostart" } else { "manual" }
                );
                match outcome {
                    Ok(app_state) => {
                        let orchestrator = std::sync::Arc::clone(&app_state.host.orchestrator);
                        state::spawn_event_bridge(handle.clone(), std::sync::Arc::clone(&orchestrator));
                        handle.manage(app_state);
                        after_boot(&handle, orchestrator, hidden).await;
                    }
                    Err(failure) => {
                        // ここで panic せず理由を残し、フロントの覆いに「初期化に失敗した」と
                        // 表示させる。**隠して作っていたら窓を出す**（D0 の 5）— 窓もトレイも
                        // 無いプロセスが多重起動の枠だけを握って居座る形を作らない。
                        fuseforks_core::note!("初期化に失敗しました: {}", failure.message);
                        let slot = handle.state::<state::BootError>();
                        if let Ok(mut guard) = slot.0.lock() {
                            *guard = Some(failure);
                        }
                        if hidden {
                            tray::show_main(&handle);
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
            commands::get_tool_call,
            commands::token_usage,
            commands::list_model_templates,
            commands::list_roles,
            commands::workspace_path,
            // 外の LLM から依頼を受ける扉（Spec 25）
            commands::pricing_source_status,
            commands::save_pricing_source,
            commands::fetch_model_prices,
            commands::get_jev_settings,
            commands::set_jev_settings,
            commands::set_jev_token,
            commands::clear_jev_token,
            commands::test_jev,
            commands::test_model_template,
            commands::mcp_host_status,
            commands::set_mcp_host,
            commands::regenerate_mcp_host_token,
            commands::get_external_name,
            commands::set_external_name,
            commands::get_external_icon,
            commands::set_external_icon,
            commands::clear_external_icon,
            commands::get_reception,
            commands::set_reception,
            // 資格情報（値を返す経路は存在しない）
            commands::set_model_credential,
            commands::clear_model_credential,
            commands::model_credential_exists,
            commands::list_mcp_secrets,
            commands::set_mcp_secret,
            commands::clear_mcp_secret,
            // 定義の編集
            commands::create_agent,
            commands::update_agent,
            commands::delete_agent,
            commands::set_connections,
            commands::reorder_agents,
            commands::set_topology_position,
            commands::set_topology_positions,
            commands::upsert_model_template,
            commands::delete_model_template,
            commands::upsert_role,
            commands::delete_role,
            // グループ（Spec 51）
            commands::list_groups,
            commands::create_group,
            commands::upsert_group,
            commands::delete_group,
            commands::commit_agent_drop,
            // 判断役（Spec 62）
            commands::list_judges,
            commands::create_judge,
            commands::update_judge,
            commands::delete_judge,
            commands::read_judge_file,
            commands::save_judge_file,
            commands::try_judge,
            commands::assist_draft,
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
            commands::delete_blackboard_note,
            commands::clear_blackboard,
            // 村の設定（Spec 13）
            commands::get_token_budget,
            commands::set_token_budget,
            commands::get_ask_timeout,
            commands::set_ask_timeout,
            commands::get_plan_review_bypass,
            commands::set_plan_review_bypass,
            commands::get_run_approval,
            commands::set_run_approval,
            commands::get_default_verifier,
            commands::set_default_verifier,
            commands::get_language,
            commands::set_language,
            commands::get_user_name,
            commands::set_user_name,
            commands::get_user_icon,
            commands::set_user_icon,
            commands::clear_user_icon,
            // コマンドの承認（Spec 20）
            commands::list_command_requests,
            commands::approve_command,
            commands::reject_command,
            // 承認して続けさせる（Spec 56）
            commands::resume_after_approval,
            // MCP
            commands::read_mcp_config,
            commands::write_mcp_config,
            commands::reload_mcp,
            commands::list_mcp_servers,
            commands::agent_mcp_status,
            commands::mcp_oauth_authorize,
            commands::mcp_oauth_cancel,
            commands::mcp_oauth_revoke,
            // ライフサイクルと配送
            commands::start_agent,
            commands::stop_agent,
            commands::interrupt_turn,
            commands::dispatch_plan_wave,
            commands::discard_plan_wave,
            commands::interrupt_all,
            commands::set_agent_running,
            commands::send_user_message,
            commands::read_attachment,
            commands::list_work_dir_files,
            commands::reset_conversation,
            // 会話（セッション。Spec 12）
            commands::list_sessions,
            commands::current_session,
            commands::resume_session,
            commands::list_fork_points,
            commands::fork_session,
            commands::delete_session,
            commands::export_session,
            commands::session_stats,
            commands::summarize_session,
            // 予定（Spec 07）
            commands::list_schedules,
            commands::create_schedule,
            commands::update_schedule,
            commands::approve_schedule_probe,
            commands::delete_schedule,
            commands::set_schedule_enabled,
            // 起動と終了（Spec 69）
            commands::get_startup_settings,
            commands::set_startup_settings,
            commands::get_autostart,
            commands::set_autostart,
        ])
        .build(tauri::generate_context!())
        .expect("Tauri アプリケーションの起動に失敗しました");

    app.run(|_app, _event| {
        // macOS: 隠した窓は Dock のアイコンを押しても戻らない（Reopen を受けないと何も起きない）。
        // **窓を隠す分岐はここに置かない** — 隠すのは onCloseRequested の 1 箇所だけ
        // （launch_exit_contract 2）。Cmd+Q は CloseRequested を通らずに終了へ進む。
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = _event {
            tray::show_main(_app);
        }
    });
}

/// 組み立てが成功して `AppState` を manage した後の 2 つ（Spec 69 D0 の 4）。
///
/// トレイを作ってから、一括起動を**別のタスクで**回す（`start_agent` は MCP の接続を待つ）。
/// 設定は**この時点の値**を読む — 組み立ての間に画面から切り替えた値も拾う。
async fn after_boot(
    handle: &tauri::AppHandle,
    orchestrator: std::sync::Arc<fuseforks_core::Orchestrator>,
    hidden: bool,
) {
    let settings = handle
        .state::<startup::StartupState>()
        .store
        .lock()
        .map(|store| store.settings())
        .unwrap_or_default();

    let tray_ready = settings.minimize_to_tray
        && match tray::install(handle, orchestrator.language().await) {
            Ok(()) => true,
            Err(err) => {
                fuseforks_core::note!("トレイを作れませんでした: {err}");
                false
            }
        };
    // **隠しているのにトレイが無い形を作らない** — 戻す手段がもう一度起動することしか残らない。
    // トレイを作れなかったときと、組み立ての間に設定が切られたときの両方をここで受ける。
    if hidden && !tray_ready {
        tray::show_main(handle);
    }

    if settings.batch_start_on_launch {
        tauri::async_runtime::spawn(launch::run(orchestrator));
    }
}
