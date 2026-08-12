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
//!
//! **上限は表示に掛け、数えには掛けない。** 表示した件数を総数として返すと、
//! 読み手は「何件あるか」を誤り、確かめるための再検索が周を食う。
//! 走査は一致ゼロの grep でも全ファイルに及ぶので、数え続けても最悪コストは
//! 変わらない — 打ち切って節約できるのは出力だけで、走査ではない。

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
pub(crate) const MAX_LINE_CHARS: usize = 240;

/// ツール出力全体の上限（文字数）。ツール結果はそのままプロンプトへ入る。
/// 書き換え系ではこの上限を超える diff は「切り詰め」ではなく**拒否**になる
/// （write_tools_contract。切り詰めた diff は「何が変わったか」の契約を壊す）。
pub(crate) const MAX_OUTPUT_CHARS: usize = 12_000;

/// 読み込む 1 ファイルの上限（bytes）。これを超えるファイルは走査しない。
/// 書き換え系（tools/edit.rs）も同じ上限を共有する。
pub(crate) const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// 1 回の grep で走査するファイル数の上限。病的に広い木で止まらなくなるのを防ぐ。
pub(crate) const MAX_FILES: usize = 20_000;

/// ファイル別の件数を並べる最大ファイル数（件数の多い順）。
/// 内訳そのものが出力を埋めては本末転倒なので、ここにも上限を敷く。
const MAX_COUNT_FILES: usize = 20;

/// 名前で走査から外すディレクトリ。ビルド成果物と依存の置き場は
/// 一致してもノイズにしかならず、走査時間だけを食う。
const SKIP_DIRS: [&str; 6] = ["node_modules", "target", "dist", "build", "out", "vendor"];

/// 作業フォルダを起点に相対パスを解決し、**フォルダの外なら拒否**する。
///
/// 戻りは (絶対パス, 表示用の相対パス)。エラーはモデルへそのまま返る文字列。
/// canonicalize は実在するパスでしか成功しないため、この関数を通る限り
/// **新規ファイルの作成は構造的に不可能**（write_tools_contract の担保の片翼）。
pub(crate) fn resolve_in_work_dir(
    work_dir: &Path,
    user_path: &str,
) -> Result<(PathBuf, String), String> {
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

/// 作業フォルダを起点に**まだ存在しない宛先**を解決し、外なら拒否する（Spec 09）。
///
/// [`resolve_in_work_dir`] は canonicalize が成功する = 実在するパスにしか使えない。
/// 新規作成の宛先は実在しないのが正常系なので、そのままでは全部
/// 「見つかりません」で落ちる。**囲いの強さを落とさずに**実在しない宛先を
/// 通すため、3 段で検査する:
///
/// 1. 宛先の**実在する最も深い祖先**を canonicalize し、前方一致で囲いの中を確認
/// 2. 祖先から宛先までの残り成分に `..` と絶対パス成分が無いことを確認
///    （canonicalize できない区間は文字列検査しか手が無い。`..` を通すと、
///    実在する祖先の検査をすり抜けて外へ出られる）
/// 3. symlink は辿らない（1 の canonicalize は祖先までしか及ばないので、
///    宛先自身が既存の symlink なら別途弾く）
///
/// **この関数は `file` ツール専用。** `sd` / `yq` は [`resolve_in_work_dir`] を
/// 使い続ける — あちらの canonicalize が「新規ファイル作成なし」を構造的に
/// 担保しており（`write_tools_contract`）、こちらの存在でその担保は弱まらない。
///
/// 戻りは (絶対パス, 表示用の相対パス)。パス自体は実在してもしなくてもよい。
pub(crate) fn resolve_creatable(
    work_dir: &Path,
    user_path: &str,
) -> Result<(PathBuf, String), String> {
    let root = work_dir.canonicalize().map_err(|_| {
        format!(
            "作業フォルダ `{}` が存在しません。設定を確認してください。",
            work_dir.display()
        )
    })?;

    if user_path.trim().is_empty() || user_path == "." {
        return Err("パスが空です。作業フォルダからの相対パスを指定してください。".to_owned());
    }

    let outside =
        || format!("`{user_path}` は作業フォルダの外を指しています。操作できるのは作業フォルダの中だけです。");

    let joined = root.join(user_path);
    // 絶対パスの user_path は join で root を丸ごと置き換える（Path::join の仕様）。
    // 検査ではなく結果を見て弾く — 表記の種類（ドライブ文字・UNC・先頭 /）を
    // 数え上げると必ず漏れる。
    if !joined.starts_with(&root) {
        return Err(outside());
    }

    // 実在する最深の祖先まで遡る。ここまでは canonicalize が効く。
    let mut existing = joined.as_path();
    let mut trailing: Vec<&std::ffi::OsStr> = Vec::new();
    while !existing.exists() {
        let Some(name) = existing.file_name() else {
            return Err(outside());
        };
        trailing.push(name);
        let Some(parent) = existing.parent() else {
            return Err(outside());
        };
        existing = parent;
    }

    let anchor = existing.canonicalize().map_err(|_| outside())?;
    if !anchor.starts_with(&root) {
        return Err(outside());
    }

    // 残り成分の検査。`..` は祖先の検査をすり抜けるので、ここで必ず落とす。
    // （`joined` の時点では `a/../../x` のような並びが文字列として残っている）
    for component in joined.strip_prefix(&root).map_err(|_| outside())?.components() {
        match component {
            std::path::Component::Normal(_) | std::path::Component::CurDir => {}
            _ => return Err(outside()),
        }
    }

    // 宛先自身が既存の symlink なら弾く。祖先の canonicalize では追えない
    // （実在する = ループを抜けるので、リンク先が外でも 1 の検査は通ってしまう）。
    if joined
        .symlink_metadata()
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(outside());
    }

    // 表示は root からの相対。canonicalize 済みの祖先へ残りを積み直すことで、
    // 途中に混ざった `.` を落とした正規形にする。
    let mut resolved = anchor;
    for name in trailing.into_iter().rev() {
        resolved.push(name);
    }
    let display = resolved
        .strip_prefix(&root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| user_path.to_owned());

    Ok((resolved, display))
}

/// 作業フォルダが未設定のときの案内文。全ファイル系ツールで共通。
pub(crate) fn work_dir_missing() -> String {
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

/// 走査対象のファイルだけを列挙する（grep / rag 共用）。
///
/// `pub(crate)` なのは走査の規律（symlink 不追従・隠しフォルダと
/// [`SKIP_DIRS`] の除外・[`MAX_FILES`] の打ち切り）を 2 箇所に生やさないため —
/// 別実装を持つと**片方だけ直る**（Spec 18 D8。#55 / #58 と同型）。
pub(crate) fn collect_files(root: &Path) -> (Vec<PathBuf>, bool) {
    let (entries, truncated) = collect_entries(root, false);
    (entries.into_iter().map(|e| e.path).collect(), truncated)
}

/// 作業フォルダのファイルを**相対パスの文字列**として列挙する（Spec 24 の補完用）。
///
/// [`collect_files`] の隣に置くのは、走査の規律と**相対表示への変換**を
/// 同じ場所に留めるため。区切りは `/` に揃える（`resolve_in_work_dir` と
/// `rag::rel_display` の既存の作法）。
///
/// **並びはソート済み。** 走査の順は決定的だがスタック由来で人には読めないので、
/// 補完の同点破りが安定するようここで整える。
///
/// # 読めないフォルダの扱い
///
/// `root` が存在しない・読めないときは**空の一覧**を返す（[`collect_entries`] が
/// 読めないディレクトリを飛ばす規律のまま）。**ここでは理由を作らない** —
/// 作業フォルダが壊れていることは `resolve_in_work_dir` が
/// 「作業フォルダ `X` が存在しません」と**既に名指しで報告する**ので、
/// 2 つ目の報告経路を生やすと同じ規律が 2 箇所に住む。
/// 補完は候補を出せないだけで、原因は道具を使った瞬間に画面へ出る。
pub(crate) fn relative_file_paths(root: &Path) -> (Vec<String>, bool) {
    let (paths, truncated) = collect_files(root);
    let mut rel: Vec<String> = paths
        .iter()
        .filter_map(|p| p.strip_prefix(root).ok())
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    rel.sort();
    (rel, truncated)
}

/// バイナリらしいファイルか。先頭 4 KiB に NUL が居れば読まない。
pub(crate) fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(4096).any(|&b| b == 0)
}

/// 一致行を表示上限へ丸める。
pub(crate) fn clip_line(line: &str) -> String {
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

    fn description(&self, language: crate::world::Language) -> String {
        language
            .pick(
                "作業フォルダ内のファイルから、正規表現に一致する行を探す。\
                 **ファイルの中身を知りたいときは、全文を読む前にまずこれで当たりを付けること** — \
                 一致行だけが返るので速くて安い。結果は `パス:行番号: 内容` の形式。\
                 検索できるのはエージェント設定で指定された作業フォルダの中だけ。\
                 **「何件あるか」だけを知りたいときは `count_only: true`** — \
                 一致行を返さずファイル別と全体の件数だけを返すので、\
                 パターンを変えて何度も叩き直す必要がなくなる。\
                 `include` でファイル名を**正規表現**で絞れる（例: `\\.rs$`。**glob ではない**）。\
                 `context: 1〜3` で一致行の前後も返る（一致行は `:`、前後の行は `-`）。",
                "Search files in the working folder for lines matching a regular \
                 expression. **When you want to know what a file contains, probe with \
                 this before reading the whole file** — only matching lines come back, \
                 so it is fast and cheap. Results are `path:line: content`. \
                 Only the working folder configured for this agent is searchable. \
                 **If you only need to know how many matches there are, pass \
                 `count_only: true`** — it returns per-file and total counts instead of \
                 the lines, so you don't have to re-run with narrower patterns. \
                 `include` filters by **file name as a regex** (e.g. `\\.rs$`; \
                 **not a glob**). `context: 1-3` also returns surrounding lines \
                 (matches are marked `:`, context lines `-`).",
            )
            .to_owned()
    }

    fn parameters(&self, language: crate::world::Language) -> Value {
        let d = |ja: &str, en: &str| language.pick(ja, en).to_owned();
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": d("検索する正規表現（Rust regex 構文）。例: `fn \\w+`",
                                     "Regular expression to search for (Rust regex syntax), e.g. `fn \\w+`")
                },
                "path": {
                    "type": "string",
                    "description": d("検索範囲を絞る相対パス（ファイルまたはフォルダ）。省略時は作業フォルダ全体",
                                     "Relative path (file or folder) to narrow the search. Defaults to the whole working folder")
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": d("大文字小文字を無視するか。既定は false",
                                     "Ignore case. Defaults to false")
                },
                "count_only": {
                    "type": "boolean",
                    "description": d("true で一致行を返さず件数だけを返す。既定は false",
                                     "If true, return only counts instead of matching lines. Defaults to false")
                },
                "include": {
                    "type": "string",
                    "description": d("対象を**ファイル名**で絞る正規表現（**glob ではない**。例: `\\.rs$`）。省略時は全ファイル",
                                     "Regex that filters by **file name** (**not a glob**; e.g. `\\.rs$`). Defaults to all files")
                },
                "context": {
                    "type": "integer",
                    "description": d("一致行の前後も返す行数（0〜3、既定 0）。一致行は `:`、前後の行は `-` で区切られる",
                                     "How many surrounding lines to return (0-3, default 0). Matches are marked `:`, context lines `-`")
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
        let count_only = args
            .get("count_only")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let include = args
            .get("include")
            .and_then(Value::as_str)
            .map(str::to_owned);
        // 負値は `as_u64` が弾く。**clamp せずエラー**にするのは `run_grep` の側。
        let Some(context) = args
            .get("context")
            .map_or(Some(0usize), |value| value.as_u64().map(|n| n as usize))
        else {
            return Ok(format!(
                "`context` は 0〜{MAX_CONTEXT} の整数で指定してください。"
            ));
        };

        let query = GrepQuery {
            pattern,
            rel_path,
            case_insensitive,
            count_only,
            include,
            context,
            agent_id: ctx.agent_id.clone(),
        };

        // 走査は I/O + CPU の塊なので Rayon 側へ逃がし、Tokio ワーカーを空ける。
        spawn_rayon(move || run_grep(&work_dir, &query)).await
    }
}

/// `context` の上限（Spec 16 D3）。`diff` の unified diff と同じ値。
const MAX_CONTEXT: usize = 3;

/// `include` のコンパイル。**glob 形の誤りを名指しで返す**（Spec 16 D1）。
///
/// モデルは習慣で `*.rs` と書く。これは Rust regex として**コンパイルに失敗する**
/// ので、空振り（一致ゼロ）ではなく**その場で直せる文面**を返せる。
/// 空振りで返すと、モデルはパターンの側を疑って再検索し周が増える。
fn compile_include(pattern: &str, case_insensitive: bool) -> Result<regex::Regex, String> {
    regex::RegexBuilder::new(pattern)
        .case_insensitive(case_insensitive)
        // `pattern` と同じ上限。片方だけ緩めない。
        .size_limit(1 << 20)
        .build()
        .map_err(|err| {
            if pattern.starts_with('*') || pattern.contains("*.") {
                format!(
                    "`include` は glob ではなく正規表現です。\
                     `*.rs` ではなく `\\.rs$` と書いてください（元のエラー: {err}）"
                )
            } else {
                format!("`include` を正規表現として解釈できません: {err}")
            }
        })
}

/// 一致行の集合から、前後 `context` 行を含む窓を作って重なりを畳む。
///
/// **同じファイルの中だけで畳む**（Spec 16 D8）。跨ぐと行番号の意味が変わる。
/// 戻りは 0 始まりの行番号の閉区間で、開始位置の昇順。
fn merge_windows(matches: &[usize], context: usize, line_count: usize) -> Vec<(usize, usize)> {
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for &line in matches {
        let start = line.saturating_sub(context);
        let end = (line + context).min(line_count.saturating_sub(1));
        match merged.last_mut() {
            // 隣接（+1）も畳む — 間に 1 行だけ挟んで区切ると読みにくいだけ。
            Some(last) if start <= last.1 + 1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

/// ファイル別の件数を「多い順」に並べる。同数はパス順で安定させる。
fn format_per_file(per_file: &mut [(String, usize)]) -> String {
    per_file.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let shown = per_file.len().min(MAX_COUNT_FILES);
    let mut out = String::from("ファイル別の件数:\n");
    for (path, count) in &per_file[..shown] {
        out.push_str(&format!("  {path}: {count} 件\n"));
    }
    if per_file.len() > shown {
        out.push_str(&format!(
            "  （ほか {} ファイル。件数の多い順に {shown} 件だけ表示しています）\n",
            per_file.len() - shown
        ));
    }
    out
}

/// grep 本体。ブロッキングして良い文脈で呼ぶ。
/// `grep` の 1 回の呼び出し。
///
/// 引数を束ねているのは、`include` / `context` を足した時点で `run_grep` の
/// 引数が 8 本になったため（`too_many_arguments`）。抑制で黙らせず、
/// 「1 回の問い」という単位を型にした。
struct GrepQuery {
    pattern: String,
    rel_path: String,
    case_insensitive: bool,
    count_only: bool,
    include: Option<String>,
    context: usize,
    agent_id: crate::model::AgentId,
}

fn run_grep(work_dir: &Path, query: &GrepQuery) -> String {
    let GrepQuery {
        pattern,
        rel_path,
        case_insensitive,
        count_only,
        include,
        context,
        agent_id,
    } = query;
    let (case_insensitive, count_only, context) = (*case_insensitive, *count_only, *context);
    let (pattern, rel_path) = (pattern.as_str(), rel_path.as_str());
    let include = include.as_deref();
    // **範囲外は clamp せずエラー**（Spec 16 D7）。丸めると、指定した値と効いた値が
    // 食い違ったまま出力が返り、モデルは 4 行分の文脈を見たつもりで読む。
    if context > MAX_CONTEXT {
        return format!("`context` は 0〜{MAX_CONTEXT} で指定してください（指定値: {context}）。");
    }

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

    // **`include` を使った呼び出しは、結果に関わらず 1 行残す**（2026-08-05）。
    //
    // `tool:` 行は `args_chars` しか書かないので、**引数の中身はどの計器にも
    // 載っていない** — glob 形（`*.rs`）を書いたか、名指しの拒否を読んで
    // `\.rs$` へ直したかが実機のログから読めなかった。`run decision:` を足したのと
    // 同じ判断で、**機構が動いているかどうかは動いた記録が無ければ確かめられない**
    // （`failures.md` #58 の同型）。
    //
    // 出すのは `include` があるときだけ。省略した呼び出しでは 1 行も増えない。
    let include = match include {
        None => None,
        Some(source) => {
            let compiled = compile_include(source, case_insensitive);
            let outcome = match &compiled {
                Ok(_) => "ok",
                // 予測できる誤り（glob 形）と、それ以外の構文エラーを分ける。
                Err(_) if source.starts_with('*') || source.contains("*.") => "glob",
                Err(_) => "invalid",
            };
            crate::note!(
                "grep include: agent={} pattern={} outcome={outcome}",
                agent_id,
                clip_line(source)
            );
            match compiled {
                Ok(regex) => Some(regex),
                Err(message) => return message,
            }
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

    // **`include` は walk の後・read の前**（Spec 16 Notes 1）。
    // read は減るが walk は減らないので、`MAX_FILES` の打ち切りには効かない。
    let files: Vec<PathBuf> = match &include {
        None => files,
        Some(regex) => files
            .into_iter()
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| regex.is_match(name))
            })
            .collect(),
    };

    let root = work_dir.canonicalize().unwrap_or_else(|_| work_dir.to_path_buf());
    let mut lines: Vec<String> = Vec::new();
    let mut per_file: Vec<(String, usize)> = Vec::new();
    let mut total_matches = 0usize;
    // 表示した**一致行**の数。文脈行は数えない（Spec 16 D4）。
    let mut shown_matches = 0usize;
    let mut output_chars = 0usize;
    let mut clipped = false;

    for file in &files {
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

        let file_lines: Vec<&str> = text.lines().collect();
        let hits: Vec<usize> = file_lines
            .iter()
            .enumerate()
            .filter(|(_, line)| regex.is_match(line))
            .map(|(index, _)| index)
            .collect();

        // **表示は止めても数えるのは止めない。** 走査そのものは一致ゼロの
        // grep でも全ファイルに及ぶので、数え続けても最悪コストは変わらない。
        // 表示件数を総数として返すと「何件あるか」を読み手が誤り、
        // 確かめるための grep が周を食う（一致行の列挙と件数の集計は別の問い）。
        total_matches += hits.len();
        if !hits.is_empty() {
            per_file.push((display.clone(), hits.len()));
        }
        if count_only || clipped || hits.is_empty() {
            continue;
        }

        // **数えるのは一致行だけ。文脈行は文字数の枠だけを食う**（Spec 16 D4）。
        let room = MAX_MATCHES.saturating_sub(shown_matches);
        if room == 0 || output_chars > MAX_OUTPUT_CHARS {
            clipped = true;
            continue;
        }
        let shown_hits = &hits[..room.min(hits.len())];
        if shown_hits.len() < hits.len() {
            clipped = true;
        }

        // 窓は**このファイルの中だけ**で畳む（Spec 16 D8）。
        let windows = merge_windows(shown_hits, context, file_lines.len());
        let mut previous_end: Option<usize> = None;
        for (start, end) in windows {
            // `--` は context > 0 のとき、**同じファイル内で離れた窓の間**だけ。
            // ファイルが変わるところでは出さない（パスが行頭にあり既に区切れている）。
            if context > 0
                && let Some(last_end) = previous_end
                && start > last_end + 1
            {
                lines.push("--".to_owned());
                output_chars += 2;
            }
            for (offset, line) in file_lines[start..=end].iter().enumerate() {
                let index = start + offset;
                let is_hit = shown_hits.binary_search(&index).is_ok();
                let separator = if is_hit { ':' } else { '-' };
                // 文脈行にも 240 字のクリップを掛ける（1 本の長い行で枠が飛ばないように）。
                let entry = format!("{display}:{}{separator} {}", index + 1, clip_line(line));
                output_chars += entry.chars().count();
                lines.push(entry);
                if is_hit {
                    shown_matches += 1;
                }
            }
            previous_end = Some(end);
            // **打ち切りは窓の境界で行う。** ブロックの途中では切らない
            // （Spec 17 D3 と同じ — 切ってよいのは一覧であって中身ではない）。
            if output_chars > MAX_OUTPUT_CHARS {
                clipped = true;
                break;
            }
        }
    }

    if total_matches == 0 {
        return format!(
            "一致なし（{} ファイルを走査）。パターンや範囲を変えて再試行できます。",
            files.len()
        );
    }

    let files_hit = per_file.len();
    let mut out = String::new();
    if count_only {
        out.push_str(&format!("全 {total_matches} 件が一致（{files_hit} ファイル）。\n"));
        // **黙って無視しない**（Spec 16 S3）。指定した引数が効かなかったことは、
        // 指定した側に伝わらないと次の手が選べない。拒否にしないのは、件数は
        // 正しく返せるのにその周の答えが無くなるため。
        if context > 0 {
            out.push_str("（`count_only` では行を返さないため `context` は無視しました）\n");
        }
        out.push_str(&format_per_file(&mut per_file));
    } else if clipped {
        out.push_str(&format!(
            "全 {total_matches} 件が一致（{files_hit} ファイル）。うち先頭 {shown_matches} 件を表示:\n"
        ));
        out.push_str(&lines.join("\n"));
        // **表示の打ち切り。** ここでは `include` が効く（read するファイルが減る）。
        out.push_str(
            "\n（表示上限に達したため一致行を打ち切りました。総数は上の数字が正しい。\
             行が要らないなら `count_only: true`、絞るなら `include` でファイル名を、\
             `path` で場所を、またはパターンを具体化してください",
        );
        if context > 0 {
            out.push_str("。`context` を下げると表示できる一致が増えます");
        }
        out.push_str("）\n");
        out.push_str(&format_per_file(&mut per_file));
    } else {
        out.push_str(&format!("{total_matches} 件が一致:\n"));
        out.push_str(&lines.join("\n"));
    }
    if files_truncated {
        // **走査の打ち切り。`include` は効かない**（Spec 16 D6）— 名前で絞るのは
        // read の前だが walk の後なので、走査するファイル数そのものは減らない。
        // ここで `include` を案内すると、直らない方法を試させることになる。
        out.push_str(&format!(
            "\n（ファイル数が {MAX_FILES} を超えたため、一部は走査していません。\
             `path` で場所を絞ってください。**`include` では直りません** — \
             名前での絞り込みは読むファイルを減らすだけで、走査するファイルは減りません）"
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

    fn description(&self, language: crate::world::Language) -> String {
        language
            .pick(
                "作業フォルダ内のファイル・フォルダを**名前**で探す。\
                 **ファイルの場所や存在を知りたいときは、中身を検索する grep より先にこれを使うこと。**\
                 パターンは名前（パスの最後の要素）に対する正規表現で、\
                 結果は相対パスの一覧（フォルダは末尾 `/`）。\
                 探せるのはエージェント設定で指定された作業フォルダの中だけ。",
                "Find files and folders in the working folder **by name**. \
                 **When you want to know where a file is or whether it exists, use \
                 this before grep (which searches contents).** The pattern is a \
                 regex applied to the name (the last path segment); results are a \
                 list of relative paths (folders end with `/`). \
                 Only the working folder configured for this agent is searchable.",
            )
            .to_owned()
    }

    fn parameters(&self, language: crate::world::Language) -> Value {
        let d = |ja: &str, en: &str| language.pick(ja, en).to_owned();
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": d("名前に対する正規表現（Rust regex 構文）。例: `\\.md$`、`^config`",
                                     "Regex applied to the name (Rust regex syntax), e.g. `\\.md$`, `^config`")
                },
                "path": {
                    "type": "string",
                    "description": d("探索範囲を絞る相対パス（フォルダ）。省略時は作業フォルダ全体",
                                     "Relative folder path to narrow the search. Defaults to the whole working folder")
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": d("大文字小文字を無視するか。既定は true（名前検索は表記揺れが多い）",
                                     "Ignore case. Defaults to true (file names vary in casing)")
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

    fn description(&self, language: crate::world::Language) -> String {
        language
            .pick(
                "作業フォルダ内の 2 つのファイルの差分を unified diff 形式で返す。\
                 **2 つのファイルの違いを知りたいとき、両方の全文を読む代わりに使うこと** — \
                 変わった行だけが返るので速くて安い。\
                 比較できるのはエージェント設定で指定された作業フォルダの中だけ。",
                "Return the difference between two files in the working folder as a \
                 unified diff. **When you want to know how two files differ, use this \
                 instead of reading both in full** — only changed lines come back, so \
                 it is fast and cheap. \
                 Only the working folder configured for this agent is comparable.",
            )
            .to_owned()
    }

    fn parameters(&self, language: crate::world::Language) -> Value {
        let d = |ja: &str, en: &str| language.pick(ja, en).to_owned();
        serde_json::json!({
            "type": "object",
            "properties": {
                "old_path": {
                    "type": "string",
                    "description": d("比較元ファイルの相対パス", "Relative path of the old file")
                },
                "new_path": {
                    "type": "string",
                    "description": d("比較先ファイルの相対パス", "Relative path of the new file")
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
                "fuseforks-fs-{tag}-{}",
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
            cancel: None,
            rag_roots: Vec::new(),
            language: crate::world::Language::Ja,
        }
    }

    // -- relative_file_paths（Spec 24。補完の候補源）----------------------------

    /// 補完へ渡す一覧は**相対・`/` 区切り・ソート済み**。
    ///
    /// 3 つとも崩れると、補完の順位が打鍵ごとに揺れるか、挿した文字列が
    /// そのままツール引数にならなくなる（同梱ツールの `path` は相対パス）。
    #[test]
    fn relative_paths_are_relative_slashed_and_sorted() {
        let dir = TempDir::new("rel-paths");
        dir.write("b.txt", "b");
        dir.write("a.txt", "a");
        dir.write("sub/deep/c.txt", "c");
        // 隠しフォルダは走査の規律どおり落ちる（補完も同じ集合を使う）。
        dir.write(".git/HEAD", "ref");

        let (paths, truncated) = relative_file_paths(&dir.0);

        assert!(!truncated);
        assert_eq!(paths, vec!["a.txt", "b.txt", "sub/deep/c.txt"]);
        assert!(
            paths.iter().all(|p| !p.contains('\\')),
            "区切りは / に揃えること: {paths:?}"
        );
    }

    /// 読めないフォルダは**空の一覧**（例外にしない）。
    ///
    /// 壊れた作業フォルダは `resolve_in_work_dir` が名指しで報告するので、
    /// ここに 2 つ目の報告経路を作らない。
    #[test]
    fn an_unreadable_root_yields_an_empty_listing() {
        let dir = TempDir::new("rel-missing");
        let (paths, truncated) = relative_file_paths(&dir.0.join("does-not-exist"));
        assert!(paths.is_empty());
        assert!(!truncated);
    }

    // -- resolve_creatable（Spec 09。実在しない宛先の境界検査）------------------

    #[test]
    fn creatable_accepts_a_new_path_under_an_existing_ancestor() {
        let dir = TempDir::new("creatable-new");
        dir.write("src/main.rs", "fn main() {}\n");

        let (path, display) = resolve_creatable(&dir.0, "src/deep/new.txt").unwrap();
        assert!(!path.exists(), "まだ存在しない宛先を通すこと");
        assert!(path.starts_with(dir.0.canonicalize().unwrap()));
        assert_eq!(display, "src/deep/new.txt");
    }

    #[test]
    fn creatable_accepts_an_existing_path_too() {
        // move / copy の宛先は「既にある」こともある（上書き可否は呼び出し側の判断）。
        let dir = TempDir::new("creatable-existing");
        dir.write("a.txt", "x");

        let (path, display) = resolve_creatable(&dir.0, "a.txt").unwrap();
        assert!(path.exists());
        assert_eq!(display, "a.txt");
    }

    #[test]
    fn creatable_rejects_parent_traversal_even_when_the_ancestor_is_inside() {
        // **この関数の要**。祖先（src/）は囲いの中なので 1 段目の検査は通る。
        // 残り成分の `..` を見ていないと、ここから外へ出られる。
        let dir = TempDir::new("creatable-escape");
        dir.write("src/main.rs", "fn main() {}\n");

        for attempt in [
            "src/../../escape.txt",
            "../escape.txt",
            "src/../../../tmp/escape.txt",
            "src/sub/../../../escape.txt",
        ] {
            let err = resolve_creatable(&dir.0, attempt)
                .expect_err(&format!("`{attempt}` は拒否されること"));
            assert!(err.contains("作業フォルダの外"), "{attempt}: {err}");
        }
    }

    #[test]
    fn creatable_rejects_absolute_destinations() {
        let dir = TempDir::new("creatable-absolute");
        // join は絶対パスで root を丸ごと置き換える（Path::join の仕様）ので、
        // 表記を数え上げずに結果で弾く。
        let absolute = std::env::temp_dir().join("fuseforks-escape.txt");
        let err = resolve_creatable(&dir.0, &absolute.to_string_lossy())
            .expect_err("絶対パスは拒否されること");
        assert!(err.contains("作業フォルダの外"), "{err}");
    }

    #[test]
    fn creatable_rejects_an_empty_path() {
        let dir = TempDir::new("creatable-empty");
        for attempt in ["", "   ", "."] {
            assert!(
                resolve_creatable(&dir.0, attempt).is_err(),
                "`{attempt}` は宛先として拒否されること"
            );
        }
    }

    #[test]
    fn creatable_rejects_a_symlinked_destination() {
        let dir = TempDir::new("creatable-symlink");
        let outside = TempDir::new("creatable-outside");
        outside.write("secret.txt", "秘密");

        // Windows の symlink 作成は権限が要る。作れない環境では検証を諦めるが、
        // **黙って通さない** — 何を確かめられなかったかを出力に残す。
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(outside.0.join("secret.txt"), dir.0.join("link.txt"));
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(outside.0.join("secret.txt"), dir.0.join("link.txt"));

        if made.is_err() {
            eprintln!(
                "[test] symlink を作れないため creatable_rejects_a_symlinked_destination は未検証"
            );
            return;
        }

        let err = resolve_creatable(&dir.0, "link.txt").expect_err("symlink の宛先は拒否されること");
        assert!(err.contains("作業フォルダの外"), "{err}");
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
    async fn grep_reports_the_true_total_when_it_clips() {
        let dir = TempDir::new("total");
        let body: String = (0..500).map(|i| format!("needle {i}\n")).collect();
        dir.write("big.txt", &body);

        let reply = GrepTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({ "pattern": "needle" }),
            )
            .await
            .unwrap();

        assert!(
            reply.contains("500"),
            "打ち切っても総数を書くこと（表示件数だけでは何件あるか分からない）: {reply}"
        );
    }

    #[tokio::test]
    async fn grep_breaks_the_count_down_by_file_when_it_clips() {
        let dir = TempDir::new("perfile");
        dir.write("many.txt", &(0..300).map(|i| format!("needle {i}\n")).collect::<String>());
        dir.write("few.txt", "needle a\nneedle b\n");

        let reply = GrepTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({ "pattern": "needle" }),
            )
            .await
            .unwrap();

        assert!(reply.contains("many.txt: 300"), "ファイル別の件数を書くこと: {reply}");
        assert!(reply.contains("few.txt: 2"), "打ち切りの外のファイルも数えること: {reply}");
    }

    #[tokio::test]
    async fn grep_count_only_returns_totals_without_matching_lines() {
        let dir = TempDir::new("countonly");
        dir.write("a.txt", "needle one\nneedle two\n");

        let reply = GrepTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({ "pattern": "needle", "count_only": true }),
            )
            .await
            .unwrap();

        assert!(reply.contains("a.txt: 2"), "ファイル別の件数を書くこと: {reply}");
        assert!(
            !reply.contains("needle one"),
            "集計モードでは一致行そのものを返さないこと: {reply}"
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

    // ---- include / context（Spec 16） --------------------------------------

    /// `include` はファイル名で対象を絞る。
    #[tokio::test]
    async fn include_narrows_the_scan_to_matching_file_names() {
        let dir = TempDir::new("grep-include");
        dir.write("a.rs", "needle in code\n");
        dir.write("b.md", "needle in prose\n");

        let reply = GrepTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({ "pattern": "needle", "include": "\\.rs$" }),
            )
            .await
            .unwrap();

        assert!(reply.contains("a.rs"), "{reply}");
        assert!(!reply.contains("b.md"), "台帳側が混じっている: {reply}");
        assert!(reply.contains("1 件が一致"), "{reply}");
    }

    /// **`include` の一致対象は名前だけ。** フォルダ名に一致しても配下は巻き込まない。
    #[tokio::test]
    async fn include_matches_the_file_name_not_the_path() {
        let dir = TempDir::new("grep-include-dir");
        // `run.json` という名前の**フォルダ**の下にファイルを置く。
        dir.write("run.json/inside.txt", "needle\n");
        dir.write("other.txt", "needle\n");

        let reply = GrepTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({ "pattern": "needle", "include": "run\\.json" }),
            )
            .await
            .unwrap();

        assert!(reply.contains("一致なし"), "配下を巻き込んでいる: {reply}");
    }

    /// **glob 形の誤りは名指しで返す**（Spec 16 D1）。
    ///
    /// 空振り（一致ゼロ）で返すと、モデルはパターンの側を疑って再検索し周が増える。
    #[tokio::test]
    async fn a_glob_shaped_include_is_named_rather_than_silently_missing() {
        let dir = TempDir::new("grep-include-glob");
        dir.write("a.rs", "needle\n");

        let reply = GrepTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({ "pattern": "needle", "include": "*.rs" }),
            )
            .await
            .unwrap();

        assert!(reply.contains("glob ではなく正規表現"), "{reply}");
        assert!(reply.contains("\\.rs$"), "直し方が書かれていない: {reply}");
    }

    /// `context` で前後の行が `-` 付きで返る。**一致行の `:` は変わらない。**
    #[tokio::test]
    async fn context_returns_neighbouring_lines_marked_with_a_dash() {
        let dir = TempDir::new("grep-context");
        dir.write("a.txt", "one\ntwo\nneedle\nfour\nfive\n");

        let reply = GrepTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({ "pattern": "needle", "context": 1 }),
            )
            .await
            .unwrap();

        assert!(reply.contains("a.txt:3: needle"), "一致行は `:`: {reply}");
        assert!(reply.contains("a.txt:2- two"), "前の行は `-`: {reply}");
        assert!(reply.contains("a.txt:4- four"), "後の行は `-`: {reply}");
        assert!(!reply.contains("one"), "窓の外は出ないこと: {reply}");
    }

    /// **範囲外の `context` はエラー。clamp しない**（Spec 16 D7）。
    #[tokio::test]
    async fn an_out_of_range_context_is_an_error_rather_than_clamped() {
        let dir = TempDir::new("grep-context-range");
        dir.write("a.txt", "needle\n");

        let too_big = GrepTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({ "pattern": "needle", "context": 4 }),
            )
            .await
            .unwrap();
        assert!(too_big.contains("0〜3 で指定"), "{too_big}");
        assert!(!too_big.contains("needle"), "走らせないこと: {too_big}");

        let negative = GrepTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({ "pattern": "needle", "context": -1 }),
            )
            .await
            .unwrap();
        assert!(negative.contains("0〜3"), "{negative}");
    }

    /// 重なった窓は畳み、同じ行を 2 回出さない（Spec 16 D5）。
    #[tokio::test]
    async fn overlapping_windows_are_merged_without_repeating_a_line() {
        let dir = TempDir::new("grep-merge");
        dir.write("a.txt", "one\nneedle\nmiddle\nneedle\nfive\n");

        let reply = GrepTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({ "pattern": "needle", "context": 1 }),
            )
            .await
            .unwrap();

        assert_eq!(
            reply.matches("a.txt:3- middle").count(),
            1,
            "重なった行が 2 回出ている: {reply}"
        );
        assert!(!reply.contains("--"), "畳んだので区切りは要らない: {reply}");
    }

    /// **`--` は同じファイル内で離れた窓の間だけ**（Spec 16 D8）。
    ///
    /// ファイルが変わるところでは出さない（パスが行頭にあり既に区切れている）。
    #[tokio::test]
    async fn the_separator_appears_only_between_distant_windows_of_one_file() {
        let dir = TempDir::new("grep-separator");
        dir.write(
            "a.txt",
            "needle\nx\nx\nx\nx\nx\nneedle\n",
        );
        dir.write("b.txt", "needle\n");

        let split = GrepTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({ "pattern": "needle", "context": 1, "path": "a.txt" }),
            )
            .await
            .unwrap();
        assert!(split.contains("\n--\n"), "離れた窓の間には出ること: {split}");

        let across = GrepTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({ "pattern": "needle", "context": 0 }),
            )
            .await
            .unwrap();
        assert!(
            !across.contains("--"),
            "context 0 とファイル跨ぎでは出さないこと: {across}"
        );
    }

    /// **`context` を変えても総数は変わらない**（Spec 16 D4）。
    ///
    /// 数えるのは一致行だけで、文脈行は文字数の枠しか食わない。
    #[tokio::test]
    async fn context_does_not_change_the_reported_total() {
        let dir = TempDir::new("grep-total");
        dir.write("a.txt", "one\nneedle\nthree\nfour\nneedle\nsix\n");

        let bare = GrepTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({ "pattern": "needle" }),
            )
            .await
            .unwrap();
        let padded = GrepTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({ "pattern": "needle", "context": 1 }),
            )
            .await
            .unwrap();

        assert!(bare.contains("2 件が一致"), "{bare}");
        assert!(padded.contains("2 件が一致"), "{padded}");
    }

    /// ファイルの端で窓がはみ出しても落ちない。
    #[tokio::test]
    async fn a_window_at_either_edge_of_a_file_is_clamped_safely() {
        let dir = TempDir::new("grep-edge");
        dir.write("a.txt", "needle\n");

        let reply = GrepTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({ "pattern": "needle", "context": 3 }),
            )
            .await
            .unwrap();

        assert!(reply.contains("a.txt:1: needle"), "{reply}");
    }

    /// **`count_only` は `include` で絞った結果を数える。**
    ///
    /// `context` は集計モードでは出力に現れない（返すのは件数だけなので、
    /// 無視されたことが結果から読める）。
    #[tokio::test]
    async fn count_only_counts_within_the_include_filter() {
        let dir = TempDir::new("grep-count-include");
        dir.write("a.rs", "needle\nneedle\n");
        dir.write("b.md", "needle\n");

        let reply = GrepTool
            .call(
                &ctx_with(Some(&dir.0)),
                &serde_json::json!({
                    "pattern": "needle", "include": "\\.rs$",
                    "count_only": true, "context": 2
                }),
            )
            .await
            .unwrap();

        assert!(reply.contains("全 2 件が一致"), "{reply}");
        assert!(!reply.contains("b.md"), "{reply}");
        assert!(!reply.contains("needle"), "行は返さないこと: {reply}");
        // **黙って無視しない**（Spec 16 S3）。
        assert!(
            reply.contains("`context` は無視しました"),
            "無視した旨が出ていない: {reply}"
        );
    }
}
