//! ホストの棚（`{app_data_dir}` に置く設定ファイル）の読み書きの共通部分（Spec 77 #5 / N3）。
//!
//! 棚は `mcp_server.json` / `jev.json` / `image_gen.json` / `pricing.json` / `probe_approvals.json` の 5 つ。
//!
//! **「ファイルが無い」だけが未作成。** それ以外の読み取りエラー（中身が UTF-8 でない・権限が無い・
//! 他のプロセスが共有なしで開いている）は、JSON の解析の失敗と同じく「読めない」として返す。
//! 読めない棚は既定値で読むが**書き戻さない**（`failures.md` #70 — 読みの安全側と書きの安全側は別）。
//! 旧実装は 5 つの棚がどれも `read_to_string` の `Err(_)` を「未作成」と読み、次の保存で人が入れた
//! 設定・合鍵・承認の一覧を既定値で上書きしていた。コアの `ConfigStore`（`load_schedules` /
//! `read_config` / `load_world`）は最初から `NotFound` を分けている — ここはそれに揃える。

use std::io::ErrorKind;
use std::path::Path;

use serde::de::DeserializeOwned;

/// 棚を読む。`Ok(None)` = ファイルが無い（初回）。`Err` = 読めない（読み取りか解析の失敗の理由）。
///
/// # Errors
/// `NotFound` 以外の読み取りエラー、または JSON として読めないとき（理由の文。**中身は含めない** —
/// 合鍵の入った棚があるので、読めなかった中身をログや画面へ流さない）。
pub(crate) fn load<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.to_string()),
    };
    serde_json::from_str(&raw)
        .map(Some)
        .map_err(|err| err.to_string())
}

/// 一時ファイルへ書き、権限を絞ってから本名へ rename する。
///
/// 本名へ直に書くと、書く途中で落ちたときに壊れた JSON が残り、次の起動でその棚が読めなくなる
/// （承認の台帳なら、人が直すまで前判定と後判定が全部止まる）。Unix では書いた直後の一瞬だけ
/// 既定の権限で見える窓も同じ 1 手で閉じる（2026-10-30 の監査 11 で `mcp_server.json` だけを直していた
/// 形を、5 つの棚へ広げた。Spec 77 N3）。
///
/// # Errors
/// フォルダを作れない・書けない・rename できないとき。
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_file_name(format!(
        "{}.tmp",
        path.file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_default()
    ));
    std::fs::write(&tmp, bytes)?;
    restrict_permissions(&tmp);
    std::fs::rename(&tmp, path)
}

/// 棚のファイルを本人だけが読める権限にする（Unix のみ）。
///
/// **Windows では何もしない** — `app_data_dir` は既に利用者ごとに分かれており、
/// ACL を自前で組むと「効いているつもりで効いていない」を作りやすい。
/// できないことをできたふりの処理で覆わない。
pub(crate) fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(err) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            fuseforks_core::note!(
                "settings: {} の権限を絞れませんでした（{err}）",
                path.display()
            );
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    /// 棚 5 つの保存は本名へ直に書かず、[`super::write_atomic`] を通す（Spec 77 N3）。
    ///
    /// 原子性そのものは振る舞いで観測できないので、ソースで留める。テストの節（`#[cfg(test)]` から後）は
    /// 数えない。ソースは実行時に読む（`include_str!` はコンパイル時に焼くので、ファイルを直しても
    /// 再コンパイルされないことがある）。
    #[test]
    fn every_store_saves_through_the_atomic_writer() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for file in [
            "mcp_server.rs",
            "jev_settings.rs",
            "image_gen_settings.rs",
            "pricing_source.rs",
            "probe_approvals.rs",
        ] {
            let source = std::fs::read_to_string(src.join(file)).unwrap();
            let body = source.split("#[cfg(test)]").next().unwrap();
            assert!(
                !body.contains("std::fs::write("),
                "{file} は棚を本名へ直に書かない（settings_file::write_atomic を通す）"
            );
            assert!(
                body.contains("settings_file::write_atomic("),
                "{file} の保存は settings_file::write_atomic を呼ぶ"
            );
            assert!(
                !body.contains("std::fs::read_to_string("),
                "{file} の読み込みは settings_file::load を通す（NotFound だけを未作成にする）"
            );
        }
    }
}
