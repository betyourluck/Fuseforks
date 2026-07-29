//! 作業フォルダ内のファイルを**書き換える**同梱ツール（`sd`、将来 `yq`）。
//!
//! 契約は `data_contract.yaml` の `write_tools_contract`（Spec 01 で凍結）。
//! 読み取り系（tools/fs.rs）と違い、書き込みは被害クラスを「漏洩」から
//! 「改竄」へ広げるため、より厳しい不変条件を敷く:
//!
//! - **二段階実行**: 既定は preview（diff を返すだけで書かない）。
//!   `apply: true` で書き込み、そのときも適用した diff を必ず返す。
//!   黙って書く経路は存在しない。
//! - **diff 上限は切り詰めでなく拒否**: 12,000 字を超える diff の書き込みは
//!   preview / apply とも実行しない。切り詰めた diff を許すと
//!   「何が変わったかが会話に残る」契約が崩れる。
//! - **1 呼び出し 1 ファイル・新規作成なし・同文不書き込み**。
//! - **検査順序の固定**: 境界 → is_file → (拡張子) → サイズ → バイナリ →
//!   内容の解釈。最初に失敗した段の文言だけを返す。

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::Value;

use crate::compute::spawn_rayon;
use crate::error::CoreResult;
use crate::tool::{AgentTool, ToolContext};
use crate::tools::fs::{
    MAX_FILE_BYTES, MAX_OUTPUT_CHARS, looks_binary, resolve_in_work_dir, work_dir_missing,
};

/// 書き込み対象ファイルを検査順序 1〜5 で開き、UTF-8 文字列として返す。
///
/// 戻りは (絶対パス, 表示用相対パス, 本文)。エラーはモデルへ返す文字列。
/// 順序は write_tools_contract で凍結されている — ここを一本化することで
/// sd / yq のエラーメッセージが同じ状況で同じ文言になる。
fn open_for_edit(work_dir: &Path, user_path: &str) -> Result<(PathBuf, String, String), String> {
    // 1. 境界解決（実在 + 囲い内。canonicalize が新規作成を構造的に封じる）
    let (path, display) = resolve_in_work_dir(work_dir, user_path)?;

    // 2. ファイルであること（ディレクトリ・特殊ファイル拒否）
    if !path.is_file() {
        return Err(format!("`{user_path}` はファイルではありません。"));
    }

    // 4. サイズ上限（3. 拡張子は yq のみ。呼び出し側で先に検査する）
    let size = path.metadata().map(|m| m.len()).unwrap_or(0);
    if size > MAX_FILE_BYTES {
        return Err(format!(
            "`{user_path}` が大きすぎます（上限 {MAX_FILE_BYTES} bytes）。"
        ));
    }

    // 5. バイナリ拒否
    let bytes = std::fs::read(&path).map_err(|e| format!("`{user_path}` を読めません: {e}"))?;
    if looks_binary(&bytes) {
        return Err(format!("`{user_path}` はバイナリファイルのため編集できません。"));
    }

    // 書き戻す前提なので lossy 変換は使えない — 化けたまま書けば改竄になる。
    let text = String::from_utf8(bytes)
        .map_err(|_| format!("`{user_path}` は UTF-8 ではないため編集できません。"))?;

    Ok((path, display, text))
}

/// 変更前後の unified diff を作る。
fn unified_diff(display: &str, before: &str, after: &str) -> String {
    similar::TextDiff::from_lines(before, after)
        .unified_diff()
        .context_radius(3)
        .header(display, display)
        .to_string()
}

/// 変更行数（+ と − の合計）。diff 上限で拒否するときの統計に使う。
fn changed_line_count(before: &str, after: &str) -> usize {
    similar::TextDiff::from_lines(before, after)
        .iter_all_changes()
        .filter(|change| change.tag() != similar::ChangeTag::Equal)
        .count()
}

// ---------------------------------------------------------------------------

/// 正規表現でファイル内を置換するツール（`sd` 相当）。
pub struct SdTool;

#[async_trait]
impl AgentTool for SdTool {
    fn name(&self) -> &str {
        "sd"
    }

    fn description(&self) -> String {
        "作業フォルダ内の 1 ファイルを正規表現で置換する。\
         **既定では書き込まず、適用した場合の差分（diff）だけを返す。**\
         差分を確認してから `apply: true` で実際に書き込むこと。\
         置換文字列では `$1` や `$name` でキャプチャを参照できる。\
         リテラルの `$` は `$$` と書く。対象は 1 回の呼び出しで 1 ファイルだけ。"
            .to_owned()
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "対象ファイルの相対パス"
                },
                "pattern": {
                    "type": "string",
                    "description": "検索する正規表現（Rust regex 構文）"
                },
                "replacement": {
                    "type": "string",
                    "description": "置換文字列。`$1` `$name` でキャプチャ参照、リテラル `$` は `$$`"
                },
                "apply": {
                    "type": "boolean",
                    "description": "true で書き込む。省略時は preview（差分を返すだけで書かない）"
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "大文字小文字を無視するか。既定は false"
                }
            },
            "required": ["path", "pattern", "replacement"],
            "additionalProperties": false
        })
    }

    async fn call(&self, ctx: &ToolContext, args: &Value) -> CoreResult<String> {
        let Some(work_dir) = ctx.work_dir.clone() else {
            return Ok(work_dir_missing());
        };
        let (Some(path), Some(pattern), Some(replacement)) = (
            args.get("path").and_then(Value::as_str),
            args.get("pattern").and_then(Value::as_str),
            args.get("replacement").and_then(Value::as_str),
        ) else {
            return Ok("引数 `path` / `pattern` / `replacement` がすべて必要です。".into());
        };
        let (path, pattern, replacement) =
            (path.to_owned(), pattern.to_owned(), replacement.to_owned());
        let apply = args.get("apply").and_then(Value::as_bool).unwrap_or(false);
        let case_insensitive = args
            .get("case_insensitive")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        spawn_rayon(move || run_sd(&work_dir, &path, &pattern, &replacement, apply, case_insensitive))
            .await
    }
}

/// sd 本体。ブロッキングして良い文脈で呼ぶ。
fn run_sd(
    work_dir: &Path,
    user_path: &str,
    pattern: &str,
    replacement: &str,
    apply: bool,
    case_insensitive: bool,
) -> String {
    let (path, display, text) = match open_for_edit(work_dir, user_path) {
        Ok(opened) => opened,
        Err(message) => return message,
    };

    // 6. 内容の解釈（正規表現コンパイル）。
    // インラインフラグ ((?i) 等) はパターン側が優先される — regex crate の
    // ビルダー既定はパターン内の指定で上書きできる仕様（grep も同挙動）。
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

    let match_count = regex.find_iter(&text).count();
    if match_count == 0 {
        return format!("`{display}` に一致はありません。書き込みは行っていません。");
    }

    let replaced = regex.replace_all(&text, replacement).into_owned();
    if replaced == text {
        return format!(
            "{match_count} 件が一致しましたが、置換後も内容が同一です。変更なし（書き込みは行っていません）。"
        );
    }

    let diff = unified_diff(&display, &text, &replaced);
    if diff.chars().count() > MAX_OUTPUT_CHARS {
        // 契約: 切り詰めた diff は「何が変わったか」を会話に残せないため、
        // 上限を超える書き込みは preview / apply を問わず起こせない。
        return format!(
            "差分が大きすぎるため実行しません（{match_count} 件の置換で {} 行が変わり、\
             diff が上限 {MAX_OUTPUT_CHARS} 字を超えます）。\
             パターンを具体化するか、範囲を分けて置換してください。",
            changed_line_count(&text, &replaced)
        );
    }

    if apply {
        if let Err(err) = std::fs::write(&path, replaced.as_bytes()) {
            return format!("`{display}` へ書き込めませんでした: {err}");
        }
        format!("適用済み: {match_count} 件を置換しました。\n{diff}")
    } else {
        format!(
            "preview（未適用）: {match_count} 件を置換します。\
             この内容で良ければ `apply: true` で書き込んでください。\n{diff}"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AgentId;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "concordia-edit-{tag}-{}",
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

        fn read(&self, rel: &str) -> String {
            std::fs::read_to_string(self.0.join(rel)).unwrap()
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

    async fn call_sd(dir: &TempDir, args: serde_json::Value) -> String {
        SdTool.call(&ctx_with(Some(&dir.0)), &args).await.unwrap()
    }

    #[tokio::test]
    async fn sd_without_a_work_dir_explains_how_to_enable_it() {
        let reply = SdTool
            .call(
                &ctx_with(None),
                &serde_json::json!({ "path": "a.txt", "pattern": "x", "replacement": "y" }),
            )
            .await
            .unwrap();
        assert!(reply.contains("作業フォルダ"), "{reply}");
    }

    #[tokio::test]
    async fn sd_preview_returns_a_diff_without_writing() {
        let dir = TempDir::new("preview");
        dir.write("a.txt", "old value\nkeep\n");

        let reply = call_sd(
            &dir,
            serde_json::json!({ "path": "a.txt", "pattern": "old", "replacement": "new" }),
        )
        .await;

        assert!(reply.contains("preview"), "{reply}");
        assert!(reply.contains("-old value"), "{reply}");
        assert!(reply.contains("+new value"), "{reply}");
        assert_eq!(dir.read("a.txt"), "old value\nkeep\n", "書き込まれないこと");
    }

    #[tokio::test]
    async fn sd_apply_writes_and_the_returned_diff_matches() {
        let dir = TempDir::new("apply");
        dir.write("a.txt", "old value\n");

        let reply = call_sd(
            &dir,
            serde_json::json!({
                "path": "a.txt", "pattern": "old", "replacement": "new", "apply": true
            }),
        )
        .await;

        assert!(reply.contains("適用済み"), "{reply}");
        assert!(reply.contains("+new value"), "{reply}");
        assert_eq!(dir.read("a.txt"), "new value\n", "実際に書き込まれること");
    }

    #[tokio::test]
    async fn sd_supports_capture_references_and_dollar_dollar_escape() {
        let dir = TempDir::new("capture");
        dir.write("a.txt", "price: 100\n");

        call_sd(
            &dir,
            serde_json::json!({
                "path": "a.txt",
                "pattern": r"price: (\d+)",
                "replacement": "cost: $$$1",
                "apply": true
            }),
        )
        .await;

        assert_eq!(dir.read("a.txt"), "cost: $100\n", "$$ はリテラル $、$1 は参照");
    }

    #[tokio::test]
    async fn sd_rejects_paths_that_escape_the_work_dir() {
        let parent = TempDir::new("escape");
        parent.write("secret.txt", "機密\n");
        let inner = parent.0.join("inner");
        std::fs::create_dir_all(&inner).unwrap();

        let reply = SdTool
            .call(
                &ctx_with(Some(&inner)),
                &serde_json::json!({
                    "path": "../secret.txt", "pattern": "機密", "replacement": "x", "apply": true
                }),
            )
            .await
            .unwrap();

        assert!(!reply.contains("機密\n"), "内容を漏らさないこと: {reply}");
        assert_eq!(
            std::fs::read_to_string(parent.0.join("secret.txt")).unwrap(),
            "機密\n",
            "囲いの外は書き換えないこと"
        );
    }

    #[tokio::test]
    async fn sd_rejects_directories() {
        let dir = TempDir::new("dir");
        dir.write("sub/a.txt", "x\n");

        let reply = call_sd(
            &dir,
            serde_json::json!({ "path": "sub", "pattern": "x", "replacement": "y" }),
        )
        .await;
        assert!(reply.contains("ファイルではありません"), "{reply}");
    }

    #[tokio::test]
    async fn sd_reports_invalid_regex_as_a_readable_message() {
        let dir = TempDir::new("badre");
        dir.write("a.txt", "x\n");

        let reply = call_sd(
            &dir,
            serde_json::json!({ "path": "a.txt", "pattern": "([", "replacement": "y" }),
        )
        .await;
        assert!(reply.contains("正規表現"), "{reply}");
    }

    #[tokio::test]
    async fn sd_zero_matches_does_not_write() {
        let dir = TempDir::new("nomatch");
        dir.write("a.txt", "abc\n");

        let reply = call_sd(
            &dir,
            serde_json::json!({ "path": "a.txt", "pattern": "zzz", "replacement": "y", "apply": true }),
        )
        .await;

        assert!(reply.contains("一致はありません"), "{reply}");
        assert_eq!(dir.read("a.txt"), "abc\n");
    }

    #[tokio::test]
    async fn sd_identical_replacement_is_reported_as_no_change_and_not_written() {
        let dir = TempDir::new("samecontent");
        dir.write("a.txt", "same\n");

        let reply = call_sd(
            &dir,
            serde_json::json!({ "path": "a.txt", "pattern": "same", "replacement": "same", "apply": true }),
        )
        .await;

        assert!(reply.contains("変更なし"), "{reply}");
    }

    #[tokio::test]
    async fn sd_refuses_oversized_diffs_with_statistics_even_in_preview() {
        let dir = TempDir::new("hugediff");
        let body: String = (0..2000).map(|i| format!("needle line {i}\n")).collect();
        dir.write("big.txt", &body);

        for apply in [false, true] {
            let reply = call_sd(
                &dir,
                serde_json::json!({
                    "path": "big.txt", "pattern": "needle", "replacement": "replaced", "apply": apply
                }),
            )
            .await;

            assert!(reply.contains("大きすぎる"), "{reply}");
            assert!(reply.contains("2000 件"), "統計を返すこと: {reply}");
            assert!(!reply.contains("+replaced"), "切り詰めた diff も返さないこと");
        }
        assert_eq!(dir.read("big.txt"), body, "preview / apply とも書かないこと");
    }

    #[tokio::test]
    async fn sd_inline_flags_take_precedence_over_the_argument() {
        let dir = TempDir::new("inline");
        dir.write("a.txt", "Needle\n");

        // 引数で大文字小文字無視を指定しても、パターン内の (?-i) が勝つ。
        let reply = call_sd(
            &dir,
            serde_json::json!({
                "path": "a.txt", "pattern": "(?-i)needle", "replacement": "x",
                "case_insensitive": true
            }),
        )
        .await;
        assert!(reply.contains("一致はありません"), "{reply}");
    }

    #[tokio::test]
    async fn sd_rejects_non_utf8_files() {
        let dir = TempDir::new("nonutf8");
        // NUL を含まない不正 UTF-8（バイナリ判定をすり抜ける並び）。
        std::fs::write(dir.0.join("bad.txt"), [0xFF, 0xFE, b'a']).unwrap();

        let reply = call_sd(
            &dir,
            serde_json::json!({ "path": "bad.txt", "pattern": "a", "replacement": "b" }),
        )
        .await;
        assert!(reply.contains("UTF-8"), "{reply}");
    }
}
