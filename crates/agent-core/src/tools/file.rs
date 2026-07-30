//! 作業フォルダ内のファイル・フォルダを操作する同梱ツール（`file`）。
//!
//! 契約は `data_contract.yaml` の `file_tool_contract`（Spec 09 で凍結）。
//!
//! # なぜ 1 ツール + op 列挙なのか
//!
//! 操作ごとにツールを並べると、毎ターン運ぶスキーマの固定費が操作数ぶん増え、
//! モデルの選択肢も散る。`yq` が確立した「1 ツール + 閉じた op 列挙」を踏襲する。
//!
//! # 書き換え系（tools/edit.rs）との役割分担
//!
//! - `sd` / `yq` = **既存ファイルの部分編集**。二段階実行（preview → apply）で
//!   「何が変わったか」を会話に残す
//! - `file` = **存在そのものの操作**（作る・動かす・複製する・消す）と全文の読み書き
//!
//! 起点は実機の空転（2026-07-31）: 新規作成の手段が無いまま `sd` で作成を
//! 試み続けた。**禁止ではなく、その行動を選ぶ理由を消す**のが処方で、
//! 能力（本ツール）と案内（`sd` / `yq` の不在文言）の両方が要る。
//!
//! # 縮退の方向を安全側へ固定する
//!
//! - 削除は**ごみ箱へ移すだけ**。完全削除の経路は無く、ごみ箱が使えない環境では
//!   失敗として返す（黙って完全削除へフォールバックしない）
//! - 上書きは `overwrite: true` の明示が要る（既定は拒否して案内を返す）

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::Value;

use crate::compute::spawn_rayon;
use crate::error::CoreResult;
use crate::tool::{AgentTool, ToolContext};
use crate::tools::fs::{
    MAX_FILE_BYTES, MAX_OUTPUT_CHARS, looks_binary, resolve_creatable, resolve_in_work_dir,
    work_dir_missing,
};

/// ファイル・フォルダを操作するツール。
pub struct FileTool;

#[async_trait]
impl AgentTool for FileTool {
    fn name(&self) -> &str {
        "file"
    }

    fn description(&self) -> String {
        "作業フォルダ内のファイル・フォルダを操作する。\
         **新しいファイルを作れる唯一のツール**（翻訳・要約・生成物の書き出しはこれを使う）。\
         op で操作を選ぶ: read（全文を読む）/ write（新規作成・全文置換）/ \
         mkdir（フォルダ作成）/ move（移動・改名）/ copy（複製）/ remove（ごみ箱へ移す）。\
         既にあるファイルの**一部だけ**を直すなら sd / yq のほうが安く確実。\
         削除はごみ箱へ移すだけで、完全には消さない。"
            .to_owned()
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "op": {
                    "type": "string",
                    "enum": ["read", "write", "mkdir", "move", "copy", "remove"],
                    "description": "操作。read=読む / write=書く / mkdir=フォルダ作成 / move=移動・改名 / copy=複製 / remove=ごみ箱へ"
                },
                "path": {
                    "type": "string",
                    "description": "対象の相対パス（作業フォルダ起点）"
                },
                "content": {
                    "type": "string",
                    "description": "write で書き込む内容（全文）"
                },
                "to": {
                    "type": "string",
                    "description": "move / copy の宛先の相対パス"
                },
                "overwrite": {
                    "type": "boolean",
                    "description": "既存を上書きするか。既定 false（既存があれば拒否する）"
                }
            },
            "required": ["op", "path"],
            "additionalProperties": false
        })
    }

    async fn call(&self, ctx: &ToolContext, args: &Value) -> CoreResult<String> {
        let Some(work_dir) = ctx.work_dir.clone() else {
            return Ok(work_dir_missing());
        };
        let (Some(op), Some(path)) = (
            args.get("op").and_then(Value::as_str),
            args.get("path").and_then(Value::as_str),
        ) else {
            return Ok("引数 `op` と `path` が必要です。".into());
        };
        let (op, path) = (op.to_owned(), path.to_owned());
        let content = args
            .get("content")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let to = args.get("to").and_then(Value::as_str).map(str::to_owned);
        let overwrite = args
            .get("overwrite")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        // ファイル I/O は Tokio ワーカーを塞がない側へ逃がす（既存ツールと同じ規律）。
        spawn_rayon(move || run_file(&work_dir, &op, &path, content.as_deref(), to.as_deref(), overwrite))
            .await
    }
}

/// `file` 本体。ブロッキングして良い文脈で呼ぶ。
fn run_file(
    work_dir: &Path,
    op: &str,
    user_path: &str,
    content: Option<&str>,
    to: Option<&str>,
    overwrite: bool,
) -> String {
    match op {
        "read" => run_read(work_dir, user_path),
        "write" => match content {
            Some(content) => run_write(work_dir, user_path, content, overwrite),
            None => "write には `content`（書き込む全文）が必要です。".to_owned(),
        },
        "mkdir" => run_mkdir(work_dir, user_path),
        "move" | "copy" => match to {
            Some(to) => run_transfer(work_dir, user_path, to, overwrite, op == "move"),
            None => format!("{op} には `to`（宛先の相対パス）が必要です。"),
        },
        "remove" => run_remove(work_dir, user_path),
        other => format!(
            "`{other}` は使えない操作です。op は read / write / mkdir / move / copy / remove のいずれかです。"
        ),
    }
}

/// 全文を読む。上限とバイナリ拒否は読み取り系と同じ規律。
fn run_read(work_dir: &Path, user_path: &str) -> String {
    let (path, display) = match resolve_in_work_dir(work_dir, user_path) {
        Ok(resolved) => resolved,
        Err(message) => return message,
    };
    if !path.is_file() {
        return format!("`{display}` はファイルではありません。");
    }
    if path.metadata().map(|m| m.len()).unwrap_or(0) > MAX_FILE_BYTES {
        return format!("`{display}` が大きすぎます（上限 {MAX_FILE_BYTES} bytes）。grep で必要な箇所だけを探してください。");
    }

    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) => return format!("`{display}` を読めません: {err}"),
    };
    if looks_binary(&bytes) {
        return format!("`{display}` はバイナリファイルのため読めません。");
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return format!("`{display}` は UTF-8 ではないため読めません。");
    };

    // 出力は必ず有界。打ち切りは黙って行わず、落とした量を書く
    // （silent truncation は「全部読んだ」と誤読される）。
    if text.chars().count() > MAX_OUTPUT_CHARS {
        let head: String = text.chars().take(MAX_OUTPUT_CHARS).collect();
        let dropped = text.chars().count() - MAX_OUTPUT_CHARS;
        return format!(
            "`{display}`（先頭 {MAX_OUTPUT_CHARS} 字。残り {dropped} 字は省略しました）\n{head}"
        );
    }
    format!("`{display}`\n{text}")
}

/// 全文を書く。**既存は `overwrite: true` が無い限り拒否**する。
fn run_write(work_dir: &Path, user_path: &str, content: &str, overwrite: bool) -> String {
    let (path, display) = match resolve_creatable(work_dir, user_path) {
        Ok(resolved) => resolved,
        Err(message) => return message,
    };

    if path.is_dir() {
        return format!("`{display}` はフォルダです。ファイルとして書き込めません。");
    }
    let existed = path.is_file();
    if existed && !overwrite {
        return format!(
            "`{display}` は既にあります。書き込んでいません。\
             一部だけ直すなら sd / yq を、全文を置き換えるなら overwrite: true を付けて再実行してください。"
        );
    }
    if content.len() as u64 > MAX_FILE_BYTES {
        return format!("内容が大きすぎます（上限 {MAX_FILE_BYTES} bytes）。分割して書いてください。");
    }

    // 親フォルダは自動で作る。深い階層への 1 回の書き込みで mkdir を
    // 強制すると、モデルが 2 呼び出しに割るだけで得るものが無い。
    if let Some(parent) = path.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        return format!("`{display}` の親フォルダを作れません: {err}");
    }
    if let Err(err) = std::fs::write(&path, content) {
        return format!("`{display}` を書けません: {err}");
    }

    let chars = content.chars().count();
    if existed {
        format!("`{display}` を上書きしました（{chars} 字）。")
    } else {
        format!("`{display}` を作成しました（{chars} 字）。")
    }
}

/// フォルダを作る（中間も含めて）。
fn run_mkdir(work_dir: &Path, user_path: &str) -> String {
    let (path, display) = match resolve_creatable(work_dir, user_path) {
        Ok(resolved) => resolved,
        Err(message) => return message,
    };
    if path.is_dir() {
        return format!("`{display}` は既にあります（作成は不要です）。");
    }
    if path.is_file() {
        return format!("`{display}` は同名のファイルが既にあります。");
    }
    match std::fs::create_dir_all(&path) {
        Ok(()) => format!("`{display}` を作成しました。"),
        Err(err) => format!("`{display}` を作れません: {err}"),
    }
}

/// 移動（改名）と複製。**両端とも境界内であることを検査する。**
fn run_transfer(
    work_dir: &Path,
    user_path: &str,
    to: &str,
    overwrite: bool,
    is_move: bool,
) -> String {
    let verb = if is_move { "移動" } else { "複製" };

    // 移動元は実在必須。宛先は実在しなくてよい（片端だけの検査は囲いの穴になる）。
    let (source, source_display) = match resolve_in_work_dir(work_dir, user_path) {
        Ok(resolved) => resolved,
        Err(message) => return message,
    };
    let (dest, dest_display) = match resolve_creatable(work_dir, to) {
        Ok(resolved) => resolved,
        Err(message) => return message,
    };

    if source == dest {
        return format!("移動元と宛先が同じです（`{source_display}`）。何もしていません。");
    }
    if dest.exists() && !overwrite {
        return format!(
            "`{dest_display}` は既にあります。{verb}していません。\
             置き換えるなら overwrite: true を付けて再実行してください。"
        );
    }
    if !is_move && source.is_dir() {
        return format!(
            "`{source_display}` はフォルダです。フォルダの複製には対応していません\
             （move なら フォルダのまま移動できます）。"
        );
    }
    if let Some(parent) = dest.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        return format!("`{dest_display}` の親フォルダを作れません: {err}");
    }
    // 上書きの宛先は先に退ける。rename はプラットフォームで挙動が割れ
    // （Windows は既存宛先で失敗しうる）、copy は黙って潰す。
    if dest.exists()
        && overwrite
        && let Err(err) = remove_existing(&dest)
    {
        return format!("`{dest_display}` を置き換えられません: {err}");
    }

    let result = if is_move {
        std::fs::rename(&source, &dest).map(|()| ())
    } else {
        std::fs::copy(&source, &dest).map(|_| ())
    };
    match result {
        Ok(()) => format!("`{source_display}` を `{dest_display}` へ{verb}しました。"),
        Err(err) => format!("`{source_display}` を{verb}できません: {err}"),
    }
}

/// 上書き時に宛先を退ける。ここは利用者の明示（`overwrite: true`）があるので
/// ごみ箱を経由しない — 経由すると「置き換え」がごみ箱を汚し続ける。
fn remove_existing(path: &Path) -> std::io::Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// **ごみ箱へ移す。** 完全削除の経路は無い。
fn run_remove(work_dir: &Path, user_path: &str) -> String {
    let (path, display) = match resolve_in_work_dir(work_dir, user_path) {
        Ok(resolved) => resolved,
        Err(message) => return message,
    };

    // 作業フォルダそのものを消させない。囲いの中で最も壊れるのがここ。
    if work_dir.canonicalize().map(|root| root == path).unwrap_or(false) {
        return "作業フォルダそのものは削除できません。".to_owned();
    }

    match trash::delete(&path) {
        Ok(()) => format!("`{display}` をごみ箱へ移しました（完全には削除していません）。"),
        // ごみ箱が使えない環境（一部の Linux 構成・ネットワークドライブ）。
        // 黙って完全削除へ倒さない — 取り消せない操作へ勝手に格上げしない。
        Err(err) => format!(
            "`{display}` をごみ箱へ移せません: {err}\n\
             この環境ではごみ箱が使えないようです。完全削除は行わないので、利用者に削除を頼んでください。"
        ),
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
                "concordia-file-{tag}-{}",
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

        fn exists(&self, rel: &str) -> bool {
            self.0.join(rel).exists()
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

    async fn call(dir: &TempDir, args: Value) -> String {
        FileTool.call(&ctx_with(Some(&dir.0)), &args).await.unwrap()
    }

    #[tokio::test]
    async fn without_a_work_dir_it_explains_how_to_enable_it() {
        let reply = FileTool
            .call(&ctx_with(None), &serde_json::json!({ "op": "read", "path": "a.txt" }))
            .await
            .unwrap();
        assert!(reply.contains("作業フォルダ"), "有効化の道筋を案内すること: {reply}");
    }

    #[tokio::test]
    async fn write_creates_a_new_file_including_parent_folders() {
        let dir = TempDir::new("write-new");
        let reply = call(
            &dir,
            serde_json::json!({ "op": "write", "path": "docs/en/README.md", "content": "# Title\n" }),
        )
        .await;

        assert!(reply.contains("作成しました"), "{reply}");
        assert_eq!(dir.read("docs/en/README.md"), "# Title\n");
    }

    #[tokio::test]
    async fn write_refuses_an_existing_file_and_points_at_the_alternatives() {
        let dir = TempDir::new("write-existing");
        dir.write("a.txt", "元の内容");

        let reply = call(
            &dir,
            serde_json::json!({ "op": "write", "path": "a.txt", "content": "新しい内容" }),
        )
        .await;

        assert!(reply.contains("既にあります"), "{reply}");
        // 次の一手を必ず示す（拒否だけだと同じ呼び出しを繰り返す）。
        assert!(reply.contains("sd") && reply.contains("overwrite"), "{reply}");
        assert_eq!(dir.read("a.txt"), "元の内容", "拒否経路では書かない");
    }

    #[tokio::test]
    async fn write_with_overwrite_replaces_the_file() {
        let dir = TempDir::new("write-overwrite");
        dir.write("a.txt", "元の内容");

        let reply = call(
            &dir,
            serde_json::json!({ "op": "write", "path": "a.txt", "content": "新しい内容", "overwrite": true }),
        )
        .await;

        assert!(reply.contains("上書きしました"), "{reply}");
        assert_eq!(dir.read("a.txt"), "新しい内容");
    }

    #[tokio::test]
    async fn read_returns_the_whole_text() {
        let dir = TempDir::new("read");
        dir.write("a.txt", "本文\n2 行目\n");

        let reply = call(&dir, serde_json::json!({ "op": "read", "path": "a.txt" })).await;
        assert!(reply.contains("本文") && reply.contains("2 行目"), "{reply}");
    }

    #[tokio::test]
    async fn read_announces_the_cut_instead_of_truncating_silently() {
        let dir = TempDir::new("read-long");
        dir.write("big.txt", &"あ".repeat(MAX_OUTPUT_CHARS + 500));

        let reply = call(&dir, serde_json::json!({ "op": "read", "path": "big.txt" })).await;
        assert!(reply.contains("省略しました"), "落とした量を明示すること: 先頭 80 字 = {}", &reply[..80.min(reply.len())]);
    }

    #[tokio::test]
    async fn mkdir_creates_intermediate_folders() {
        let dir = TempDir::new("mkdir");
        let reply = call(&dir, serde_json::json!({ "op": "mkdir", "path": "a/b/c" })).await;

        assert!(reply.contains("作成しました"), "{reply}");
        assert!(dir.0.join("a/b/c").is_dir());
    }

    #[tokio::test]
    async fn move_relocates_and_copy_duplicates() {
        let dir = TempDir::new("transfer");
        dir.write("src/a.txt", "中身");

        let moved = call(
            &dir,
            serde_json::json!({ "op": "move", "path": "src/a.txt", "to": "dst/b.txt" }),
        )
        .await;
        assert!(moved.contains("移動しました"), "{moved}");
        assert!(!dir.exists("src/a.txt"), "move は元を残さない");
        assert_eq!(dir.read("dst/b.txt"), "中身");

        let copied = call(
            &dir,
            serde_json::json!({ "op": "copy", "path": "dst/b.txt", "to": "dst/c.txt" }),
        )
        .await;
        assert!(copied.contains("複製しました"), "{copied}");
        assert_eq!(dir.read("dst/b.txt"), "中身", "copy は元を残す");
        assert_eq!(dir.read("dst/c.txt"), "中身");
    }

    #[tokio::test]
    async fn transfer_refuses_an_existing_destination_without_overwrite() {
        let dir = TempDir::new("transfer-existing");
        dir.write("a.txt", "A");
        dir.write("b.txt", "B");

        let reply = call(
            &dir,
            serde_json::json!({ "op": "move", "path": "a.txt", "to": "b.txt" }),
        )
        .await;

        assert!(reply.contains("既にあります"), "{reply}");
        assert_eq!(dir.read("a.txt"), "A", "拒否経路では動かさない");
        assert_eq!(dir.read("b.txt"), "B");

        let forced = call(
            &dir,
            serde_json::json!({ "op": "move", "path": "a.txt", "to": "b.txt", "overwrite": true }),
        )
        .await;
        assert!(forced.contains("移動しました"), "{forced}");
        assert_eq!(dir.read("b.txt"), "A");
    }

    #[tokio::test]
    async fn remove_moves_to_the_trash_and_says_so() {
        let dir = TempDir::new("remove");
        dir.write("a.txt", "消す対象");

        let reply = call(&dir, serde_json::json!({ "op": "remove", "path": "a.txt" })).await;

        if reply.contains("ごみ箱へ移せません") {
            // ごみ箱が無い環境。**完全削除へ倒していない**ことがここの契約。
            assert!(dir.exists("a.txt"), "失敗時にファイルを消してしまわないこと");
            assert!(reply.contains("完全削除は行わない"), "{reply}");
            return;
        }
        assert!(reply.contains("ごみ箱"), "{reply}");
        assert!(!dir.exists("a.txt"), "場所からは消えること");
    }

    #[tokio::test]
    async fn the_work_dir_itself_cannot_be_removed() {
        let dir = TempDir::new("remove-root");
        let reply = call(&dir, serde_json::json!({ "op": "remove", "path": "." })).await;

        assert!(reply.contains("作業フォルダそのもの"), "{reply}");
        assert!(dir.0.is_dir());
    }

    #[tokio::test]
    async fn every_op_refuses_paths_that_escape_the_work_dir() {
        let dir = TempDir::new("escape");
        dir.write("src/a.txt", "中身");

        for args in [
            serde_json::json!({ "op": "write", "path": "../escape.txt", "content": "x" }),
            serde_json::json!({ "op": "write", "path": "src/../../escape.txt", "content": "x" }),
            serde_json::json!({ "op": "mkdir", "path": "../escape" }),
            serde_json::json!({ "op": "move", "path": "src/a.txt", "to": "../escape.txt" }),
            serde_json::json!({ "op": "copy", "path": "src/a.txt", "to": "../escape.txt" }),
            serde_json::json!({ "op": "remove", "path": "../" }),
        ] {
            let reply = call(&dir, args.clone()).await;
            assert!(
                reply.contains("作業フォルダの外") || reply.contains("見つかりません"),
                "{args} が拒否されること: {reply}"
            );
        }
        assert_eq!(dir.read("src/a.txt"), "中身", "どの経路でも元は無傷");
    }

    #[tokio::test]
    async fn an_unknown_op_lists_the_available_ones() {
        let dir = TempDir::new("unknown-op");
        let reply = call(&dir, serde_json::json!({ "op": "append", "path": "a.txt" })).await;
        assert!(reply.contains("read") && reply.contains("remove"), "{reply}");
    }

    #[tokio::test]
    async fn binary_files_are_refused_on_read() {
        let dir = TempDir::new("binary");
        std::fs::write(dir.0.join("bin.dat"), [0u8, 1, 2, 0, 3]).unwrap();

        let reply = call(&dir, serde_json::json!({ "op": "read", "path": "bin.dat" })).await;
        assert!(reply.contains("バイナリ"), "{reply}");
    }
}
