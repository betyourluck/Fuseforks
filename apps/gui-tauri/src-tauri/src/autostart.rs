//! サインイン時の自動起動（Spec 69 D4。`launch_exit_contract` 6〜10）。
//!
//! **状態はアプリに保存しない。OS の登録そのものを毎回読む** — タスクマネージャーやログイン項目で
//! OS 側から切れるので、写しを持つと真実が 2 つになる。
//!
//! | OS | 書き方 |
//! |---|---|
//! | Windows | `windows-registry` で自前で書く。`HKCU\…\Run` の値 `fuseforks` = `"{exe}" --autostart`。HKLM には書かない |
//! | macOS | `auto-launch` 0.6（`LaunchAgent`）。`~/Library/LaunchAgents/fuseforks.plist` |
//! | Linux | `auto-launch` 0.6（XDG）。`~/.config/autostart/fuseforks.desktop`。`Exec` のパスは引用符で包む |
//!
//! Windows を自前で書くのは利用者の裁定（Spec 69 裁定 4）。値の形が `auto-launch` の内部の
//! `format!` に寄りかからず、読む側（`registeredCommand`・`scope`）も同じ crate で書けるので揃う。

use serde::Serialize;

use crate::startup::AUTOSTART_ARG;

/// 登録の名前。`productName`（小文字）。NSIS のアンインストーラが消す値 `${PRODUCTNAME}` と一致させる。
/// レジストリの値の名前は大文字小文字を区別しない（Spec 69 P0 実測）。
pub const ENTRY_NAME: &str = "fuseforks";

/// 登録がどこにあるか。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AutostartScope {
    /// 今の利用者の登録（Windows は HKCU、macOS / Linux はホームの下）。
    User,
    /// 全利用者の登録（Windows の HKLM）。Fuseforks は書かない — 残っていれば別の経路で入ったもの。
    Machine,
    /// 両方。
    Both,
    /// 登録が無い。
    None,
}

/// 画面へ返す自動起動の状態。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutostartView {
    /// サインインで起動するか（OS 側の無効化まで見た結果）。
    pub enabled: bool,
    /// 登録がどこにあるか。
    pub scope: AutostartScope,
    /// 登録されているコマンドの文字列（無ければ `None`）。画面は今の exe と違えば知らせる。
    pub registered_command: Option<String>,
    /// 今動いている実行ファイルのパス。
    pub current_exe: String,
}

/// 今の自動起動の状態を読む。
///
/// # Errors
/// OS の登録を読めなかった場合（存在しないのは失敗ではない）。
pub fn read() -> Result<AutostartView, String> {
    platform::read()
}

/// 自動起動を入れる / 切る。書いた後の状態を [`read`] と同じ形で返す。
///
/// **Windows で切るとき、HKLM に値が残っていて消せなかった（管理者でない）場合もエラーにしない** —
/// HKCU の分は済んでいるので、全体を失敗にすると済んだ分が画面から読めなくなる。
/// 返り値の `scope` が `machine` になり、画面が直し方を出す（Spec 69 D4）。
///
/// # Errors
/// OS の登録を書けなかった場合。
pub fn set(enabled: bool) -> Result<AutostartView, String> {
    if enabled {
        platform::enable()?;
    } else {
        platform::disable()?;
    }
    platform::read()
}

fn current_exe_string() -> String {
    std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_default()
}

fn scope_of(user: bool, machine: bool) -> AutostartScope {
    match (user, machine) {
        (true, true) => AutostartScope::Both,
        (true, false) => AutostartScope::User,
        (false, true) => AutostartScope::Machine,
        (false, false) => AutostartScope::None,
    }
}

/// Windows の `Run` の値。**パスを引用符で包む**（MSI の入る先 `C:\Program Files\…` は空白を含む）。
#[cfg(any(windows, test))]
fn windows_command(exe: &str) -> String {
    format!("\"{exe}\" {AUTOSTART_ARG}")
}

/// `Explorer\StartupApproved\Run` の値が「無効」を表すか。**先頭バイトの最下位ビット**で見る
/// （有効 `0x02` / `0x06`、無効 `0x03` / `0x07`）。
///
/// `auto-launch` 0.6 は「末尾 8 バイトが全部 0 なら有効」（無効にした時刻の有無）で見ている。
/// 実際のタスクマネージャーの値では 2 つの見方は食い違わない（Spec 69 P4 実測: 無効にすると
/// `03 00 00 00` + 無効にした時刻の FILETIME を書く）。食い違うのは先頭 0x03・時刻 0 の形だけで、
/// タスクマネージャーはその形を作らない。
#[cfg(any(windows, test))]
fn approved_disabled(bytes: &[u8]) -> bool {
    bytes.first().is_some_and(|first| first & 1 == 1)
}

/// Windows の読みの結果から画面の状態を組む（純関数）。
///
/// 有効 = どちらかの `Run` に値があり、かつ両方の `StartupApproved\Run` で無効になっていない
/// （`auto-launch` 0.6 の `is_enabled` と同じ組み合わせ方）。コマンドは HKCU を優先する。
#[cfg(any(windows, test))]
fn windows_view(
    user: Option<String>,
    machine: Option<String>,
    user_disabled: bool,
    machine_disabled: bool,
    current_exe: String,
) -> AutostartView {
    let registered = user.is_some() || machine.is_some();
    AutostartView {
        enabled: registered && !user_disabled && !machine_disabled,
        scope: scope_of(user.is_some(), machine.is_some()),
        registered_command: user.or(machine),
        current_exe,
    }
}

/// macOS の plist（`auto-launch` が書く固定の形）から `ProgramArguments` を 1 本の文字列へ。
#[cfg(any(target_os = "macos", test))]
fn parse_plist_program_arguments(raw: &str) -> Option<String> {
    let after_key = &raw[raw.find("<key>ProgramArguments</key>")?..];
    let array = &after_key[after_key.find("<array>")? + "<array>".len()..];
    let array = &array[..array.find("</array>")?];
    let mut parts = Vec::new();
    let mut rest = array;
    while let Some(open) = rest.find("<string>") {
        let body = &rest[open + "<string>".len()..];
        let close = body.find("</string>")?;
        parts.push(body[..close].to_owned());
        rest = &body[close + "</string>".len()..];
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// Linux の `.desktop`（`auto-launch` が書く固定の形）から `Exec=` の値を取る。
#[cfg(any(target_os = "linux", test))]
fn parse_desktop_exec(raw: &str) -> Option<String> {
    raw.lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("Exec="))
        .map(str::to_owned)
}

#[cfg(windows)]
mod platform {
    use windows_registry::{Key, Type, CURRENT_USER, LOCAL_MACHINE};

    use super::{approved_disabled, current_exe_string, windows_command, windows_view, AutostartView, ENTRY_NAME};

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
    /// `E_FILENOTFOUND`。キーか値が無い。
    const NOT_FOUND: u32 = 0x8007_0002;
    /// `E_ACCESSDENIED`。管理者でない起動で HKLM を書き込みで開いたとき（P0 実測。値の有無に依らない）。
    const ACCESS_DENIED: u32 = 0x8007_0005;
    /// 有効を表す `StartupApproved\Run` の値（`auto-launch` 0.6 と同じ 12 バイト）。
    const ENABLED_BYTES: [u8; 12] = [0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

    fn code(err: &windows_result::Error) -> u32 {
        err.code().0 as u32
    }

    fn read_value(root: &Key) -> Result<Option<String>, String> {
        match root.open(RUN).and_then(|key| key.get_string(ENTRY_NAME)) {
            Ok(value) => Ok(Some(value)),
            Err(err) if code(&err) == NOT_FOUND => Ok(None),
            Err(err) => Err(format!("Run の値を読めませんでした（0x{:08X}）", code(&err))),
        }
    }

    fn disabled_in(root: &Key) -> Result<bool, String> {
        match root.open(APPROVED).and_then(|key| key.get_value(ENTRY_NAME)) {
            Ok(value) => Ok(approved_disabled(&value)),
            Err(err) if code(&err) == NOT_FOUND => Ok(false),
            Err(err) => Err(format!("StartupApproved の値を読めませんでした（0x{:08X}）", code(&err))),
        }
    }

    pub fn read() -> Result<AutostartView, String> {
        Ok(windows_view(
            read_value(CURRENT_USER)?,
            read_value(LOCAL_MACHINE)?,
            disabled_in(CURRENT_USER)?,
            disabled_in(LOCAL_MACHINE)?,
            current_exe_string(),
        ))
    }

    pub fn enable() -> Result<(), String> {
        let command = windows_command(&current_exe_string());
        CURRENT_USER
            .create(RUN)
            .and_then(|key| key.set_string(ENTRY_NAME, &command))
            .map_err(|err| format!("Run へ書けませんでした（0x{:08X}）", code(&err)))?;
        // タスクマネージャーで切られた状態が残っていると、チェックを入れたのに起動しない。
        // キーがあるときだけ有効へ上書きする（`auto-launch` 0.6 と同じ振る舞い）。
        match CURRENT_USER.options().read().write().open(APPROVED) {
            Ok(key) => key
                .set_bytes(ENTRY_NAME, Type::Bytes, &ENABLED_BYTES)
                .map_err(|err| format!("StartupApproved へ書けませんでした（0x{:08X}）", code(&err))),
            Err(err) if code(&err) == NOT_FOUND => Ok(()),
            Err(err) => Err(format!("StartupApproved を開けませんでした（0x{:08X}）", code(&err))),
        }
    }

    pub fn disable() -> Result<(), String> {
        match CURRENT_USER
            .options()
            .read()
            .write()
            .open(RUN)
            .and_then(|key| key.remove_value(ENTRY_NAME))
        {
            Ok(()) => {}
            Err(err) if code(&err) == NOT_FOUND => {}
            Err(err) => return Err(format!("Run の値を消せませんでした（0x{:08X}）", code(&err))),
        }
        // HKLM は**値があるときだけ**消しにいく — 管理者でない起動では、書き込みで開く段で
        // 値の有無に依らず拒否されるので、常に開きにいくと値が無いのに残っていると誤って出す。
        if read_value(LOCAL_MACHINE)?.is_some() {
            match LOCAL_MACHINE
                .options()
                .read()
                .write()
                .open(RUN)
                .and_then(|key| key.remove_value(ENTRY_NAME))
            {
                Ok(()) => fuseforks_core::note!("autostart: 全利用者の登録（HKLM）も消しました"),
                Err(err) if code(&err) == ACCESS_DENIED => fuseforks_core::note!(
                    "autostart: 全利用者の登録（HKLM）は管理者の権限が要るため消せませんでした"
                ),
                Err(err) if code(&err) == NOT_FOUND => {}
                Err(err) => {
                    return Err(format!("HKLM の Run の値を消せませんでした（0x{:08X}）", code(&err)));
                }
            }
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod platform {
    use std::path::PathBuf;

    use auto_launch::{AutoLaunch, AutoLaunchBuilder};

    use super::{current_exe_string, scope_of, AutostartView, AUTOSTART_ARG, ENTRY_NAME};

    /// 登録する実行ファイル。Linux の AppImage は展開先ではなく AppImage そのもの（`APPIMAGE`）。
    /// **Linux は `Exec` に入るので引用符で包む。macOS は plist の 1 要素なので包まない。**
    fn app_path() -> String {
        #[cfg(target_os = "linux")]
        {
            let path = std::env::var_os("APPIMAGE")
                .map(|path| PathBuf::from(path).display().to_string())
                .unwrap_or_else(current_exe_string);
            format!("\"{path}\"")
        }
        #[cfg(not(target_os = "linux"))]
        {
            current_exe_string()
        }
    }

    fn launcher() -> Result<AutoLaunch, String> {
        let mut builder = AutoLaunchBuilder::new();
        builder
            .set_app_name(ENTRY_NAME)
            .set_app_path(&app_path())
            .set_args(&[AUTOSTART_ARG]);
        #[cfg(target_os = "macos")]
        builder.set_macos_launch_mode(auto_launch::MacOSLaunchMode::LaunchAgent);
        builder.build().map_err(|err| err.to_string())
    }

    /// `auto-launch` が書くファイル（読み取りだけに使う）。
    fn entry_file() -> Option<PathBuf> {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        #[cfg(target_os = "macos")]
        let path = home.join("Library").join("LaunchAgents").join(format!("{ENTRY_NAME}.plist"));
        #[cfg(not(target_os = "macos"))]
        let path = home.join(".config").join("autostart").join(format!("{ENTRY_NAME}.desktop"));
        Some(path)
    }

    fn registered_command() -> Option<String> {
        let raw = std::fs::read_to_string(entry_file()?).ok()?;
        #[cfg(target_os = "macos")]
        return super::parse_plist_program_arguments(&raw);
        #[cfg(not(target_os = "macos"))]
        return super::parse_desktop_exec(&raw);
    }

    pub fn read() -> Result<AutostartView, String> {
        let enabled = launcher()?.is_enabled().map_err(|err| err.to_string())?;
        let registered = registered_command();
        Ok(AutostartView {
            enabled,
            scope: scope_of(registered.is_some(), false),
            registered_command: registered,
            current_exe: current_exe_string(),
        })
    }

    pub fn enable() -> Result<(), String> {
        launcher()?.enable().map_err(|err| err.to_string())
    }

    pub fn disable() -> Result<(), String> {
        launcher()?.disable().map_err(|err| err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_windows_value_quotes_the_path_and_adds_the_argument() {
        assert_eq!(
            windows_command(r"C:\Program Files\fuseforks\fuseforks.exe"),
            r#""C:\Program Files\fuseforks\fuseforks.exe" --autostart"#
        );
    }

    #[test]
    fn the_startup_approved_value_is_disabled_by_its_lowest_bit() {
        assert!(!approved_disabled(&[0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(!approved_disabled(&[0x06]));
        assert!(approved_disabled(&[0x03, 0, 0, 0, 0x10, 0x20, 0, 0, 0, 0, 0, 0]));
        assert!(approved_disabled(&[0x07]));
        assert!(!approved_disabled(&[]));
    }

    #[test]
    fn the_windows_view_reads_both_hives_and_the_task_manager() {
        let exe = "C:\\x\\fuseforks.exe".to_owned();
        let user = Some("\"C:\\x\\fuseforks.exe\" --autostart".to_owned());
        let machine = Some("\"C:\\y\\fuseforks.exe\" --autostart".to_owned());

        let none = windows_view(None, None, false, false, exe.clone());
        assert!(!none.enabled);
        assert_eq!(none.scope, AutostartScope::None);
        assert_eq!(none.registered_command, None);

        let only_user = windows_view(user.clone(), None, false, false, exe.clone());
        assert!(only_user.enabled);
        assert_eq!(only_user.scope, AutostartScope::User);

        // HKLM だけに残っている（チェックを外しても消せなかった形）。
        let only_machine = windows_view(None, machine.clone(), false, false, exe.clone());
        assert!(only_machine.enabled);
        assert_eq!(only_machine.scope, AutostartScope::Machine);
        assert_eq!(only_machine.registered_command, machine);

        // 両方あればコマンドは HKCU を出す。
        let both = windows_view(user.clone(), machine, false, false, exe.clone());
        assert_eq!(both.scope, AutostartScope::Both);
        assert_eq!(both.registered_command, user);

        // タスクマネージャーで切られていれば、登録があっても無効。
        assert!(!windows_view(user.clone(), None, true, false, exe.clone()).enabled);
        assert!(!windows_view(user, None, false, true, exe).enabled);
    }

    #[test]
    fn the_view_serializes_with_lowercase_scope_and_camel_case_keys() {
        let view = windows_view(Some("c".to_owned()), None, false, false, "e".to_owned());
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["scope"], "user");
        assert_eq!(json["registeredCommand"], "c");
        assert_eq!(json["currentExe"], "e");
        assert_eq!(json["enabled"], true);
    }

    #[test]
    fn the_plist_program_arguments_are_joined() {
        // auto-launch 0.6 の build_launch_agent_plist が書く形。
        // ProgramArguments より前に別の配列（AssociatedBundleIdentifiers）があっても取り違えない。
        let raw = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
  <key>Label</key>
  <string>fuseforks</string>
  <key>AssociatedBundleIdentifiers</key>
  <array><string>jp.outcasts.fuseforks</string></array>
  <key>ProgramArguments</key>
  <array><string>/Applications/fuseforks.app/Contents/MacOS/fuseforks</string><string>--autostart</string></array>
  <key>RunAtLoad</key>
  <true/>
  </dict>
</plist>"#;
        assert_eq!(
            parse_plist_program_arguments(raw).as_deref(),
            Some("/Applications/fuseforks.app/Contents/MacOS/fuseforks --autostart")
        );
        assert_eq!(parse_plist_program_arguments("<plist></plist>"), None);
    }

    #[test]
    fn the_desktop_exec_line_is_read() {
        // auto-launch 0.6 の build_xdg_autostart_data が書く形（パスは渡す側で引用符に包む）。
        let raw = "[Desktop Entry]\nType=Application\nVersion=1.0\nName=fuseforks\nComment=fuseforks startup script\nExec=\"/home/u/My Apps/fuseforks.AppImage\" --autostart\nStartupNotify=false\nTerminal=false";
        assert_eq!(
            parse_desktop_exec(raw).as_deref(),
            Some("\"/home/u/My Apps/fuseforks.AppImage\" --autostart")
        );
        assert_eq!(parse_desktop_exec("[Desktop Entry]\nType=Application"), None);
    }
}
