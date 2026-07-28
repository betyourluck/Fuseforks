//! 作業フォルダ内を検索・比較する同梱ツール（`grep` / `diff`）。
//!
//! # なぜ同梱するか
//!
//! コーディング用エージェントが最も頻繁に使う道具がこの 2 つで、
//! ファイル全文を読むより桁違いに安く速い（トークン節約はこの製品の
//! 最重要課題の一つ）。MCP の filesystem サーバーにも検索はあるが、
//! 外部プロセスに依存せず誰の環境でも動くことに意味がある。
//!
//! # 探索範囲は作業フォルダに閉じる
//!
//! エージェントはプロンプトインジェクションを受けうるので、**読める範囲が
//! そのまま漏洩しうる範囲**になる。読めるのは [`crate::model::AgentSpec::work_dir`]
//! としてユーザーが明示したフォルダの中だけ。強制は入口のパス文字列検査ではなく
//! **canonicalize 後の前方一致**で行う — `..` の検査だけでは symlink 経由の
//! 脱出を塞げない。
//!
//! # 出力は必ず有界
//!
//! 一致件数・1 行の長さ・全体の文字数のすべてに上限を敷く。巨大な一致で
//! プロンプトを埋めると、節約のためのツールが逆に課金を膨らませる。
//! 打ち切りは黙って行わず、何件落としたかを結果に書く。

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::Value;

use crate::compute::spawn_rayon;
use crate::error::CoreResult;
use crate::tool::{AgentTool, ToolContext};

/// grep が返す一致の最大件数。
const MAX_MATCHES: usize = 100;

/// 一致行 1 本の表示上限（文字数）。minify 済み JS のような 1 行の塊で
/// 出力全体が埋まるのを防ぐ。
const MAX_LINE_CHARS: usize = 240;

/// ツール出力全体の上限（文字数）。ツール結果はそのままプロンプトへ入る。
const MAX_OUTPUT_CHARS: usize = 12_000;

/// 読み込む 1 ファイルの上限（bytes）。これを超えるファイルは走査しない。
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// 1 回の grep で走査するファイル数の上限。病的に広い木で止まらなくなるのを防ぐ。
const MAX_FILES: usize = 20_000;

/// 名前で走査から外すディレクトリ。ビルド成果物と依存の置き場は
/// 一致してもノイズにしかならず、走査時間だけを食う。
const SKIP_DIRS: [&str; 6] = ["node_modules", "target", "dist", "build", "out", "vendor"];

/// 作業フォルダを起点に相対パスを解決し、**フォルダの外なら拒否**する。
///
/// 戻りは (絶対パス, 表示用の相対パス)。エラーはモデルへそのまま返る文字列。
fn resolve_in_work_dir(work_dir: &Path, user_path: &str) -> Result<(PathBuf, String), String> {
    let root = work_dir.canonicalize().map_err(|_| {
        format!(
            "作業フォルダ `{}` が存在しません。設定を確認してください。",
            work_dir.display()
        )
    })?;

    let joined = if user_path.is_empty() || user_path == "." {
        root.clone()
    } else {
        root.join(user_path)
    };

    // canonicalize は実在しないパスで失敗する。「無い」と「外」を区別して返す。
    let resolved = joined
        .canonicalize()
        .map_err(|_| format!("`{user_path}` は作業フォルダ内に見つかりません。"))?;

    if !resolved.starts_with(&root) {
        return Err(format!(
            "`{user_path}` は作業フォルダの外を指しています。読めるのは作業フォルダの中だけです。"
        ));
    }

    let display = resolved
        .strip_prefix(&root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    Ok((resolved, display))
}

/// 作業フォルダが未設定のときの案内文。grep / diff で共通。
fn work_dir_missing() -> String {
    "作業フォルダが設定されていないため、このツールは使えません。\
     利用者に「エージェント設定の『作業フォルダ』欄にフォルダを指定してほしい」と伝えてください。"
        .to_owned()
}

/// 走査で拾った 1 エントリ。
struct WalkEntry {
    path: PathBuf,
    is_dir: bool,
}

/// 走査対象を列挙する。順序は決定的（名前順の深さ優先）。
///
/// `include_dirs` はディレクトリ自身も結果へ含めるか（`fd` 用）。
/// 上限 [`MAX_FILES`] に達したら打ち切り、第 2 戻り値で伝える。
fn collect_entries(root: &Path, include_dirs: bool) -> (Vec<WalkEntry>, bool) {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let mut truncated = false;

    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue; // 読めないディレクトリ（権限など）は黙って飛ばす。
        };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(std::fs::DirEntry::file_name);

        for entry in entries {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(kind) = entry.file_type() else { continue };

            // symlink は辿らない。作業フォルダ外へのリンクを踏むと囲いが破れる。
            if kind.is_symlink() {
                continue;
            }
            if found.len() >= MAX_FILES {
                truncated = true;
                return (found, truncated);
            }
            if kind.is_dir() {
                // 隠しディレクトリ（.git / .venv 等）と定番のビルド出力を外す。
                if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) {
                    continue;
                }
                if include_dirs {
                    found.push(WalkEntry {
                        path: path.clone(),
                        is_dir: true,
                    });
                }
                stack.push(path);
            } else if kind.is_file() {
                found.push(WalkEntry {
                    path,
                    is_dir: false,
                });
            }
        }
    }
    (found, truncated)
}

/// 走査対象のファイルだけを列挙する（grep 用）。
fn collect_files(root: &Path) -> (Vec<PathBuf>, bool) {
    let (entries, truncated) = collect_entries(root, false);
    (entries.into_iter().map(|e| e.path).collect(), truncated)
}

/// バイナリらしいファイルか。先頭 4 KiB に NUL が居れば読まない。
fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(4096).any(|&b| b == 0)
}

/// 一致行を表示上限へ丸める。
fn clip_line(line: &str) -> String {
    let trimmed = line.trim_end();
    if trimmed.chars().count() <= MAX_LINE_CHARS {
        return trimmed.to_owned();
    }
    let clipped: String = trimmed.chars().take(MAX_LINE_CHARS).collect();
    format!("{clipped} …")
}

// ---------------------------------------------------------------------------

/// 作業フォルダ内のファイルから正規表現に一致する行を探すツール。
pub struct GrepTool;

#[async_trait]
impl AgentTool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> String {
        "作業フォルダ内のファイルから、正規表現に一致する行を探す。\
         **ファイルの中身を知りたいときは、全文を読む前にまずこれで当たりを付けること** — \
         一致行だけが返るので速くて安い。結果は `パス:行番号: 内容` の形式。\
         検索できるのはエージェント設定で指定された作業フォルダの中だけ。"
            .to_owned()
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "検索する正規表現（Rust regex 構文）。例: `fn \\w+`"
                },
                "path": {
                    "type": "string",
                    "description": "検索範囲を絞る相対パス（ファイルまたはフォルダ）。省略時は作業フォルダ全体"
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "大文字小文字を無視するか。既定は false"
                }
            },
            "required": ["pattern"],
            "additionalProperties": false
        })
    }

    async fn call(&self, ctx: &ToolContext, args: &Value) -> CoreResult<String> {
        let Some(work_dir) = ctx.work_dir.clone() else {
            return Ok(work_dir_missing());
        };
        let Some(pattern) = args.get("pattern").and_then(Value::as_str) else {
            return Ok("引数 `pattern` が必要です。".into());
        };
        let pattern = pattern.to_owned();
        let rel_path = args
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let case_insensitive = args
            .get("case_insensitive")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        // 走査は I/O + CPU の塊なので Rayon 側へ逃がし、Tokio ワーカーを空ける。
        spawn_rayon(move || run_grep(&work_dir, &pattern, &rel_path, case_insensitive)).await
    }
}

/// grep 本体。ブロッキングして良い文脈で呼ぶ。
fn run_grep(work_dir: &Path, pattern: &str, rel_path: &str, case_insensitive: bool) -> String {
    let regex = match regex::RegexBuilder::new(pattern)
        .case_insensitive(case_insensitive)
        // 破滅的な巨大パターンでメモリを食い潰さないための上限（regex 既定の 10 倍未満）。
        .size_limit(1 << 20)
        .build()
    {
        Ok(regex) => regex,
        Err(err) => {
            return format!("正規表現として解釈できません: {err}\nパターンを直して再試行してください。");
        }
    };

    let (start, _) = match resolve_in_work_dir(work_dir, rel_path) {
        Ok(resolved) => resolved,
        Err(message) => return message,
    };

    let (files, files_truncated) = if start.is_file() {
        (vec![start.clone()], false)
    } else {
        collect_files(&start)
    };

    let root = work_dir.canonicalize().unwrap_or_else(|_| work_dir.to_path_buf());
    let mut lines: Vec<String> = Vec::new();
    let mut total_matches = 0usize;
    let mut output_chars = 0usize;
    let mut clipped = false;

    'files: for file in &files {
        if let Ok(meta) = file.metadata()
            && meta.len() > MAX_FILE_BYTES
        {
            continue;
        }
        let Ok(bytes) = std::fs::read(file) else { continue };
        if looks_binary(&bytes) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        let display = file
            .strip_prefix(&root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| file.display().to_string());

        for (index, line) in text.lines().enumerate() {
            if !regex.is_match(line) {
                continue;
            }
            total_matches += 1;
            if total_matches > MAX_MATCHES || output_chars > MAX_OUTPUT_CHARS {
                clipped = true;
                break 'files;
            }
            let entry = format!("{display}:{}: {}", index + 1, clip_line(line));
            output_chars += entry.chars().count();
            lines.push(entry);
        }
    }

    if lines.is_empty() {
        return format!(
            "一致なし（{} ファイルを走査）。パターンや範囲を変えて再試行できます。",
            files.len()
        );
    }

    let mut out = format!("{} 件が一致:\n", lines.len());
    out.push_str(&lines.join("\n"));
    if clipped {
        out.push_str(&format!(
            "\n（表示上限に達したため打ち切りました。`path` で範囲を絞るか、パターンを具体化してください）"
        ));
    }
    if files_truncated {
        out.push_str(&format!(
            "\n（ファイル数が {MAX_FILES} を超えたため、一部は走査していません）"
        ));
    }
    out
}

// ---------------------------------------------------------------------------

/// 作業フォルダ内をファイル名で探すツール（`fd` 相当）。
pub struct FdTool;

#[async_trait]
impl AgentTool for FdTool {
    fn name(&self) -> &str {
        "fd"
    }

    fn description(&self) -> String {
        "作業フォルダ内のファイル・フォルダを**名前**で探す。\
         **ファイルの場所や存在を知りたいときは、中身を検索する grep より先にこれを使うこと。**\
         パターンは名前（パスの最後の要素）に対する正規表現で、\
         結果は相対パスの一覧（フォルダは末尾 `/`）。\
         探せるのはエージェント設定で指定された作業フォルダの中だけ。"
            .to_owned()
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "名前に対する正規表現（Rust regex 構文）。例: `\\.md$`、`^config`"
                },
                "path": {
                    "type": "string",
                    "description": "探索範囲を絞る相対パス（フォルダ）。省略時は作業フォルダ全体"
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "大文字小文字を無視するか。既定は true（名前検索は表記揺れが多い）"
                }
            },
            "required": ["pattern"],
            "additionalProperties": false
        })
    }

    async fn call(&self, ctx: &ToolContext, args: &Value) -> CoreResult<String> {
        let Some(work_dir) = ctx.work_dir.clone() else {
            return Ok(work_dir_missing());
        };
        let Some(pattern) = args.get("pattern").and_then(Value::as_str) else {
            return Ok("引数 `pattern` が必要です。".into());
        };
        let pattern = pattern.to_owned();
        let rel_path = args
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        // grep と既定が違う: 名前検索は「Readme か README か」のような表記揺れが
        // 本質的に多く、厳密一致を既定にすると空振り → 再試行の無駄な往復が増える。
        let case_insensitive = args
            .get("case_insensitive")
            .and_then(Value::as_bool)
            .unwrap_or(true);

        spawn_rayon(move || run_fd(&work_dir, &pattern, &rel_path, case_insensitive)).await
    }
}

/// fd 本体。ブロッキングして良い文脈で呼ぶ。
fn run_fd(work_dir: &Path, pattern: &str, rel_path: &str, case_insensitive: bool) -> String {
    let regex = match regex::RegexBuilder::new(pattern)
        .case_insensitive(case_insensitive)
        .size_limit(1 << 20)
        .build()
    {
        Ok(regex) => regex,
        Err(err) => {
            return format!("正規表現として解釈できません: {err}\nパターンを直して再試行してください。");
        }
    };

    let (start, _) = match resolve_in_work_dir(work_dir, rel_path) {
        Ok(resolved) => resolved,
        Err(message) => return message,
    };
    if start.is_file() {
        return "`path` にはフォルダを指定してください（ファイルの中身を探すなら grep）。".into();
    }

    let root = work_dir.canonicalize().unwrap_or_else(|_| work_dir.to_path_buf());
    let (entries, walk_truncated) = collect_entries(&start, true);

    let mut hits: Vec<String> = entries
        .iter()
        .filter(|entry| {
            // 一致対象は名前（最後の要素）だけ。相対パス全体に掛けると、一致した
            // フォルダの配下すべてが道連れでヒットし、一覧がノイズで埋まる。
            entry
                .path
                .file_name()
                .map(|name| regex.is_match(&name.to_string_lossy()))
                .unwrap_or(false)
        })
        .map(|entry| {
            let display = entry
                .path
                .strip_prefix(&root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| entry.path.display().to_string());
            if entry.is_dir {
                format!("{display}/")
            } else {
                display
            }
        })
        .collect();
    hits.sort();

    if hits.is_empty() {
        return format!(
            "一致なし（{} エントリを走査）。パターンや範囲を変えて再試行できます。",
            entries.len()
        );
    }

    let total = hits.len();
    let clipped = total > MAX_MATCHES;
    hits.truncate(MAX_MATCHES);

    let mut out = format!("{total} 件が一致:\n");
    out.push_str(&hits.join("\n"));
    if clipped {
        out.push_str(&format!(
            "\n（先頭 {MAX_MATCHES} 件のみ表示。`path` で範囲を絞るか、パターンを具体化してください）"
        ));
    }
    if walk_truncated {
        out.push_str(&format!(
            "\n（エントリ数が {MAX_FILES} を超えたため、一部は走査していません）"
        ));
    }
    out
}

// ---------------------------------------------------------------------------

/// 作業フォルダ内の 2 ファイルを unified diff で比較するツール。
pub struct DiffTool;

#[async_trait]
impl AgentTool for DiffTool {
    fn name(&self) -> &str {
        "diff"
    }

    fn description(&self) -> String {
        "作業フォルダ内の 2 つのファイルの差分を unified diff 形式で返す。\
         **2 つのファイルの違いを知りたいとき、両方の全文を読む代わりに使うこと** — \
         変わった行だけが返るので速くて安い。\
         比較できるのはエージェント設定で指定された作業フォルダの中だけ。"
            .to_owned()
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "old_path": {
                    "type": "string",
                    "description": "比較元ファイルの相対パス"
                },
                "new_path": {
                    "type": "string",
                    "description": "比較先ファイルの相対パス"
                }
            },
            "required": ["old_path", "new_path"],
            "additionalProperties": false
        })
    }

    async fn call(&self, ctx: &ToolContext, args: &Value) -> CoreResult<String> {
        let Some(work_dir) = ctx.work_dir.clone() else {
            return Ok(work_dir_missing());
        };
        let (Some(old_path), Some(new_path)) = (
            args.get("old_path").and_then(Value::as_str),
            args.get("new_path").and_then(Value::as_str),
        ) else {
            return Ok("引数 `old_path` と `new_path` の両方が必要です。".into());
        };
        let (old_path, new_path) = (old_path.to_owned(), new_path.to_owned());

        spawn_rayon(move || run_diff(&work_dir, &old_path, &new_path)).await
    }
}

/// diff 本体。ブロッキングして良い文脈で呼ぶ。
fn run_diff(work_dir: &Path, old_path: &str, new_path: &str) -> String {
    let read = |user_path: &str| -> Result<(String, String), String> {
        let (path, display) = resolve_in_work_dir(work_dir, user_path)?;
        if !path.is_file() {
            return Err(format!("`{user_path}` はファイルではありません。"));
        }
        if path.metadata().map(|m| m.len()).unwrap_or(0) > MAX_FILE_BYTES {
            return Err(format!(
                "`{user_path}` が大きすぎます（上限 {} bytes）。",
                MAX_FILE_BYTES
            ));
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("`{user_path}` を読めません: {e}"))?;
        if looks_binary(&bytes) {
            return Err(format!("`{user_path}` はバイナリファイルのため比較できません。"));
        }
        Ok((String::from_utf8_lossy(&bytes).into_owned(), display))
    };

    let (old_text, old_display) = match read(old_path) {
        Ok(ok) => ok,
        Err(message) => return message,
    };
    let (new_text, new_display) = match read(new_path) {
        Ok(ok) => ok,
        Err(message) => return message,
    };

    if old_text == new_text {
        return "2 つのファイルの内容は同一です。".to_owned();
    }

    let diff = similar::TextDiff::from_lines(&old_text, &new_text);
    let unified = diff
        .unified_diff()
        .context_radius(3)
        .header(&old_display, &new_display)
        .to_string();

    if unified.chars().count() > MAX_OUTPUT_CHARS {
        let clipped: String = unified.chars().take(MAX_OUTPUT_CHARS).collect();
        return format!(
            "{clipped}\n（差分が大きいため打ち切りました。ファイルを分けて比較してください）"
        );
    }
    unified
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AgentId;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "concordia-fs-{tag}-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn write(&self, rel: &str, content: &str) {
            let path = self.0.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn ctx_with(work_dir: Option<&Path>) -> ToolContext {
        ToolContext {
            agent_id: AgentId::from("agent_01"),
            work_dir: work_dir.map(Path::to_path_buf),
        }
    }

    #[tokio::test]
    async fn grep_without_a_work_dir_explains_how_to_enable_it() {
        let reply = GrepTool
            .call(&ctx_with(None), &serde_json::json!({ "pattern": "x" }))
            .await
            .unwrap();
        assert!(reply.contains("作業フォルダ"), "有効化の道筋を案内すること: {reply}");
    }

    #[tokio::test]
    async fn grep_finds_matches_with_relative_paths_and_line_numbers() {
        let dir = TempDir::new("hit");
        dir.write("src/main.rs", "fn main() {}\nfn helper() {}\n");
        dir.write("readme.md", "説明\n");

        let reply = GrepTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({ "pattern": "fn \\w+" }),
            )
            .await
            .unwrap();

        assert!(reply.contains("src/main.rs:1: fn main() {}"), "{reply}");
        assert!(reply.contains("src/main.rs:2: fn helper() {}"), "{reply}");
        assert!(!reply.contains("readme.md"), "一致しないファイルは出さない");
    }

    #[tokio::test]
    async fn grep_rejects_paths_that_escape_the_work_dir() {
        let parent = TempDir::new("escape");
        parent.write("secret.txt", "外側の機密\n");
        let inner = parent.0.join("inner");
        std::fs::create_dir_all(&inner).unwrap();

        let reply = GrepTool
            .call(
                &ctx_with(Some(&inner)),
                &serde_json::json!({ "pattern": "機密", "path": "../secret.txt" }),
            )
            .await
            .unwrap();

        assert!(!reply.contains("外側の機密"), "囲いの外の内容が漏れないこと");
        // 実在しない扱い・囲い外扱いのどちらでもよいが、一致としては返さない。
        assert!(!reply.contains("件が一致"), "{reply}");
    }

    #[tokio::test]
    async fn grep_skips_hidden_and_dependency_directories() {
        let dir = TempDir::new("skip");
        dir.write(".git/config", "url = needle\n");
        dir.write("node_modules/pkg/index.js", "needle\n");
        dir.write("src/lib.rs", "needle\n");

        let reply = GrepTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({ "pattern": "needle" }),
            )
            .await
            .unwrap();

        assert!(reply.contains("src/lib.rs"), "{reply}");
        assert!(!reply.contains(".git"), "{reply}");
        assert!(!reply.contains("node_modules"), "{reply}");
    }

    #[tokio::test]
    async fn grep_output_is_bounded_and_announces_the_cut() {
        let dir = TempDir::new("cap");
        let body: String = (0..500).map(|i| format!("needle {i}\n")).collect();
        dir.write("big.txt", &body);

        let reply = GrepTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({ "pattern": "needle" }),
            )
            .await
            .unwrap();

        assert!(reply.contains("打ち切りました"), "黙って切らないこと: {reply}");
        assert!(
            reply.lines().count() <= MAX_MATCHES + 4,
            "上限を大きく超えないこと: {} 行",
            reply.lines().count()
        );
    }

    #[tokio::test]
    async fn grep_reports_invalid_regex_as_a_readable_message() {
        let dir = TempDir::new("badre");
        let reply = GrepTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({ "pattern": "([" }),
            )
            .await
            .unwrap();
        assert!(reply.contains("正規表現"), "{reply}");
    }

    #[tokio::test]
    async fn grep_can_be_case_insensitive() {
        let dir = TempDir::new("case");
        dir.write("a.txt", "Needle\n");

        let strict = GrepTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({ "pattern": "needle" }),
            )
            .await
            .unwrap();
        assert!(strict.contains("一致なし"), "{strict}");

        let relaxed = GrepTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({ "pattern": "needle", "case_insensitive": true }),
            )
            .await
            .unwrap();
        assert!(relaxed.contains("a.txt:1"), "{relaxed}");
    }

    #[tokio::test]
    async fn fd_without_a_work_dir_explains_how_to_enable_it() {
        let reply = FdTool
            .call(&ctx_with(None), &serde_json::json!({ "pattern": "x" }))
            .await
            .unwrap();
        assert!(reply.contains("作業フォルダ"), "{reply}");
    }

    #[tokio::test]
    async fn fd_finds_files_and_dirs_by_name_and_marks_dirs() {
        let dir = TempDir::new("fd-hit");
        dir.write("src/main.rs", "");
        dir.write("src/config/app.toml", "");
        dir.write("config.md", "");

        let reply = FdTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({ "pattern": "^config" }),
            )
            .await
            .unwrap();

        assert!(reply.contains("config.md"), "{reply}");
        assert!(reply.contains("src/config/"), "フォルダは末尾スラッシュ: {reply}");
        assert!(!reply.contains("main.rs"), "{reply}");
    }

    #[tokio::test]
    async fn fd_matches_the_name_not_the_whole_relative_path() {
        let dir = TempDir::new("fd-name");
        dir.write("needle/inner.txt", "");
        dir.write("other/needle.txt", "");

        let reply = FdTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({ "pattern": "needle" }),
            )
            .await
            .unwrap();

        assert!(reply.contains("needle/"), "{reply}");
        assert!(reply.contains("other/needle.txt"), "{reply}");
        assert!(
            !reply.contains("needle/inner.txt"),
            "一致フォルダの配下を道連れにしない: {reply}"
        );
    }

    #[tokio::test]
    async fn fd_is_case_insensitive_by_default() {
        let dir = TempDir::new("fd-case");
        dir.write("README.md", "");

        let reply = FdTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({ "pattern": "readme" }),
            )
            .await
            .unwrap();
        assert!(reply.contains("README.md"), "{reply}");

        let strict = FdTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({ "pattern": "readme", "case_insensitive": false }),
            )
            .await
            .unwrap();
        assert!(strict.contains("一致なし"), "{strict}");
    }

    #[tokio::test]
    async fn fd_rejects_paths_that_escape_the_work_dir() {
        let parent = TempDir::new("fd-escape");
        parent.write("secret.txt", "");
        let inner = parent.0.join("inner");
        std::fs::create_dir_all(&inner).unwrap();

        let reply = FdTool
            .call(
                &ctx_with(Some(&inner)),
                &serde_json::json!({ "pattern": "secret", "path": ".." }),
            )
            .await
            .unwrap();
        assert!(!reply.contains("secret.txt"), "囲いの外を列挙しないこと: {reply}");
    }

    #[tokio::test]
    async fn fd_output_is_bounded_and_announces_the_cut() {
        let dir = TempDir::new("fd-cap");
        for i in 0..150 {
            dir.write(&format!("needle_{i:03}.txt"), "");
        }

        let reply = FdTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({ "pattern": "needle" }),
            )
            .await
            .unwrap();

        assert!(reply.contains("150 件が一致"), "{reply}");
        assert!(reply.contains("先頭 100 件のみ表示"), "黙って切らないこと: {reply}");
        assert!(reply.lines().count() <= MAX_MATCHES + 3, "{reply}");
    }

    #[tokio::test]
    async fn diff_produces_a_unified_diff_between_two_files() {
        let dir = TempDir::new("diff");
        dir.write("old.txt", "共通\n古い行\n共通2\n");
        dir.write("new.txt", "共通\n新しい行\n共通2\n");

        let reply = DiffTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({ "old_path": "old.txt", "new_path": "new.txt" }),
            )
            .await
            .unwrap();

        assert!(reply.contains("-古い行"), "{reply}");
        assert!(reply.contains("+新しい行"), "{reply}");
        assert!(reply.contains("old.txt"), "ヘッダにファイル名が入ること: {reply}");
    }

    #[tokio::test]
    async fn diff_on_identical_files_says_so_instead_of_returning_nothing() {
        let dir = TempDir::new("same");
        dir.write("a.txt", "同じ\n");
        dir.write("b.txt", "同じ\n");

        let reply = DiffTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({ "old_path": "a.txt", "new_path": "b.txt" }),
            )
            .await
            .unwrap();
        assert!(reply.contains("同一"), "{reply}");
    }

    #[tokio::test]
    async fn diff_reports_missing_files_and_escapes_as_readable_messages() {
        let parent = TempDir::new("diff-escape");
        parent.write("secret.txt", "外側\n");
        let inner = parent.0.join("inner");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(inner.join("a.txt"), "中\n").unwrap();

        let missing = DiffTool
            .call(
                &ctx_with(Some(&inner)),
                &serde_json::json!({ "old_path": "a.txt", "new_path": "ghost.txt" }),
            )
            .await
            .unwrap();
        assert!(missing.contains("ghost.txt"), "{missing}");

        let escape = DiffTool
            .call(
                &ctx_with(Some(&inner)),
                &serde_json::json!({ "old_path": "a.txt", "new_path": "../secret.txt" }),
            )
            .await
            .unwrap();
        assert!(!escape.contains("外側"), "囲いの外の内容が漏れないこと: {escape}");
        assert!(!escape.contains("---"), "diff として成立させないこと: {escape}");
    }
}
