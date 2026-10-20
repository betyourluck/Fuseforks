//! トレイ（Spec 69 D2。`launch_exit_contract` 2〜4）。
//!
//! **アイコンは `minimizeToTray` が真の間だけ出す**（裁定 1）。作るのは組み立てが成功した後 —
//! メニューの文言は村の言語で出し、言語が決まっているのは組み立ての後だから。組み立てに失敗した
//! プロセスにはトレイを作らない（窓もトレイも無いプロセスを残さない）。
//!
//! メニューは「Fuseforks を表示」「終了」の 2 項目。**「終了」は窓を出してから画面へ
//! [`QUIT_EVENT`] を送るだけ** — 閉じる確認は画面（`App.vue`）が持っているので、確認を出すか・
//! 出したまま 2 回目を積まないかは画面が決める（`launch_exit_contract` 4）。

use fuseforks_core::world::Language;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager};

/// トレイの id。1 つしか作らない。
pub const TRAY_ID: &str = "main";

/// トレイの「終了」が押されたことを画面へ知らせるイベント名。
pub const QUIT_EVENT: &str = "tray://quit-requested";

const SHOW_ID: &str = "show";
const QUIT_ID: &str = "quit";

/// メニューの 2 語（表示 / 終了）。**画面の辞書は Rust から読めないので、ここに ja / en を持つ。**
pub fn labels(language: Language) -> (&'static str, &'static str) {
    match language {
        Language::Ja => ("Fuseforks を表示", "終了"),
        Language::En => ("Show Fuseforks", "Quit"),
    }
}

fn menu(app: &AppHandle, language: Language) -> tauri::Result<Menu<tauri::Wry>> {
    let (show, quit) = labels(language);
    let show = MenuItem::with_id(app, SHOW_ID, show, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, QUIT_ID, quit, true, None::<&str>)?;
    Menu::with_items(app, &[&show, &quit])
}

/// 窓を前面へ出す（多重起動の受け口・トレイ・Dock の 3 つが同じ手順を使う）。
///
/// 3 つとも失敗を握り潰す — 最小化の解除が効かない環境でも前面化は試す価値がある。
pub fn show_main(app: &AppHandle) {
    let Some(window) = app
        .get_webview_window("main")
        .or_else(|| app.webview_windows().into_values().next())
    else {
        fuseforks_core::note!("前面に出すウィンドウがありません");
        return;
    };
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

/// トレイを作る。**既にあれば何もしない**（設定の切り替えと起動の 2 経路から呼ばれる）。
///
/// # Errors
/// メニューかアイコンを作れなかった場合。
pub fn install(app: &AppHandle, language: Language) -> tauri::Result<()> {
    if app.tray_by_id(TRAY_ID).is_some() {
        return Ok(());
    }
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Fuseforks")
        .menu(&menu(app, language)?)
        // macOS はクリックでメニューを開く（メニューバーの作法）。Windows / Linux は左クリックで表示。
        .show_menu_on_left_click(cfg!(target_os = "macos"))
        .on_menu_event(|app, event| match event.id().as_ref() {
            SHOW_ID => show_main(app),
            QUIT_ID => {
                show_main(app);
                let _ = app.emit(QUIT_EVENT, ());
            }
            _ => {}
        });
    #[cfg(not(target_os = "macos"))]
    {
        builder = builder.on_tray_icon_event(|tray, event| {
            use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });
    }
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

/// トレイを消す（`minimizeToTray` を切ったとき）。無ければ何もしない。
pub fn remove(app: &AppHandle) {
    let _ = app.remove_tray_by_id(TRAY_ID);
}

/// 言語を切り替えたらメニューの文言を差し替える。トレイが無ければ何もしない。
///
/// # Errors
/// メニューを作り直せなかった場合。
pub fn relabel(app: &AppHandle, language: Language) -> tauri::Result<()> {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        tray.set_menu(Some(menu(app, language)?))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_words_follow_the_village_language() {
        assert_eq!(labels(Language::Ja), ("Fuseforks を表示", "終了"));
        assert_eq!(labels(Language::En), ("Show Fuseforks", "Quit"));
    }
}
