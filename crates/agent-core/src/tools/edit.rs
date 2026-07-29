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
/// `allowed_exts` は検査順序 3（yq の拡張子制限）。`None` なら検査しない（sd）。
fn open_for_edit(
    work_dir: &Path,
    user_path: &str,
    allowed_exts: Option<&[&str]>,
) -> Result<(PathBuf, String, String), String> {
    // 1. 境界解決（実在 + 囲い内。canonicalize が新規作成を構造的に封じる）
    let (path, display) = resolve_in_work_dir(work_dir, user_path)?;

    // 2. ファイルであること（ディレクトリ・特殊ファイル拒否）
    if !path.is_file() {
        return Err(format!("`{user_path}` はファイルではありません。"));
    }

    // 3. 拡張子（yq のみ。推測でパースしない）
    if let Some(allowed) = allowed_exts {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if !allowed.contains(&ext.as_str()) {
            return Err(format!(
                "`{user_path}` は対応していない形式です（対応: {}）。",
                allowed.join(" / ")
            ));
        }
    }

    // 4. サイズ上限
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
    let (path, display, text) = match open_for_edit(work_dir, user_path, None) {
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

// ---------------------------------------------------------------------------
// yq — 構造を保った設定編集
// ---------------------------------------------------------------------------

/// `a.b[0].c` 形式のパスの 1 区切り。
#[derive(Debug, Clone, PartialEq)]
enum PathSeg {
    /// マップのキー。
    Key(String),
    /// 配列のインデックス。
    Index(usize),
}

/// パスの表示（エラーメッセージ用）。
fn seg_display(segs: &[PathSeg]) -> String {
    let mut out = String::new();
    for seg in segs {
        match seg {
            PathSeg::Key(key) => {
                if !out.is_empty() {
                    out.push('.');
                }
                out.push_str(key);
            }
            PathSeg::Index(index) => out.push_str(&format!("[{index}]")),
        }
    }
    out
}

/// `a.b[0].c` を区切り列へ解釈する。
///
/// v1 の制限（write_tools_contract）: キーは `[A-Za-z0-9_-]+`、
/// インデックスは `[数字]` のみ。クォートされたキー（`a."b.c"` 等）は
/// 非対応として読める文言で拒否する。
fn parse_key_path(key: &str) -> Result<Vec<PathSeg>, String> {
    fn err(key: &str) -> String {
        format!(
            "`{key}` はパスとして解釈できません。対応する形式は `a.b[0].c`\
             （キーは英数字・`-`・`_`、インデックスは数字）だけです。\
             クォートされたキーや記号を含むキーには対応していません。"
        )
    }

    if key.is_empty() {
        return Err(err(key));
    }

    let mut segs = Vec::new();
    let mut chars = key.chars().peekable();

    loop {
        // キー名（先頭が `[` なら省略可 — ルートが配列のケース）。
        let mut name = String::new();
        while let Some(&c) = chars.peek() {
            if c == '.' || c == '[' {
                break;
            }
            if !(c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                return Err(err(key));
            }
            name.push(c);
            chars.next();
        }
        if !name.is_empty() {
            segs.push(PathSeg::Key(name));
        } else if !matches!(chars.peek(), Some('[')) {
            return Err(err(key)); // 空セグメント（`a..b` や末尾ドット）
        }

        // 続くインデックス列 `[0][1]...`
        while matches!(chars.peek(), Some('[')) {
            chars.next();
            let mut digits = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_ascii_digit() {
                    digits.push(c);
                    chars.next();
                } else {
                    break;
                }
            }
            if digits.is_empty() || chars.next() != Some(']') {
                return Err(err(key));
            }
            let index: usize = digits.parse().map_err(|_| err(key))?;
            segs.push(PathSeg::Index(index));
        }

        match chars.next() {
            None => break,
            Some('.') => continue,
            Some(_) => return Err(err(key)),
        }
    }

    if segs.is_empty() {
        return Err(err(key));
    }
    Ok(segs)
}

/// yq の操作種別。
#[derive(Debug, Clone, Copy, PartialEq)]
enum YqOp {
    Get,
    Set,
    Remove,
}

/// TOML / YAML / JSON の値だけを取得・設定・削除するツール（`yq` 相当）。
pub struct YqTool;

/// yq が受け付ける拡張子（検査順序 3）。
const YQ_EXTENSIONS: [&str; 4] = ["toml", "yaml", "yml", "json"];

#[async_trait]
impl AgentTool for YqTool {
    fn name(&self) -> &str {
        "yq"
    }

    fn description(&self) -> String {
        "TOML / YAML / JSON ファイルの特定の値だけを取得（get）・設定（set）・\
         削除（remove）する。コメント・キー順・フォーマットは保持されるので、\
         **設定ファイルの値を変えるときはファイル全体を書き直さずこれを使うこと**。\
         `key` は `a.b[0].c` 形式のパスのみ（yq のクエリ式は使えない）。\
         set / remove は既定では書き込まず差分（diff）だけを返す。\
         確認してから `apply: true` で書き込むこと。\
         set できるのはスカラー値（文字列・数値・真偽・null）だけ。"
            .to_owned()
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "対象ファイルの相対パス（.toml / .yaml / .yml / .json）"
                },
                "op": {
                    "type": "string",
                    "enum": ["get", "set", "remove"],
                    "description": "操作。get = 値の取得、set = 値の設定、remove = キーの削除"
                },
                "key": {
                    "type": "string",
                    "description": "対象のパス。`a.b[0].c` 形式（キーは英数字・`-`・`_` のみ）"
                },
                "value": {
                    "type": "string",
                    "description": "set する値を JSON リテラルで（例: `\"text\"` / `42` / `true` / `null`）。set 時のみ"
                },
                "apply": {
                    "type": "boolean",
                    "description": "true で書き込む。省略時は preview（差分を返すだけで書かない）。set / remove のみ"
                }
            },
            "required": ["path", "op", "key"],
            "additionalProperties": false
        })
    }

    async fn call(&self, ctx: &ToolContext, args: &Value) -> CoreResult<String> {
        let Some(work_dir) = ctx.work_dir.clone() else {
            return Ok(work_dir_missing());
        };
        let (Some(path), Some(op), Some(key)) = (
            args.get("path").and_then(Value::as_str),
            args.get("op").and_then(Value::as_str),
            args.get("key").and_then(Value::as_str),
        ) else {
            return Ok("引数 `path` / `op` / `key` がすべて必要です。".into());
        };
        let op = match op {
            "get" => YqOp::Get,
            "set" => YqOp::Set,
            "remove" => YqOp::Remove,
            other => {
                return Ok(format!(
                    "`{other}` という操作はありません。`get` / `set` / `remove` から選んでください。"
                ));
            }
        };
        let (path, key) = (path.to_owned(), key.to_owned());
        let value = args.get("value").and_then(Value::as_str).map(str::to_owned);
        let apply = args.get("apply").and_then(Value::as_bool).unwrap_or(false);

        spawn_rayon(move || run_yq(&work_dir, &path, op, &key, value.as_deref(), apply)).await
    }
}

/// yq 本体。ブロッキングして良い文脈で呼ぶ。
fn run_yq(
    work_dir: &Path,
    user_path: &str,
    op: YqOp,
    key: &str,
    value: Option<&str>,
    apply: bool,
) -> String {
    let (path, display, text) = match open_for_edit(work_dir, user_path, Some(&YQ_EXTENSIONS)) {
        Ok(opened) => opened,
        Err(message) => return message,
    };

    let segs = match parse_key_path(key) {
        Ok(segs) => segs,
        Err(message) => return message,
    };

    // set の value は JSON リテラルとして解釈し、スカラーだけを受ける（対称規則）。
    let new_value = if op == YqOp::Set {
        let Some(raw) = value else {
            return "`set` には `value` が必要です（JSON リテラルで指定）。".into();
        };
        match serde_json::from_str::<Value>(raw) {
            Ok(Value::Array(_) | Value::Object(_)) => {
                return "配列・オブジェクトは set できません（スカラー値のみ）。\
                        構造の変更はファイルを直接編集してください。"
                    .into();
            }
            Ok(scalar) => Some(scalar),
            Err(_) => {
                return format!(
                    "`{raw}` は JSON リテラルとして解釈できません。\
                     文字列は `\"引用符\"` で囲んでください（例: \"text\" / 42 / true / null）。"
                );
            }
        }
    } else {
        None
    };

    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    // 6. 内容の解釈と操作（形式別バックエンド）。
    // 戻り値: get は Ok(表示文字列)、set / remove は Ok(編集後の全文)。
    let outcome = match ext.as_str() {
        "json" => yq_json(&text, op, &segs, new_value.as_ref()),
        "toml" => yq_toml(&text, op, &segs, new_value.as_ref()),
        "yaml" | "yml" => yq_yaml(&text, op, &segs, new_value.as_ref()),
        _ => unreachable!("拡張子は open_for_edit で検査済み"),
    };

    let after = match outcome {
        Ok(YqOutcome::Value(shown)) => {
            return format!("{display} の {} = {shown}", seg_display(&segs));
        }
        Ok(YqOutcome::NoChange) => {
            return format!(
                "{} は既にその値です。変更なし（書き込みは行っていません）。",
                seg_display(&segs)
            );
        }
        Ok(YqOutcome::Edited(after)) => after,
        Err(message) => return message,
    };

    let diff = unified_diff(&display, &text, &after);
    if diff.chars().count() > MAX_OUTPUT_CHARS {
        return format!(
            "差分が大きすぎるため実行しません（{} 行が変わり、diff が上限 \
             {MAX_OUTPUT_CHARS} 字を超えます）。対象を絞ってください。",
            changed_line_count(&text, &after)
        );
    }

    if apply {
        if let Err(err) = std::fs::write(&path, after.as_bytes()) {
            return format!("`{display}` へ書き込めませんでした: {err}");
        }
        format!("適用済み: {} を更新しました。\n{diff}", seg_display(&segs))
    } else {
        format!(
            "preview（未適用）: この内容で良ければ `apply: true` で書き込んでください。\n{diff}"
        )
    }
}

/// 形式別バックエンドの結果。
enum YqOutcome {
    /// get の結果（表示用文字列）。
    Value(String),
    /// set したが既に同じ値（書かない）。
    NoChange,
    /// 編集後の全文。
    Edited(String),
}

/// JSON バックエンド。キー順は保持（serde_json の preserve_order feature）、
/// 整形（インデント・改行）は正規化される — 契約どおりであり、
/// diff が対象行以外へ及ぶことは許容されている。
fn yq_json(
    text: &str,
    op: YqOp,
    segs: &[PathSeg],
    new_value: Option<&Value>,
) -> Result<YqOutcome, String> {
    let mut root: Value = serde_json::from_str(text)
        .map_err(|err| format!("JSON として解釈できません: {err}"))?;

    match op {
        YqOp::Get => {
            let target = json_navigate(&root, segs)
                .ok_or_else(|| format!("`{}` は存在しません。", seg_display(segs)))?;
            Ok(YqOutcome::Value(target.to_string()))
        }
        YqOp::Set => {
            let new_value = new_value.expect("set は value 検証済み");
            let target = json_navigate_mut(&mut root, segs)
                .ok_or_else(|| format!(
                    "`{}` は存在しません（存在しないキーへの set は行いません — \
                     パスの綴りを確認してください）。",
                    seg_display(segs)
                ))?;
            if matches!(target, Value::Array(_) | Value::Object(_)) {
                return Err(format!(
                    "`{}` は配列またはオブジェクトです。set できるのはスカラー値だけです\
                    （構造の置き換えは型破壊になるため行いません）。",
                    seg_display(segs)
                ));
            }
            if target == new_value {
                return Ok(YqOutcome::NoChange);
            }
            *target = new_value.clone();
            Ok(YqOutcome::Edited(render_json(&root)))
        }
        YqOp::Remove => {
            let (parent_segs, last) = segs.split_at(segs.len() - 1);
            let parent = json_navigate_mut(&mut root, parent_segs)
                .ok_or_else(|| format!("`{}` は存在しません。", seg_display(segs)))?;
            let removed = match (&mut *parent, &last[0]) {
                (Value::Object(map), PathSeg::Key(key)) => map.shift_remove(key).is_some(),
                (Value::Array(items), PathSeg::Index(index)) if *index < items.len() => {
                    items.remove(*index);
                    true
                }
                _ => false,
            };
            if !removed {
                return Err(format!("`{}` は存在しません。", seg_display(segs)));
            }
            Ok(YqOutcome::Edited(render_json(&root)))
        }
    }
}

/// JSON を整形して出力する。末尾改行つき（POSIX テキストの慣習に合わせる）。
fn render_json(root: &Value) -> String {
    let mut out = serde_json::to_string_pretty(root).unwrap_or_default();
    out.push('\n');
    out
}

/// JSON の読み取りナビゲーション。
fn json_navigate<'a>(root: &'a Value, segs: &[PathSeg]) -> Option<&'a Value> {
    let mut current = root;
    for seg in segs {
        current = match seg {
            PathSeg::Key(key) => current.as_object()?.get(key)?,
            PathSeg::Index(index) => current.as_array()?.get(*index)?,
        };
    }
    Some(current)
}

/// JSON の可変ナビゲーション。
fn json_navigate_mut<'a>(root: &'a mut Value, segs: &[PathSeg]) -> Option<&'a mut Value> {
    let mut current = root;
    for seg in segs {
        current = match seg {
            PathSeg::Key(key) => current.as_object_mut()?.get_mut(key)?,
            PathSeg::Index(index) => current.as_array_mut()?.get_mut(*index)?,
        };
    }
    Some(current)
}

// ---- TOML バックエンド ------------------------------------------------------

/// TOML の木の 1 ノード（可変参照）。
///
/// toml_edit は `Item`（テーブルの子）と `Value`（インラインの子）と
/// `Table`（配列テーブルの要素）で型が分かれるため、ナビゲーションは
/// この 3 態を跨いで進む。
enum TomlNodeMut<'a> {
    Item(&'a mut toml_edit::Item),
    Value(&'a mut toml_edit::Value),
    Table(&'a mut toml_edit::Table),
}

/// TOML の木を 1 区切りぶん降りる。
fn toml_descend<'a>(
    node: TomlNodeMut<'a>,
    seg: &PathSeg,
    full_path: &[PathSeg],
) -> Result<TomlNodeMut<'a>, String> {
    let missing = || format!("`{}` は存在しません。", seg_display(full_path));

    match (node, seg) {
        (TomlNodeMut::Item(item), PathSeg::Key(key)) => item
            .as_table_like_mut()
            .and_then(|table| table.get_mut(key))
            .map(TomlNodeMut::Item)
            .ok_or_else(missing),
        (TomlNodeMut::Item(item), PathSeg::Index(index)) => match item {
            toml_edit::Item::Value(value) => {
                toml_descend(TomlNodeMut::Value(value), seg, full_path)
            }
            toml_edit::Item::ArrayOfTables(tables) => tables
                .get_mut(*index)
                .map(TomlNodeMut::Table)
                .ok_or_else(missing),
            _ => Err(missing()),
        },
        (TomlNodeMut::Value(value), PathSeg::Key(key)) => value
            .as_inline_table_mut()
            .and_then(|table| table.get_mut(key))
            .map(TomlNodeMut::Value)
            .ok_or_else(missing),
        (TomlNodeMut::Value(value), PathSeg::Index(index)) => value
            .as_array_mut()
            .and_then(|array| array.get_mut(*index))
            .map(TomlNodeMut::Value)
            .ok_or_else(missing),
        (TomlNodeMut::Table(table), PathSeg::Key(key)) => table
            .get_mut(key)
            .map(TomlNodeMut::Item)
            .ok_or_else(missing),
        (TomlNodeMut::Table(_), PathSeg::Index(_)) => Err(missing()),
    }
}

/// TOML のスカラー値を JSON 値へ写す。写せない型（日時・コンテナ）は `None`。
fn toml_scalar_to_json(value: &toml_edit::Value) -> Option<Value> {
    match value {
        toml_edit::Value::String(s) => Some(Value::String(s.value().clone())),
        toml_edit::Value::Integer(i) => Some(Value::from(*i.value())),
        toml_edit::Value::Float(f) => serde_json::Number::from_f64(*f.value()).map(Value::Number),
        toml_edit::Value::Boolean(b) => Some(Value::Bool(*b.value())),
        _ => None,
    }
}

/// JSON スカラーを TOML 値へ写す。
fn json_scalar_to_toml(value: &Value) -> Result<toml_edit::Value, String> {
    match value {
        Value::String(s) => Ok(toml_edit::Value::from(s.as_str())),
        Value::Bool(b) => Ok(toml_edit::Value::from(*b)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(toml_edit::Value::from(i))
            } else if let Some(f) = n.as_f64() {
                Ok(toml_edit::Value::from(f))
            } else {
                Err(format!("`{n}` は TOML の数値として表現できません。"))
            }
        }
        Value::Null => Err(
            "TOML に null は存在しません。キーを消したい場合は `remove` を使ってください。".into(),
        ),
        Value::Array(_) | Value::Object(_) => unreachable!("スカラーは呼び出し側で検証済み"),
    }
}

/// TOML バックエンド。コメント・フォーマットは toml_edit が保持する。
fn yq_toml(
    text: &str,
    op: YqOp,
    segs: &[PathSeg],
    new_value: Option<&Value>,
) -> Result<YqOutcome, String> {
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|err| format!("TOML として解釈できません: {err}"))?;

    // 対象（set / get）または親（remove）まで降りる。
    let walk_len = match op {
        YqOp::Remove => segs.len() - 1,
        _ => segs.len(),
    };
    let mut node = TomlNodeMut::Item(doc.as_item_mut());
    for seg in &segs[..walk_len] {
        node = toml_descend(node, seg, segs)?;
    }

    match op {
        YqOp::Get => {
            // 表示はスカラーなら JSON 表記、日時は生表記 + 型注記、
            // コンテナは TOML 表記のまま。
            let shown = match &node {
                TomlNodeMut::Item(item) => match item {
                    toml_edit::Item::Value(value) => toml_value_display(value),
                    other => other.to_string().trim().to_string(),
                },
                TomlNodeMut::Value(value) => toml_value_display(value),
                TomlNodeMut::Table(table) => table.to_string().trim().to_string(),
            };
            Ok(YqOutcome::Value(shown))
        }
        YqOp::Set => {
            let new_json = new_value.expect("set は value 検証済み");
            let target: &mut toml_edit::Value = match node {
                TomlNodeMut::Item(item) => match item {
                    toml_edit::Item::Value(value) => value,
                    _ => {
                        return Err(format!(
                            "`{}` はテーブルです。set できるのはスカラー値だけです。",
                            seg_display(segs)
                        ));
                    }
                },
                TomlNodeMut::Value(value) => value,
                TomlNodeMut::Table(_) => {
                    return Err(format!(
                        "`{}` はテーブルです。set できるのはスカラー値だけです。",
                        seg_display(segs)
                    ));
                }
            };

            match toml_scalar_to_json(target) {
                Some(current) => {
                    if &current == new_json {
                        return Ok(YqOutcome::NoChange);
                    }
                }
                None => {
                    // 日時・配列・インラインテーブル。型破壊になるため拒否。
                    return Err(format!(
                        "`{}` は JSON に写像できない型（日時など）またはコンテナです。\
                         v1 では set できません。",
                        seg_display(segs)
                    ));
                }
            }

            let mut replacement = json_scalar_to_toml(new_json)?;
            // 値の前後の装飾（コメント・空白）は値に付随している。
            // 引き継がないと `port = 8080 # 説明` の行末コメントが消える。
            *replacement.decor_mut() = target.decor().clone();
            *target = replacement;
            Ok(YqOutcome::Edited(doc.to_string()))
        }
        YqOp::Remove => {
            let missing = || format!("`{}` は存在しません。", seg_display(segs));
            let removed = match (node, &segs[segs.len() - 1]) {
                (TomlNodeMut::Item(item), PathSeg::Key(key)) => item
                    .as_table_like_mut()
                    .and_then(|table| table.remove(key))
                    .is_some(),
                (TomlNodeMut::Item(item), PathSeg::Index(index)) => match item {
                    toml_edit::Item::Value(value) => remove_from_toml_value(value, *index),
                    toml_edit::Item::ArrayOfTables(tables) => {
                        if *index < tables.len() {
                            tables.remove(*index);
                            true
                        } else {
                            false
                        }
                    }
                    _ => false,
                },
                (TomlNodeMut::Value(value), PathSeg::Key(key)) => value
                    .as_inline_table_mut()
                    .and_then(|table| table.remove(key))
                    .is_some(),
                (TomlNodeMut::Value(value), PathSeg::Index(index)) => {
                    remove_from_toml_value(value, *index)
                }
                (TomlNodeMut::Table(table), PathSeg::Key(key)) => table.remove(key).is_some(),
                (TomlNodeMut::Table(_), PathSeg::Index(_)) => false,
            };
            if !removed {
                return Err(missing());
            }
            Ok(YqOutcome::Edited(doc.to_string()))
        }
    }
}

/// TOML 配列から 1 要素を消す。範囲外なら false。
fn remove_from_toml_value(value: &mut toml_edit::Value, index: usize) -> bool {
    match value.as_array_mut() {
        Some(array) if index < array.len() => {
            array.remove(index);
            true
        }
        _ => false,
    }
}

/// TOML 値の表示。スカラーは JSON 表記、日時は生表記 + 型注記。
fn toml_value_display(value: &toml_edit::Value) -> String {
    match toml_scalar_to_json(value) {
        Some(json) => json.to_string(),
        None => match value {
            toml_edit::Value::Datetime(dt) => format!("{}（日時型）", dt.value()),
            other => other.to_string().trim().to_string(),
        },
    }
}

/// YAML バックエンド（Phase 4 で実装）。
fn yq_yaml(
    _text: &str,
    _op: YqOp,
    _segs: &[PathSeg],
    _new_value: Option<&Value>,
) -> Result<YqOutcome, String> {
    Err("YAML はまだ対応していません（実装中）。".into())
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

    // ---- yq ----------------------------------------------------------------

    async fn call_yq(dir: &TempDir, args: serde_json::Value) -> String {
        YqTool.call(&ctx_with(Some(&dir.0)), &args).await.unwrap()
    }

    #[test]
    fn key_paths_parse_and_invalid_forms_are_rejected() {
        assert_eq!(
            parse_key_path("a.b[0].c").unwrap(),
            vec![
                PathSeg::Key("a".into()),
                PathSeg::Key("b".into()),
                PathSeg::Index(0),
                PathSeg::Key("c".into()),
            ]
        );
        assert_eq!(parse_key_path("[2]").unwrap(), vec![PathSeg::Index(2)]);
        assert!(parse_key_path("a..b").is_err(), "空セグメント");
        assert!(parse_key_path("a.\"b.c\"").is_err(), "クォートキーは v1 非対応");
        assert!(parse_key_path("a.b[").is_err(), "閉じ忘れ");
        assert!(parse_key_path("").is_err());
    }

    #[tokio::test]
    async fn yq_get_reads_a_value_and_missing_keys_are_reported() {
        let dir = TempDir::new("yq-get");
        dir.write("c.json", r#"{ "server": { "port": 8080 } }"#);

        let hit = call_yq(
            &dir,
            serde_json::json!({ "path": "c.json", "op": "get", "key": "server.port" }),
        )
        .await;
        assert!(hit.contains("8080"), "{hit}");

        let miss = call_yq(
            &dir,
            serde_json::json!({ "path": "c.json", "op": "get", "key": "server.host" }),
        )
        .await;
        assert!(miss.contains("存在しません"), "{miss}");
    }

    #[tokio::test]
    async fn yq_set_previews_then_applies_and_preserves_key_order() {
        let dir = TempDir::new("yq-set");
        dir.write("c.json", "{\n  \"zebra\": 1,\n  \"alpha\": { \"port\": 8080 }\n}\n");

        let preview = call_yq(
            &dir,
            serde_json::json!({ "path": "c.json", "op": "set", "key": "alpha.port", "value": "9090" }),
        )
        .await;
        assert!(preview.contains("preview"), "{preview}");
        assert!(dir.read("c.json").contains("8080"), "preview では書かない");

        let applied = call_yq(
            &dir,
            serde_json::json!({
                "path": "c.json", "op": "set", "key": "alpha.port", "value": "9090", "apply": true
            }),
        )
        .await;
        assert!(applied.contains("適用済み"), "{applied}");

        let saved = dir.read("c.json");
        assert!(saved.contains("9090"), "{saved}");
        let zebra = saved.find("zebra").unwrap();
        let alpha = saved.find("alpha").unwrap();
        assert!(zebra < alpha, "キー順（挿入順）が保たれること: {saved}");
    }

    #[tokio::test]
    async fn yq_set_with_the_same_value_does_not_write() {
        let dir = TempDir::new("yq-same");
        // 整形を意図的に崩したファイル。同値 set で正規化だけが走ると
        // 「空白差分」が生まれるので、値が同じなら触らないことを固定する。
        dir.write("c.json", r#"{"port":8080}"#);

        let reply = call_yq(
            &dir,
            serde_json::json!({ "path": "c.json", "op": "set", "key": "port", "value": "8080", "apply": true }),
        )
        .await;

        assert!(reply.contains("変更なし"), "{reply}");
        assert_eq!(dir.read("c.json"), r#"{"port":8080}"#, "整形の正規化も起こさない");
    }

    #[tokio::test]
    async fn yq_set_rejects_missing_paths_and_containers_and_bad_values() {
        let dir = TempDir::new("yq-reject");
        dir.write("c.json", r#"{ "server": { "port": 8080 }, "tags": [1, 2] }"#);

        let missing = call_yq(
            &dir,
            serde_json::json!({ "path": "c.json", "op": "set", "key": "server.ghost.deep", "value": "1" }),
        )
        .await;
        assert!(missing.contains("存在しません"), "中間キーを生やさない: {missing}");

        let container = call_yq(
            &dir,
            serde_json::json!({ "path": "c.json", "op": "set", "key": "server", "value": "1" }),
        )
        .await;
        assert!(container.contains("スカラー"), "コンテナ置換は型破壊: {container}");

        let container_value = call_yq(
            &dir,
            serde_json::json!({ "path": "c.json", "op": "set", "key": "server.port", "value": "[1,2]" }),
        )
        .await;
        assert!(container_value.contains("配列・オブジェクトは set できません"), "{container_value}");

        let bad_literal = call_yq(
            &dir,
            serde_json::json!({ "path": "c.json", "op": "set", "key": "server.port", "value": "bare-text" }),
        )
        .await;
        assert!(bad_literal.contains("JSON リテラル"), "{bad_literal}");

        assert!(dir.read("c.json").contains("8080"), "拒否経路では書かない");
    }

    #[tokio::test]
    async fn yq_can_address_array_elements() {
        let dir = TempDir::new("yq-array");
        dir.write("c.json", r#"{ "servers": [ { "port": 1 }, { "port": 2 } ] }"#);

        call_yq(
            &dir,
            serde_json::json!({
                "path": "c.json", "op": "set", "key": "servers[1].port", "value": "22", "apply": true
            }),
        )
        .await;

        let saved = dir.read("c.json");
        assert!(saved.contains("22"), "{saved}");
        assert!(saved.contains("\"port\": 1"), "他の要素は触らない: {saved}");
    }

    #[tokio::test]
    async fn yq_remove_deletes_a_key() {
        let dir = TempDir::new("yq-remove");
        dir.write("c.json", r#"{ "keep": 1, "drop": 2 }"#);

        let reply = call_yq(
            &dir,
            serde_json::json!({ "path": "c.json", "op": "remove", "key": "drop", "apply": true }),
        )
        .await;

        assert!(reply.contains("適用済み"), "{reply}");
        let saved = dir.read("c.json");
        assert!(!saved.contains("drop"), "{saved}");
        assert!(saved.contains("keep"), "{saved}");
    }

    #[tokio::test]
    async fn yq_rejects_unknown_extensions_before_parsing() {
        let dir = TempDir::new("yq-ext");
        // 中身は正しい JSON だが拡張子が対象外 — 検査順序 3 が 6 より先。
        dir.write("c.txt", r#"{ "port": 8080 }"#);

        let reply = call_yq(
            &dir,
            serde_json::json!({ "path": "c.txt", "op": "get", "key": "port" }),
        )
        .await;
        assert!(reply.contains("対応していない形式"), "{reply}");
    }

    #[tokio::test]
    async fn yq_reports_parse_errors_readably() {
        let dir = TempDir::new("yq-parse");
        dir.write("c.json", "{ broken");

        let reply = call_yq(
            &dir,
            serde_json::json!({ "path": "c.json", "op": "get", "key": "port" }),
        )
        .await;
        assert!(reply.contains("JSON として解釈できません"), "{reply}");
    }

    // ---- yq: TOML ----------------------------------------------------------

    /// コメントと行末コメントつきの代表的な設定ファイル。
    const TOML_SAMPLE: &str = "\
# サーバー設定
[server]
port = 8080 # 既定ポート
host = \"localhost\"

[log]
level = \"info\"
tags = [\"a\", \"b\"]

[[workers]]
name = \"w1\"
started = 2026-01-01T00:00:00Z
";

    #[tokio::test]
    async fn yq_toml_set_changes_only_the_target_and_keeps_comments() {
        let dir = TempDir::new("toml-set");
        dir.write("c.toml", TOML_SAMPLE);

        let reply = call_yq(
            &dir,
            serde_json::json!({
                "path": "c.toml", "op": "set", "key": "server.port", "value": "9090", "apply": true
            }),
        )
        .await;
        assert!(reply.contains("適用済み"), "{reply}");

        let saved = dir.read("c.toml");
        assert!(saved.contains("port = 9090 # 既定ポート"), "行末コメントが残ること: {saved}");
        assert!(saved.contains("# サーバー設定"), "{saved}");
        // 対象行以外は 1 字も変わらないこと。
        let expected = TOML_SAMPLE.replace("port = 8080 # 既定ポート", "port = 9090 # 既定ポート");
        assert_eq!(saved, expected, "diff が対象行のみであること");
    }

    #[tokio::test]
    async fn yq_toml_get_reads_scalars_and_datetimes_are_annotated() {
        let dir = TempDir::new("toml-get");
        dir.write("c.toml", TOML_SAMPLE);

        let port = call_yq(
            &dir,
            serde_json::json!({ "path": "c.toml", "op": "get", "key": "server.port" }),
        )
        .await;
        assert!(port.contains("8080"), "{port}");

        let tag = call_yq(
            &dir,
            serde_json::json!({ "path": "c.toml", "op": "get", "key": "log.tags[1]" }),
        )
        .await;
        assert!(tag.contains("\"b\""), "{tag}");

        let dt = call_yq(
            &dir,
            serde_json::json!({ "path": "c.toml", "op": "get", "key": "workers[0].started" }),
        )
        .await;
        assert!(dt.contains("日時型"), "型注記が付くこと: {dt}");
    }

    #[tokio::test]
    async fn yq_toml_rejects_type_breaking_sets() {
        let dir = TempDir::new("toml-reject");
        dir.write("c.toml", TOML_SAMPLE);

        let table = call_yq(
            &dir,
            serde_json::json!({ "path": "c.toml", "op": "set", "key": "server", "value": "1" }),
        )
        .await;
        assert!(table.contains("テーブル"), "{table}");

        let datetime = call_yq(
            &dir,
            serde_json::json!({
                "path": "c.toml", "op": "set", "key": "workers[0].started", "value": "\"2027-01-01\""
            }),
        )
        .await;
        assert!(datetime.contains("写像できない型"), "{datetime}");

        let null = call_yq(
            &dir,
            serde_json::json!({ "path": "c.toml", "op": "set", "key": "server.port", "value": "null" }),
        )
        .await;
        assert!(null.contains("null は存在しません"), "{null}");

        assert_eq!(dir.read("c.toml"), TOML_SAMPLE, "拒否経路では書かない");
    }

    #[tokio::test]
    async fn yq_toml_same_value_set_does_not_write() {
        let dir = TempDir::new("toml-same");
        dir.write("c.toml", TOML_SAMPLE);

        let reply = call_yq(
            &dir,
            serde_json::json!({
                "path": "c.toml", "op": "set", "key": "server.port", "value": "8080", "apply": true
            }),
        )
        .await;
        assert!(reply.contains("変更なし"), "{reply}");
        assert_eq!(dir.read("c.toml"), TOML_SAMPLE);
    }

    #[tokio::test]
    async fn yq_toml_remove_deletes_a_key_and_array_elements() {
        let dir = TempDir::new("toml-remove");
        dir.write("c.toml", TOML_SAMPLE);

        call_yq(
            &dir,
            serde_json::json!({ "path": "c.toml", "op": "remove", "key": "log.tags[0]", "apply": true }),
        )
        .await;
        let saved = dir.read("c.toml");
        assert!(!saved.contains("\"a\""), "{saved}");
        assert!(saved.contains("\"b\""), "{saved}");

        call_yq(
            &dir,
            serde_json::json!({ "path": "c.toml", "op": "remove", "key": "log.level", "apply": true }),
        )
        .await;
        let saved = dir.read("c.toml");
        assert!(!saved.contains("level"), "{saved}");
        assert!(saved.contains("# サーバー設定"), "他のコメントは残る: {saved}");
    }

    #[tokio::test]
    async fn yq_without_a_work_dir_explains_how_to_enable_it() {
        let reply = YqTool
            .call(
                &ctx_with(None),
                &serde_json::json!({ "path": "c.json", "op": "get", "key": "a" }),
            )
            .await
            .unwrap();
        assert!(reply.contains("作業フォルダ"), "{reply}");
    }
}
