//! 同梱 `rag` ツール（Spec 18）— 宣言されたフォルダへの見出し索引。
//!
//! Markdown の見出し階層（人が置いた意味の切れ目）を索引として引く。
//! 純機構（見出しの抽出・木・節解決）は [`crate::doc_index`] に住み、
//! この層は走査・囲い・出力の有界化だけを担う。
//!
//! # 囲い（rag_tool_contract）
//!
//! 読めるのは **人が `rag_sources` に宣言したフォルダの中だけ**。work_dir とは
//! 独立の境界で、内外を問わない — 囲いの単位は領域ではなく**宣言**。
//! 各ルートの解決は canonicalize + 前方一致・symlink 不追従
//! （`resolve_in_work_dir` と同じ規律の別関数。あれは work_dir 起点の関数で、
//! 流用すると「新規作成なし」の担保を持つ側の意味が濁る）。
//! 書き込みの経路は無い — このツールは `std::fs::read_to_string` しか呼ばない。

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::Value;

use crate::doc_index::{
    HeadingNode, SectionResolution, build_tree, parse_headings, resolve_section, section_bounds,
    section_path_for_line,
};
use crate::error::CoreResult;
use crate::llm::ToolSpec;
use crate::tool::{AgentTool, ToolContext};
use crate::tools::fs::{MAX_FILE_BYTES, MAX_FILES, MAX_OUTPUT_CHARS, clip_line, collect_files};

/// 宣言されたルートの検査結果。無効なルートは**無効化であって削除ではない** —
/// 印を付けるだけで宣言は残り、パスを直せば次の呼び出しから復活する。
struct CheckedRoots {
    /// canonicalize に成功した（= 実在する）ルート。表示は宣言どおりの絶対パス。
    valid: Vec<(PathBuf, String)>,
    /// 実在しなかった宣言（表示用）。
    invalid: Vec<String>,
}

/// 宣言されたルートを検査する。呼び出しごとに掛け直す — 起動時に固定すると、
/// 開いたまま外付けドライブを外した村が素通りし、繋ぎ直しても復活しない。
fn check_roots(declared: &[PathBuf]) -> CheckedRoots {
    let mut valid = Vec::new();
    let mut invalid = Vec::new();
    for root in declared {
        match root.canonicalize() {
            Ok(canonical) if canonical.is_dir() => {
                valid.push((canonical, root.display().to_string()));
            }
            _ => invalid.push(root.display().to_string()),
        }
    }
    CheckedRoots { valid, invalid }
}

/// Markdown ファイルか（拡張子 md / markdown、大文字小文字は無視）。
fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|s| s.eq_ignore_ascii_case("md") || s.eq_ignore_ascii_case("markdown"))
        .unwrap_or(false)
}

/// ルート起点の表示用相対パス。
fn rel_display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().into_owned())
}

/// `path` 引数を宣言されたルート群の中で解決する。
///
/// - 絶対パス: canonicalize してどれかのルートの中なら通す
/// - 相対パス: 各ルートを起点に試し、**ちょうど 1 つ**で見つかれば通す。
///   複数で見つかれば曖昧として拒否（黙って 1 つ選ばない — 節解決と同じ規律）
///
/// 戻りは (解決済み絶対パス, そのルート, 表示用相対パス)。
fn resolve_in_roots(
    roots: &[(PathBuf, String)],
    user_path: &str,
) -> Result<(PathBuf, PathBuf, String), String> {
    let candidate = Path::new(user_path);
    if candidate.is_absolute() {
        let resolved = candidate
            .canonicalize()
            .map_err(|_| format!("`{user_path}` が見つかりません。"))?;
        for (root, _) in roots {
            if resolved.starts_with(root) {
                let display = rel_display(root, &resolved);
                return Ok((resolved, root.clone(), display));
            }
        }
        return Err(format!(
            "`{user_path}` は宣言された資料フォルダの外を指しています。\
             読めるのは設定で宣言されたフォルダの中だけです。"
        ));
    }

    let mut hits: Vec<(PathBuf, PathBuf, String)> = Vec::new();
    for (root, root_display) in roots {
        let joined = root.join(user_path);
        let Ok(resolved) = joined.canonicalize() else {
            continue;
        };
        // symlink で外へ出る経路は canonicalize + 前方一致が落とす。
        if resolved.starts_with(root) {
            let display = rel_display(root, &resolved);
            hits.push((resolved, root.clone(), format!("{root_display} の {display}")));
        }
    }
    match hits.len() {
        0 => Err(format!("`{user_path}` は宣言された資料フォルダの中に見つかりません。")),
        1 => {
            let (resolved, root, _) = hits.into_iter().next().expect("len == 1");
            let display = rel_display(&root, &resolved);
            Ok((resolved, root, display))
        }
        _ => {
            let candidates: Vec<String> = hits.into_iter().map(|(_, _, d)| d).collect();
            Err(format!(
                "`{user_path}` は複数の資料フォルダに存在して曖昧です: {}。\
                 絶対パスで指定してください。",
                candidates.join(" / ")
            ))
        }
    }
}

/// 見出し木を 1 ファイルぶんのテキストへ描く（インデントで階層を示す）。
fn render_tree(nodes: &[HeadingNode], depth: usize, out: &mut String) {
    for node in nodes {
        out.push_str(&"  ".repeat(depth));
        out.push_str(&format!("{} {}（{} 行目）\n", "#".repeat(node.level as usize), node.title, node.line));
        render_tree(&node.children, depth + 1, out);
    }
}

/// 1 ファイルの outline ブロックを作る。読めない・大きすぎるときは 1 行の注記。
fn outline_block(root: &Path, file: &Path) -> String {
    let display = rel_display(root, file);
    let too_big = std::fs::metadata(file).map(|m| m.len() > MAX_FILE_BYTES).unwrap_or(true);
    if too_big {
        return format!("## {display}\n（2 MiB を超えるため見出しを読んでいません）\n");
    }
    let Ok(text) = std::fs::read_to_string(file) else {
        return format!("## {display}\n（読めませんでした）\n");
    };
    let tree = build_tree(parse_headings(&text));
    if tree.is_empty() {
        return format!("## {display}\n（見出しがありません）\n");
    }
    let mut block = format!("## {display}\n");
    render_tree(&tree, 0, &mut block);
    block
}

/// `outline` — フォルダの一覧 + 各 Markdown の見出し木。
///
/// 打ち切りは**ファイルの境界**（rag_tool_contract）— 見出し木が途中で終わると、
/// モデルは無い節を「無い」と読む。
fn run_outline(roots: &CheckedRoots, path: Option<&str>) -> String {
    // path があればそこへ絞る（ファイルなら 1 本、フォルダなら配下）。
    let targets: Vec<(PathBuf, PathBuf)> = match path {
        Some(p) => match resolve_in_roots(&roots.valid, p) {
            Ok((resolved, root, _)) => vec![(root, resolved)],
            Err(reason) => return reason,
        },
        None => roots.valid.iter().map(|(r, _)| (r.clone(), r.clone())).collect(),
    };

    let mut out = String::new();
    let mut shown = 0usize;
    let mut skipped = 0usize;
    let mut walk_truncated = false;

    for (root, target) in targets {
        let files: Vec<PathBuf> = if target.is_file() {
            vec![target]
        } else {
            let (all, truncated) = collect_files(&target);
            walk_truncated |= truncated;
            all.into_iter().filter(|f| is_markdown(f)).collect()
        };
        if files.is_empty() {
            continue;
        }
        out.push_str(&format!("# {}\n", root.display()));
        for file in files {
            let block = outline_block(&root, &file);
            // ファイルの境界でだけ止める。半分だけの見出し木は出さない。
            if out.chars().count() + block.chars().count() > MAX_OUTPUT_CHARS {
                skipped += 1;
                continue;
            }
            out.push_str(&block);
            shown += 1;
        }
    }

    if shown == 0 && skipped == 0 {
        out.push_str("Markdown ファイルが見つかりませんでした。\n");
    }
    if skipped > 0 {
        out.push_str(&format!(
            "\n（上限 {MAX_OUTPUT_CHARS} 字に達したため、残り {skipped} ファイルの見出しは\
             表示していません。`path` で範囲を絞ってください）\n"
        ));
    }
    if walk_truncated {
        out.push_str(&format!("（ファイル数が {MAX_FILES} を超えたため、一部は走査していません）\n"));
    }
    append_invalid_note(&mut out, roots);
    out
}

/// `search` — 一致行 + その行が属する見出しの経路。
///
/// 打ち切りは**一致行の境界**（grep と同じ）。`run_grep` は共有しない —
/// あちらは「`grep` の 1 回の問い」という単位で `grep include:` の計器を持ち、
/// 借りると rag の呼び出しが grep の計器に混ざる（Spec 18 D8）。
fn run_search(roots: &CheckedRoots, pattern: &str, path: Option<&str>, case_insensitive: bool) -> String {
    let regex = match regex::RegexBuilder::new(pattern)
        .case_insensitive(case_insensitive)
        // `grep` と同じ上限。片方だけ緩めない。
        .size_limit(1 << 20)
        .build()
    {
        Ok(regex) => regex,
        Err(err) => {
            return format!("正規表現として解釈できません: {err}\nパターンを直して再試行してください。");
        }
    };

    let targets: Vec<(PathBuf, PathBuf)> = match path {
        Some(p) => match resolve_in_roots(&roots.valid, p) {
            Ok((resolved, root, _)) => vec![(root, resolved)],
            Err(reason) => return reason,
        },
        None => roots.valid.iter().map(|(r, _)| (r.clone(), r.clone())).collect(),
    };

    let mut out = String::new();
    let mut total = 0usize;
    let mut dropped = 0usize;
    let mut walk_truncated = false;

    for (root, target) in targets {
        let files: Vec<PathBuf> = if target.is_file() {
            vec![target]
        } else {
            let (all, truncated) = collect_files(&target);
            walk_truncated |= truncated;
            all.into_iter().filter(|f| is_markdown(f)).collect()
        };
        for file in files {
            if std::fs::metadata(&file).map(|m| m.len() > MAX_FILE_BYTES).unwrap_or(true) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&file) else { continue };
            let flat = parse_headings(&text);
            let display = rel_display(&root, &file);
            for (idx, line) in text.lines().enumerate() {
                if !regex.is_match(line) {
                    continue;
                }
                total += 1;
                let section = section_path_for_line(&flat, idx + 1);
                let section_note = if section.is_empty() {
                    String::new()
                } else {
                    format!("\n    節: {}", section.join(" > "))
                };
                let entry = format!("{display}:{}: {}{section_note}\n", idx + 1, clip_line(line));
                // 一致行の境界でだけ止める（行を半分にしない）。数えは続ける —
                // 打ち切り文に真の総数を書くため（grep の件数修復 #55 と同じ規律)。
                if out.chars().count() + entry.chars().count() > MAX_OUTPUT_CHARS {
                    dropped += 1;
                    continue;
                }
                out.push_str(&entry);
            }
        }
    }

    if total == 0 {
        out.push_str("一致はありませんでした。\n");
    }
    if dropped > 0 {
        out.push_str(&format!(
            "\n（全 {total} 件のうち {dropped} 件は上限 {MAX_OUTPUT_CHARS} 字に達したため\
             表示していません。`path` で範囲を絞るか、パターンを具体的にしてください）\n"
        ));
    }
    if walk_truncated {
        out.push_str(&format!("（ファイル数が {MAX_FILES} を超えたため、一部は走査していません）\n"));
    }
    append_invalid_note(&mut out, roots);
    out
}

/// `read` — 見出し名で指定した節の本文。
///
/// **節は途中で切らない**（rag_tool_contract）。上限を超えたら節ごと拒否し、
/// 行数・文字数と次の手を返す。`file read` が先頭 12,000 字で切り詰めるのとは
/// **意図的に逆** — 節は構造の単位で、途中で切れると「ここまでが節の内容」と
/// 読まれる。ファイルは構造の単位ではない。
fn run_read(roots: &CheckedRoots, path: &str, section: Option<&str>) -> String {
    let (resolved, _root, display) = match resolve_in_roots(&roots.valid, path) {
        Ok(hit) => hit,
        Err(reason) => return reason,
    };
    if !resolved.is_file() {
        return format!("`{display}` はファイルではありません。`outline` で構造を確かめてください。");
    }
    if std::fs::metadata(&resolved).map(|m| m.len() > MAX_FILE_BYTES).unwrap_or(true) {
        return format!("`{display}` は 2 MiB を超えるため読めません。`search` で位置を絞ってください。");
    }
    let Ok(text) = std::fs::read_to_string(&resolved) else {
        return format!("`{display}` を読めませんでした（UTF-8 のテキストではない可能性があります）。");
    };

    let Some(section_query) = section else {
        // 節指定なし = ファイル全体。節と同じ規律で、超えるなら切らずに拒否する。
        let chars = text.chars().count();
        if chars > MAX_OUTPUT_CHARS {
            return format!(
                "`{display}` は全体で {} 行・{chars} 字あり、上限 {MAX_OUTPUT_CHARS} 字を\
                 超えます。途中で切った本文は返しません — `outline` で見出しを確かめ、\
                 `section` で節を指定してください。",
                text.lines().count()
            );
        }
        return format!("`{display}`（全文）\n\n{text}");
    };

    let flat = parse_headings(&text);
    if flat.is_empty() {
        return format!("`{display}` には見出しがありません。`section` なしで全文を読んでください。");
    }
    match resolve_section(&flat, section_query) {
        SectionResolution::NotFound => {
            format!(
                "`{display}` に「{section_query}」という節はありません。\
                 `outline` で見出しの一覧を確かめてください。"
            )
        }
        SectionResolution::Ambiguous(hits) => {
            let candidates: Vec<String> = hits.iter().map(|&i| flat[i].title.clone()).collect();
            format!(
                "「{section_query}」は `{display}` の中で複数の節に一致して曖昧です: {}。\
                 より具体的な見出し名を指定してください。",
                candidates.join(" / ")
            )
        }
        SectionResolution::Found { index, mode } => {
            let lines: Vec<&str> = text.lines().collect();
            let (start, end) = section_bounds(&flat, index, lines.len());
            let body = lines[start..end].join("\n");
            let chars = body.chars().count();
            if chars > MAX_OUTPUT_CHARS {
                return format!(
                    "節「{}」は {} 行・{chars} 字あり、上限 {MAX_OUTPUT_CHARS} 字を超えます。\
                     途中で切った本文は返しません — `search` で位置を絞るか、\
                     より深い子見出しを `section` に指定してください。",
                    flat[index].title,
                    end - start
                );
            }
            let breadcrumb = section_path_for_line(&flat, flat[index].line).join(" > ");
            let mode_note = match mode {
                crate::doc_index::MatchMode::Exact => String::new(),
                other => format!("（{}で「{}」に解決）", other.as_str(), flat[index].title),
            };
            format!("`{display}` > {breadcrumb}{mode_note}\n\n{body}")
        }
    }
}

/// 無効な宣言があれば 1 行添える。黙って落とすと「宣言したのに出てこない」が
/// 画面から読めない（Spec 14 apply_to の「黙って落とさない」と同じ規律）。
fn append_invalid_note(out: &mut String, roots: &CheckedRoots) {
    if !roots.invalid.is_empty() {
        out.push_str(&format!(
            "\n（宣言された {} 件のフォルダが見つからないため対象外です: {}。\
             利用者にパスの確認を伝えてください）\n",
            roots.invalid.len(),
            roots.invalid.join(" / ")
        ));
    }
}

/// 同梱 `rag` ツール本体。状態を持たない — 宣言は毎回 [`ToolContext`] から受け、
/// 索引は呼び出しの瞬間に作る（登録の口が要らない）。
pub struct RagTool;

#[async_trait]
impl AgentTool for RagTool {
    fn name(&self) -> &str {
        "rag"
    }

    fn description(&self) -> String {
        // 個体別の実文面は spec_for が組む。ここは登録簿用の一般形。
        "宣言された資料フォルダの Markdown を見出し索引で引く。".to_owned()
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "op": {
                    "type": "string",
                    "enum": ["outline", "search", "read"],
                    "description": "outline: フォルダと見出し木の一覧 / search: 一致行と所属する節 / read: 節の本文"
                },
                "path": {
                    "type": "string",
                    "description": "対象のパス。相対なら宣言フォルダ群から解決。outline / search では省略可（全フォルダ）、read では必須"
                },
                "pattern": {
                    "type": "string",
                    "description": "search の正規表現（Rust regex 構文）"
                },
                "section": {
                    "type": "string",
                    "description": "read で読む節の見出し名。省略で全文。完全一致 → 大文字小文字無視 → 部分一致の順で解決"
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "search で大文字小文字を無視するか。既定は false"
                }
            },
            "required": ["op"],
            "additionalProperties": false
        })
    }

    /// **その個体の宣言フォルダ**を列挙する。宣言が空、または全ルートが無効なら
    /// 提示しない（2 段ゲートの 2 段目。rag_tool_contract）。
    async fn spec_for(&self, ctx: &ToolContext) -> Option<ToolSpec> {
        if ctx.rag_roots.is_empty() {
            return None;
        }
        let roots = check_roots(&ctx.rag_roots);
        if roots.valid.is_empty() {
            // 宣言はあるが 1 本も生きていない。提示しても全呼び出しが失敗する。
            return None;
        }

        let mut text = String::from(
            "資料フォルダの Markdown を**見出し索引**で引く。全文を読む前に \
             `outline` で構造を見て、`search` で当たりを付け、`read` で節だけを\
             読むこと — 一致行には所属する節の経路が付く。\
             見出しが整った文書（仕様書・規格書）で効き、走り書きでは grep と\
             大差ない。読めるのは次のフォルダの中だけ（読み取り専用）:\n",
        );
        for (_, display) in &roots.valid {
            text.push_str(&format!("- `{display}`\n"));
        }
        if !roots.invalid.is_empty() {
            text.push_str(&format!(
                "（ほか {} 件の宣言が見つからず無効になっている）\n",
                roots.invalid.len()
            ));
        }

        Some(ToolSpec {
            name: self.name().to_owned(),
            description: text,
            parameters: self.parameters(),
        })
    }

    async fn call(&self, ctx: &ToolContext, args: &Value) -> CoreResult<String> {
        if ctx.rag_roots.is_empty() {
            return Ok("資料フォルダが宣言されていないため、このツールは使えません。\
                       利用者に「エージェント設定の『参照 RAG』欄にフォルダを追加してほしい」と\
                       伝えてください。"
                .to_owned());
        }
        let roots = check_roots(&ctx.rag_roots);
        if roots.valid.is_empty() {
            return Ok(format!(
                "宣言された資料フォルダがどれも見つかりません: {}。\
                 利用者にパスの確認を伝えてください。",
                roots.invalid.join(" / ")
            ));
        }

        let op = args.get("op").and_then(Value::as_str).unwrap_or_default();
        let path = args.get("path").and_then(Value::as_str);
        Ok(match op {
            "outline" => run_outline(&roots, path),
            "search" => {
                let Some(pattern) = args.get("pattern").and_then(Value::as_str) else {
                    return Ok("`search` には `pattern`（正規表現）が必要です。".to_owned());
                };
                let ci = args.get("case_insensitive").and_then(Value::as_bool).unwrap_or(false);
                run_search(&roots, pattern, path, ci)
            }
            "read" => {
                let Some(path) = path else {
                    return Ok("`read` には `path` が必要です。`outline` でファイルを確かめてください。".to_owned());
                };
                run_read(&roots, path, args.get("section").and_then(Value::as_str))
            }
            other => format!("`op` は outline / search / read のいずれかです（指定値: `{other}`）。"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AgentId;

    struct TempDocs {
        dir: std::path::PathBuf,
    }

    impl TempDocs {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("fuseforks_rag_test_{name}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self { dir }
        }
        fn write(&self, rel: &str, content: &str) {
            let path = self.dir.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, content).unwrap();
        }
    }

    impl Drop for TempDocs {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn ctx_with(roots: Vec<PathBuf>) -> ToolContext {
        ToolContext {
            agent_id: AgentId::from("tester"),
            work_dir: None,
            cancel: None,
            rag_roots: roots,
        }
    }

    async fn call(tool: &RagTool, ctx: &ToolContext, args: Value) -> String {
        tool.call(ctx, &args).await.unwrap()
    }

    const DOC: &str = "# 仕様\n\n## 契約\n\n契約の本文です。\n\n### 凍結\n\n凍結の本文です。\n\n## 実装\n\n実装の本文です。\n";

    #[tokio::test]
    async fn presented_only_with_valid_roots() {
        let docs = TempDocs::new("gate");
        docs.write("a.md", DOC);
        let tool = RagTool;

        // 宣言が空 → 提示しない。
        assert!(tool.spec_for(&ctx_with(vec![])).await.is_none());
        // 実在しない宣言だけ → 提示しない（全滅）。
        let ghost = ctx_with(vec![PathBuf::from("Z:/no/such/dir/fuseforks")]);
        assert!(tool.spec_for(&ghost).await.is_none());
        // 生きた宣言 → 提示し、フォルダを列挙する。
        let live = ctx_with(vec![docs.dir.clone()]);
        let spec = tool.spec_for(&live).await.expect("提示されること");
        assert!(spec.description.contains(&docs.dir.display().to_string()));
    }

    #[tokio::test]
    async fn outline_lists_heading_tree() {
        let docs = TempDocs::new("outline");
        docs.write("a.md", DOC);
        docs.write("note.txt", "not markdown");
        let ctx = ctx_with(vec![docs.dir.clone()]);
        let out = call(&RagTool, &ctx, serde_json::json!({ "op": "outline" })).await;
        assert!(out.contains("a.md"));
        assert!(out.contains("# 仕様"));
        assert!(out.contains("### 凍結"));
        assert!(!out.contains("note.txt"), "Markdown 以外は載らない");
    }

    #[tokio::test]
    async fn search_attaches_section_path() {
        let docs = TempDocs::new("search");
        docs.write("a.md", DOC);
        let ctx = ctx_with(vec![docs.dir.clone()]);
        let out = call(&RagTool, &ctx, serde_json::json!({ "op": "search", "pattern": "凍結の本文" })).await;
        assert!(out.contains("a.md:9"), "一致行の位置が出ること: {out}");
        assert!(out.contains("節: 仕様 > 契約 > 凍結"), "見出しの経路が付くこと: {out}");
    }

    #[tokio::test]
    async fn read_section_returns_body_with_breadcrumb() {
        let docs = TempDocs::new("read");
        docs.write("a.md", DOC);
        let ctx = ctx_with(vec![docs.dir.clone()]);
        let out = call(
            &RagTool,
            &ctx,
            serde_json::json!({ "op": "read", "path": "a.md", "section": "契約" }),
        )
        .await;
        assert!(out.contains("仕様 > 契約"), "breadcrumb: {out}");
        assert!(out.contains("契約の本文です。"));
        assert!(out.contains("凍結の本文です。"), "子見出しは節に含む");
        assert!(!out.contains("実装の本文です。"), "兄弟の節は含まない");
    }

    #[tokio::test]
    async fn read_ambiguous_section_lists_candidates() {
        let docs = TempDocs::new("ambiguous");
        docs.write("a.md", "# Top\n\n## Configuration\n\nfoo\n\n## Configuration Notes\n\nbar\n");
        let ctx = ctx_with(vec![docs.dir.clone()]);
        let out = call(
            &RagTool,
            &ctx,
            serde_json::json!({ "op": "read", "path": "a.md", "section": "config" }),
        )
        .await;
        assert!(out.contains("曖昧"), "{out}");
        assert!(out.contains("Configuration Notes"));
    }

    #[tokio::test]
    async fn read_refuses_oversized_section_instead_of_clipping() {
        let docs = TempDocs::new("oversize");
        let big_body = "あ".repeat(MAX_OUTPUT_CHARS + 500);
        docs.write("big.md", &format!("# 大節\n\n{big_body}\n"));
        let ctx = ctx_with(vec![docs.dir.clone()]);
        let out = call(
            &RagTool,
            &ctx,
            serde_json::json!({ "op": "read", "path": "big.md", "section": "大節" }),
        )
        .await;
        assert!(out.contains("超えます"), "{out}");
        assert!(!out.contains(&"あ".repeat(100)), "本文を一部でも返さない");
    }

    #[tokio::test]
    async fn outside_declared_roots_is_rejected() {
        let docs = TempDocs::new("outside");
        docs.write("a.md", DOC);
        let other = TempDocs::new("outside_other");
        other.write("secret.md", "# 機密\n");
        let ctx = ctx_with(vec![docs.dir.clone()]);

        // (a) 宣言外ルートの絶対パス。
        let abs = other.dir.join("secret.md");
        let out = call(
            &RagTool,
            &ctx,
            serde_json::json!({ "op": "read", "path": abs.to_string_lossy() }),
        )
        .await;
        assert!(out.contains("宣言された資料フォルダの外"), "{out}");

        // (b) 相対パスの `..` 越え。canonicalize + 前方一致が落とす。
        let out = call(
            &RagTool,
            &ctx,
            serde_json::json!({ "op": "read", "path": "../secret.md" }),
        )
        .await;
        assert!(out.contains("見つかりません") || out.contains("外"), "{out}");
    }

    #[tokio::test]
    async fn invalid_root_is_reported_not_dropped() {
        let docs = TempDocs::new("invalid_note");
        docs.write("a.md", DOC);
        let ghost = PathBuf::from("Z:/no/such/dir/fuseforks");
        let ctx = ctx_with(vec![docs.dir.clone(), ghost]);
        let out = call(&RagTool, &ctx, serde_json::json!({ "op": "outline" })).await;
        assert!(out.contains("a.md"), "生きたルートは動き続ける");
        assert!(out.contains("見つからないため対象外"), "無効の申告が付く: {out}");
    }
}
