//! 起動と終了の端末側の設定（Spec 69 D1）。`{app_data_dir}/startup.json`。
//!
//! **村（`world.json`）に入れない。** 起動時に一括起動を村に入れると、村を配った先で開いただけで
//! 全員が起動する — `batch_start_invariant` が防いでいる事故そのもの。棚は GUI のクレートに置く
//! （`fuseforks-host` は CLI と共有しており、CLI はトレイも一括起動の設定も持たず `--start` で明示する）。
//!
//! **読めない・壊れたファイルは既定（両方 false）として読み、書き込みは拒否しない。**
//! `mcp_server.json` / `jev.json` が「読めないなら書かない」なのは合鍵や Account ID を消すからで、
//! こちらは消えて困る値を持たない。読めなかったことは WARN を 1 行出す。

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// 設定ファイルの名前（`{app_data_dir}/startup.json`）。
pub const CONFIG_FILE: &str = "startup.json";

/// サインイン時の自動起動が渡す引数（Spec 69 D4）。起動時に argv で見分ける。
pub const AUTOSTART_ARG: &str = "--autostart";

/// 端末側の 2 つの設定。**既定は両方 false**（OFF のままなら今と 1 つも変わらない）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupSettings {
    /// 閉じる（✕ / OS の閉じる操作）で窓を隠し、トレイに残るか。
    #[serde(default)]
    pub minimize_to_tray: bool,
    /// 起動したとき、一括起動の対象（全体 ▶ と同じ集合）を 1 回だけ起こすか。
    #[serde(default)]
    pub batch_start_on_launch: bool,
}

/// `startup.json` の読み書き。
#[derive(Debug)]
pub struct StartupStore {
    path: PathBuf,
    settings: StartupSettings,
}

impl StartupStore {
    /// 設定を読む。**ファイルが無いのは失敗ではない**（初回はこれが正常）。
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(CONFIG_FILE);
        let settings = match std::fs::read_to_string(&path) {
            Ok(raw) => match serde_json::from_str::<StartupSettings>(&raw) {
                Ok(settings) => settings,
                Err(err) => {
                    fuseforks_core::note!(
                        "startup: {CONFIG_FILE} を読めませんでした。既定（トレイ OFF・起動時の一括起動 OFF）で続けます（{err}）"
                    );
                    StartupSettings::default()
                }
            },
            Err(_) => StartupSettings::default(),
        };
        Self { path, settings }
    }

    /// 現在の設定。
    pub fn settings(&self) -> StartupSettings {
        self.settings
    }

    /// 設定を差し替えて保存する。**書けなかったときは手元の値も変えない**（画面は押す前の値へ戻す。Spec 69 D7）。
    ///
    /// # Errors
    /// フォルダの作成・一時ファイルの書き込み・置き換えに失敗した場合。
    pub fn save(&mut self, settings: StartupSettings) -> Result<(), String> {
        let raw = serde_json::to_string_pretty(&settings).map_err(|err| err.to_string())?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        // 一時ファイルへ書いてから置き換える（途中で落ちても壊れたファイルを残さない）。
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, raw).map_err(|err| err.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|err| err.to_string())?;
        self.settings = settings;
        Ok(())
    }
}

/// この起動が自動起動か（argv に [`AUTOSTART_ARG`] があるか）。先頭（実行ファイル）は見ない。
pub fn launched_by_autostart<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    args.into_iter()
        .skip(1)
        .any(|arg| arg.as_ref() == OsStr::new(AUTOSTART_ARG))
}

/// 窓を見えない状態で作るか（Spec 69 D0 の 1）。**自動起動かつトレイ ON のときだけ。**
/// トレイが無いのに隠すと、戻す手段がもう一度起動することしか残らない。
pub fn starts_hidden(settings: StartupSettings, autostart: bool) -> bool {
    autostart && settings.minimize_to_tray
}

/// Tauri の管理状態。`setup` の最初に manage する（組み立ての成否に依らず IPC が読める）。
/// 自動起動か・隠して作ったかは `setup` の中で決まり、組み立ての結果を待つタスクへそのまま渡す。
pub struct StartupState {
    /// 設定の棚。
    pub store: Mutex<StartupStore>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fuseforks-startup-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_missing_file_reads_as_both_off() {
        let dir = temp_dir("missing");
        assert_eq!(StartupStore::load(&dir).settings(), StartupSettings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_file_reads_as_both_off_and_can_still_be_saved() {
        let dir = temp_dir("broken");
        std::fs::write(dir.join(CONFIG_FILE), "{ not json").unwrap();
        let mut store = StartupStore::load(&dir);
        assert_eq!(store.settings(), StartupSettings::default());
        // 読めなくても書き込みは拒まない（消えて困る値を持たない）。
        let next = StartupSettings {
            minimize_to_tray: true,
            batch_start_on_launch: false,
        };
        store.save(next).unwrap();
        assert_eq!(StartupStore::load(&dir).settings(), next);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_saved_file_round_trips_with_camel_case_keys() {
        let dir = temp_dir("roundtrip");
        let mut store = StartupStore::load(&dir);
        let next = StartupSettings {
            minimize_to_tray: false,
            batch_start_on_launch: true,
        };
        store.save(next).unwrap();
        let raw = std::fs::read_to_string(dir.join(CONFIG_FILE)).unwrap();
        assert!(raw.contains("\"batchStartOnLaunch\": true"), "{raw}");
        assert!(raw.contains("\"minimizeToTray\": false"), "{raw}");
        assert!(!dir.join("startup.json.tmp").exists());
        assert_eq!(StartupStore::load(&dir).settings(), next);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_key_reads_as_off() {
        let dir = temp_dir("partial");
        std::fs::write(dir.join(CONFIG_FILE), r#"{ "minimizeToTray": true }"#).unwrap();
        let settings = StartupStore::load(&dir).settings();
        assert!(settings.minimize_to_tray);
        assert!(!settings.batch_start_on_launch);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn autostart_is_found_only_after_the_executable() {
        assert!(launched_by_autostart(["fuseforks.exe", "--autostart"]));
        assert!(launched_by_autostart(["fuseforks.exe", "x", "--autostart"]));
        assert!(!launched_by_autostart(["fuseforks.exe"]));
        // 先頭は実行ファイルのパスなので、たまたま同じ字面でも数えない。
        assert!(!launched_by_autostart(["--autostart"]));
        assert!(!launched_by_autostart(["fuseforks.exe", "--autostart=1"]));
    }

    #[test]
    fn the_window_starts_hidden_only_for_autostart_with_the_tray() {
        let tray = StartupSettings {
            minimize_to_tray: true,
            batch_start_on_launch: false,
        };
        assert!(starts_hidden(tray, true));
        assert!(!starts_hidden(tray, false));
        assert!(!starts_hidden(StartupSettings::default(), true));
    }
}
