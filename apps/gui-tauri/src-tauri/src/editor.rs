//! 発話の中のファイルの位置を開く（2026-11-08）。`{app_data_dir}/editor.json`。
//!
//! 起点は利用者 —「メッセージの中にあるファイルの位置をファイラー（エクスプローラー）やエディタで
//! 開けるようにしたい」。ここが持つのは 3 つ:
//!
//! - **パスの解決**（[`resolve_message_path`]）— 相対パスは発言した個体の作業フォルダから。実在するものだけ返す。
//!   作業フォルダの外も返す（案内と編集は実行しないので、`rag` のフォルダなども開けたほうが役に立つ）。
//! - **外部エディタの設定の棚**（[`EditorStore`]）— 端末ごとに違うので村（`world.json`）に入れない。
//!   **既定は空**（勝手に `code` を探すと、入っていない人には黙って失敗する）。
//! - **起動**（[`open_in_editor`]）— シェルを通さず、パスは 1 つの引数として渡す。**OS の既定のアプリでは
//!   開かない**（`.exe` / `.bat` / `.ps1` なら実行される。押す対象はモデルが書いた文章）。
//!
//! 起動するコマンドは画面から渡させず、この棚から読む — IPC が「任意のコマンドを走らせる口」にならないように。
//! エクスプローラーで場所を示すのは opener の `revealItemInDir`（既定の権限で使える・実行しない）を画面が呼ぶ。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// 設定ファイルの名前（`{app_data_dir}/editor.json`）。
pub const CONFIG_FILE: &str = "editor.json";

/// 引数のひな形の中で置き換える印。
pub const PATH_PLACEHOLDER: &str = "{path}";
/// 行番号の印。行番号の無いパスでは 1。
pub const LINE_PLACEHOLDER: &str = "{line}";

/// 外部エディタの設定。**`command` が空なら未設定**（「エディタで開く」は押せない）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorSettings {
    /// 実行ファイルの名前か絶対パス（例: `code`）。`PATH` から引く。
    #[serde(default)]
    pub command: String,
    /// 引数のひな形（1 要素 1 引数。例: `["-g", "{path}:{line}"]`）。`{path}` が無ければ末尾にパスを足す。
    #[serde(default)]
    pub args: Vec<String>,
}

/// `editor.json` の読み書き。壊れたファイルは未設定として読み、書き込みは拒まない（消えて困る値を持たない）。
#[derive(Debug)]
pub struct EditorStore {
    path: PathBuf,
    settings: EditorSettings,
}

impl EditorStore {
    /// 設定を読む。ファイルが無いのは失敗ではない。
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(CONFIG_FILE);
        let settings = match std::fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str::<EditorSettings>(&raw).unwrap_or_else(|err| {
                fuseforks_core::note!("editor: {CONFIG_FILE} を読めませんでした。未設定として続けます（{err}）");
                EditorSettings::default()
            }),
            Err(_) => EditorSettings::default(),
        };
        Self { path, settings }
    }

    /// 現在の設定。
    pub fn settings(&self) -> EditorSettings {
        self.settings.clone()
    }

    /// 設定を差し替えて保存する。書けなかったときは手元の値も変えない。
    ///
    /// # Errors
    /// フォルダの作成・一時ファイルの書き込み・置き換えに失敗した場合。
    pub fn save(&mut self, settings: EditorSettings) -> Result<(), String> {
        let raw = serde_json::to_string_pretty(&settings).map_err(|err| err.to_string())?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, raw).map_err(|err| err.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|err| err.to_string())?;
        self.settings = settings;
        Ok(())
    }
}

/// Tauri の管理状態。
pub struct EditorState {
    /// 設定の棚。
    pub store: Mutex<EditorStore>,
}

/// 解決したパス。画面へ返す（`path` は表示用に `\\?\` を剥がした絶対パス）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedPath {
    pub path: String,
    pub is_dir: bool,
}

/// 発話に書かれたパス（行番号は画面が外してから渡す）を解決する。**実在しなければ `None`。**
///
/// 絶対パスはそのまま、相対パスは `work_dir` から。`work_dir` が無い個体の相対パスは解決しない
/// （どこを指すか決められないものを、アプリの今の作業フォルダで当てずっぽうに引かない）。
pub fn resolve_message_path(work_dir: Option<&Path>, raw: &str) -> Option<ResolvedPath> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let candidate = Path::new(raw);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        work_dir?.join(candidate)
    };
    let canonical = std::fs::canonicalize(&joined).ok()?;
    Some(ResolvedPath {
        is_dir: canonical.is_dir(),
        path: fuseforks_core::process::display_path(&canonical),
    })
}

/// 引数のひな形を展開する。`{path}` と `{line}` を置き換え、`{path}` がどこにも無ければ末尾にパスを足す。
pub fn expand_args(template: &[String], path: &str, line: Option<u32>) -> Vec<String> {
    let line = line.unwrap_or(1).max(1).to_string();
    let mut args: Vec<String> = template
        .iter()
        .map(|arg| arg.replace(PATH_PLACEHOLDER, path).replace(LINE_PLACEHOLDER, &line))
        .collect();
    if !template.iter().any(|arg| arg.contains(PATH_PLACEHOLDER)) {
        args.push(path.to_owned());
    }
    args
}

/// 外部エディタでパスを開く。**待たない**（エディタは開いたまま残る）。
///
/// # Errors
/// 未設定・コマンドが見つからない・パスが無い・起動に失敗した場合（理由の文）。
pub fn open_in_editor(settings: &EditorSettings, path: &str, line: Option<u32>) -> Result<(), String> {
    let command = settings.command.trim();
    if command.is_empty() {
        return Err("外部エディタが設定されていません".to_owned());
    }
    if !Path::new(path).exists() {
        return Err(format!("見つかりません: {path}"));
    }
    let program = fuseforks_core::process::resolve_program(command)
        .ok_or_else(|| format!("コマンドが見つかりません: {command}"))?;
    let args = expand_args(&settings.args, path, line);
    let mut child = std::process::Command::new(&program)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|err| format!("起動できませんでした: {err}"))?;
    fuseforks_core::note!(
        "editor: opened program={} args={}",
        fuseforks_core::process::display_path(&program),
        args.len()
    );
    // 終わるのを待つのは別のスレッドで（Unix で子が終わった後にゾンビを残さない）。
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fuseforks-editor-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_missing_file_reads_as_not_configured() {
        let dir = temp_dir("missing");
        assert_eq!(EditorStore::load(&dir).settings(), EditorSettings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_round_trip_with_camel_case_keys() {
        let dir = temp_dir("roundtrip");
        let mut store = EditorStore::load(&dir);
        let next = EditorSettings {
            command: "code".to_owned(),
            args: vec!["-g".to_owned(), "{path}:{line}".to_owned()],
        };
        store.save(next.clone()).unwrap();
        let raw = std::fs::read_to_string(dir.join(CONFIG_FILE)).unwrap();
        assert!(raw.contains("\"command\": \"code\""), "{raw}");
        assert_eq!(EditorStore::load(&dir).settings(), next);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_file_reads_as_not_configured() {
        let dir = temp_dir("broken");
        std::fs::write(dir.join(CONFIG_FILE), "{ nope").unwrap();
        assert_eq!(EditorStore::load(&dir).settings(), EditorSettings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn args_are_expanded_and_the_path_is_one_argument() {
        let path = r"C:\a b\c.rs";
        assert_eq!(
            expand_args(&["-g".to_owned(), "{path}:{line}".to_owned()], path, Some(42)),
            vec!["-g".to_owned(), format!("{path}:42")]
        );
        // 行番号が無ければ 1。
        assert_eq!(
            expand_args(&["{path}:{line}".to_owned()], path, None),
            vec![format!("{path}:1")]
        );
        // `{path}` が無ければ末尾に足す（空白を含んでも 1 つの引数のまま）。
        assert_eq!(
            expand_args(&["--new-window".to_owned()], path, None),
            vec!["--new-window".to_owned(), path.to_owned()]
        );
        assert_eq!(expand_args(&[], path, None), vec![path.to_owned()]);
    }

    #[test]
    fn relative_paths_resolve_from_the_work_dir_and_must_exist() {
        let dir = temp_dir("resolve");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src").join("a.rs"), "").unwrap();

        let file = resolve_message_path(Some(&dir), "src/a.rs").unwrap();
        assert!(!file.is_dir);
        assert!(file.path.ends_with("a.rs"), "{}", file.path);
        assert!(!file.path.starts_with(r"\\?\"), "表示用に剥がす: {}", file.path);

        let folder = resolve_message_path(Some(&dir), "src").unwrap();
        assert!(folder.is_dir);

        // 絶対パスは作業フォルダに依らない。
        let absolute = dir.join("src").join("a.rs");
        assert!(resolve_message_path(None, absolute.to_str().unwrap()).is_some());

        assert_eq!(resolve_message_path(Some(&dir), "src/missing.rs"), None);
        // 作業フォルダの無い個体の相対パスは引かない。
        assert_eq!(resolve_message_path(None, "src/a.rs"), None);
        assert_eq!(resolve_message_path(Some(&dir), "  "), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn opening_without_a_command_is_refused_before_anything_runs() {
        let err = open_in_editor(&EditorSettings::default(), "C:/nope", None).unwrap_err();
        assert!(err.contains("設定されていません"), "{err}");
    }

    #[test]
    fn opening_a_missing_path_is_refused() {
        let settings = EditorSettings {
            command: "definitely-not-an-editor-xyz".to_owned(),
            args: vec![],
        };
        let err = open_in_editor(&settings, "Z:/definitely/not/here.rs", None).unwrap_err();
        assert!(err.contains("見つかりません"), "{err}");
    }
}
