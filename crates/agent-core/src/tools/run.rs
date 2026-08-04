//! 登録済みコマンドを実行するツール（Spec 15）。
//!
//! # シェルを介さない
//!
//! `cmd /c` / `sh -c` を経由せず、実行ファイル + 引数配列で起動する。
//! シェルへ 1 本の文字列を渡すと登録との照合は文字列のパースでしか効かず、
//! `registered && rm -rf ..` から実行対象の集合を取り出すには shell の文法を
//! 実装することになる。**パースで守る境界は必ず抜けられる。**
//!
//! # エージェントは実行ファイル名を書けない
//!
//! 書けるのは登録名だけ。これが閉じた許容の実装で、PATH 解決の曖昧性と
//! Windows のコマンド名正規化（`python` / `python.exe` / `PATHEXT` /
//! 大文字小文字）が問題ごと消える理由でもある。
//!
//! # ここは安全機構ではない
//!
//! 囲いは**登録ただ 1 つ**。作業フォルダ境界も、ごみ箱限定も、環境変数の遮断も
//! 境界ではない（`command_tool_contract`）。`env_clear` は環境変数経路だけを閉じ、
//! `~/.aws/credentials` はファイル経路で読める。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use command_group::AsyncCommandGroup;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::command::{CommandRegistry, CommandRequestLog, RegisteredCommand};
use crate::config_store::ConfigStore;
use crate::error::CoreResult;
use crate::llm::ToolSpec;
use crate::tool::{AgentTool, ToolContext};
use crate::tools::fs::MAX_OUTPUT_CHARS;

/// stderr に確保する文字数。**融通しない。**
///
/// 割合や「余りを回す」形にすると、stderr の長さで stdout の打ち切り位置が変わる。
/// RepeatGuard は結果本文の**完全一致**で数えるので、同じコマンドの 2 回目が
/// 1 文字でも違う stderr を出した瞬間に stdout 側の打ち切りごと変わり、一致が壊れる。
const STDERR_CHARS: usize = 4_000;

/// stdout に確保する文字数。`STDERR_CHARS + STDOUT_CHARS == MAX_OUTPUT_CHARS`。
const STDOUT_CHARS: usize = MAX_OUTPUT_CHARS - STDERR_CHARS;

/// 子プロセスへ渡す環境変数の名前。
///
/// `env_clear` してからこの名前だけを親からコピーする。**これは安全対策ではない** —
/// `PATH` を渡す以上、子プロセスは端末上の任意の実行ファイルへ届く。可用性のための
/// 選択で、`echo $ANTHROPIC_API_KEY` のような最も稚拙な経路を 1 つ閉じるだけ。
const PASSED_ENV: [&str; 7] = [
    "PATH",
    "SYSTEMROOT",
    "TEMP",
    "TMP",
    "HOME",
    "USERPROFILE",
    "LANG",
];

/// 登録済みコマンドを実行するツール。
pub struct RunTool {
    commands: Arc<RwLock<CommandRegistry>>,
    requests: Arc<RwLock<CommandRequestLog>>,
    store: ConfigStore,
}

impl RunTool {
    /// 登録簿・登録要求・保存先を渡して作る。
    pub fn new(
        commands: Arc<RwLock<CommandRegistry>>,
        requests: Arc<RwLock<CommandRequestLog>>,
        store: ConfigStore,
    ) -> Self {
        Self { commands, requests, store }
    }

    /// 登録要求を積んで保存する。**保存に失敗しても実行の答えは変えない**
    /// （WARN 1 行で続行する既存の規律）。
    async fn note_request(&self, name: &str, extra: &[String], ctx: &ToolContext, now_ms: u64) {
        let evicted = {
            let mut log = self.requests.write().await;
            log.record(name, extra, &ctx.agent_id, now_ms)
        };
        if let Some(note) = evicted {
            crate::note!("command request evicted: {note}");
        }
        let snapshot = self.requests.read().await.clone();
        if let Err(err) = self.store.save_command_requests(&snapshot).await {
            crate::note!("command request save failed: {err}");
        }
    }
}

#[async_trait]
impl AgentTool for RunTool {
    fn name(&self) -> &str {
        "run"
    }

    fn description(&self) -> String {
        // 個体を知らない既定の説明。実際に提示されるのは `spec_for` の側で、
        // そこで**その個体から実行できる登録だけ**を列挙する。
        "利用者が登録したコマンドを実行する。登録名でしか呼べない。".to_owned()
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "実行する登録名。説明に列挙されているものだけ"
                },
                "extraArgs": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "登録の引数の**後ろへ足す**引数。登録が追加引数を許していなければ拒否される"
                }
            },
            "required": ["name"],
            "additionalProperties": false
        })
    }

    /// **その個体から実行できる登録だけ**を列挙する。1 件も無ければ提示しない。
    fn spec_for(&self, ctx: &ToolContext) -> Option<ToolSpec> {
        // description は同期なので、ここだけ blocking read を使う。
        // 登録簿の更新は稀（利用者が設定画面を触ったとき）で、競合しない。
        let registry = self.commands.try_read().ok()?;
        let work_dir = ctx.work_dir.as_deref();
        let runnable = registry.runnable(work_dir);
        if runnable.is_empty() {
            return None;
        }

        let mut lines = String::from(
            "利用者が登録したコマンドを実行する。**登録名でしか呼べない**（実行ファイル名は書けない）。\n\
             実行できる登録:\n",
        );
        for entry in &runnable {
            lines.push_str(&format!(
                "- `{}`: {}{}\n",
                entry.spec.name,
                if entry.spec.description.trim().is_empty() {
                    "(説明なし)"
                } else {
                    entry.spec.description.trim()
                },
                if entry.spec.allow_extra_args {
                    "（extraArgs を足せます）"
                } else {
                    "（extraArgs は受け付けません）"
                }
            ));
        }

        // 使えない登録は**名前を出さず件数だけ**。使えないものにスキーマ分の
        // トークンを払わず、次の手（#44）だけを渡す。
        let hidden = registry.unrunnable_count(work_dir);
        if hidden > 0 {
            lines.push_str(&format!(
                "他に {hidden} 件の登録がありますが、今は使えません\
                 （作業フォルダ未設定か、実行ファイルが見つかりません）。\
                 必要なら利用者に設定を頼んでください。\n"
            ));
        }

        Some(ToolSpec {
            name: self.name().to_owned(),
            description: lines,
            parameters: self.parameters(),
        })
    }

    async fn call(&self, ctx: &ToolContext, args: &Value) -> CoreResult<String> {
        let Some(name) = args.get("name").and_then(Value::as_str) else {
            return Ok("引数 `name` が必要です。".into());
        };
        let extra: Vec<String> = args
            .get("extraArgs")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();

        let entry = { self.commands.read().await.get(name).cloned() };

        let Some(entry) = entry else {
            // **未登録。** ここだけが登録要求を積む経路。
            let now_ms = crate::command::now_ms();
            self.note_request(name, &extra, ctx, now_ms).await;
            return Ok(format!(
                "`{name}` は登録されていないため実行しませんでした。\n\
                 利用者が「システム設定 > コマンドの登録」で追加すると、次から実行できます。\n\
                 利用者へ依頼するか、別の手段で進めてください。\
                 **このターンでは実行できません**（登録されるまで、同じ呼び出しは何度でも失敗します）。"
            ));
        };

        // **登録済みだが追加引数を許していない。** 未登録とは別の答えにする —
        // 同じ文面だとモデルは「登録待ち」と読むが、実際は引数を外せばその周で通る。
        // 登録要求には積まない（登録は既にある）。
        if !extra.is_empty() && !entry.spec.allow_extra_args {
            return Ok(format!(
                "`{name}` は登録されていますが、追加引数を受け付けない設定です。\n\
                 **引数なしで呼び直せばこの周で実行できます。**\
                 追加引数がどうしても必要なら、利用者に登録の変更を頼んでください。"
            ));
        }

        if let Some(reason) = entry.unavailable {
            return Ok(format!(
                "`{name}` は登録されていますが実行できません: {}（{}）。\n\
                 利用者に登録し直してもらってください。",
                reason.reason_ja(),
                entry.spec.program.display()
            ));
        }

        let Some(cwd) = entry.resolve_cwd(ctx.work_dir.as_deref()) else {
            return Ok(format!(
                "`{name}` の作業フォルダが決まりません。\n\
                 利用者がエージェント設定で作業フォルダを設定するか、\
                 登録に作業フォルダを指定すると実行できます。"
            ));
        };

        // **実行時にも実在を見る。** 起動時検証だけだと、アプリを開いたまま
        // 環境を入れ替えたときに素通りする。
        if !entry.spec.program.is_file() {
            return Ok(format!(
                "`{name}` の実行ファイルが見つかりません（{}）。\n\
                 利用者に登録し直してもらってください。",
                entry.spec.program.display()
            ));
        }

        run_registered(&entry, &extra, &cwd, ctx.cancel.clone()).await
    }
}

/// 1 件の登録を実際に走らせる。**モデルへ返す文字列を組み立てるところまで。**
///
/// 単体テストから直接呼べるよう、ツールから切り離してある。
pub(crate) async fn run_registered(
    entry: &RegisteredCommand,
    extra: &[String],
    cwd: &Path,
    cancel: Option<tokio_util::sync::CancellationToken>,
) -> CoreResult<String> {
    let full_args = entry.full_args(extra);
    let mut command = tokio::process::Command::new(&entry.spec.program);
    command
        .args(&full_args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // **親の環境を丸ごと渡さない。** 名前を指定した分だけコピーする。
    command.env_clear();
    for key in PASSED_ENV {
        if let Ok(value) = std::env::var(key) {
            command.env(key, value);
        }
    }

    // **木ごと起動する。** 直接 spawn すると孫（pytest が起動した子）が
    // kill から漏れる。
    let child = match command.group_spawn() {
        Ok(child) => child,
        Err(err) => {
            return Ok(format!(
                "`{}` を起動できませんでした: {err}\n\
                 利用者に登録を確かめてもらってください。",
                entry.spec.name
            ));
        }
    };

    let timeout = std::time::Duration::from_secs(entry.timeout_secs());
    let waited = match cancel {
        Some(token) => {
            tokio::select! {
                result = tokio::time::timeout(timeout, child.wait_with_output()) => Waited::Finished(result),
                () = token.cancelled() => Waited::Cancelled,
            }
        }
        None => Waited::Finished(tokio::time::timeout(timeout, child.wait_with_output()).await),
    };

    let output = match waited {
        Waited::Finished(Ok(Ok(output))) => output,
        Waited::Finished(Ok(Err(err))) => {
            return Ok(format!("`{}` の実行に失敗しました: {err}", entry.spec.name));
        }
        Waited::Finished(Err(_elapsed)) => {
            // タイムアウト。木ごと落とす。
            return Ok(format!(
                "`{}` は {} 秒で終わらなかったため中断しました（プロセスは停止済み）。\n\
                 時間のかかる処理なら、利用者に登録のタイムアウトを延ばしてもらってください。",
                entry.spec.name,
                entry.timeout_secs()
            ));
        }
        Waited::Cancelled => {
            return Ok(format!(
                "`{}` は利用者の打ち切りにより停止しました。",
                entry.spec.name
            ));
        }
    };

    let code = output
        .status
        .code()
        .map_or_else(|| "シグナルで終了".to_owned(), |c| c.to_string());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    let mut body = format!("終了コード: {code}\n");
    body.push_str(&section("stdout", &stdout, STDOUT_CHARS));
    body.push_str(&section("stderr", &stderr, STDERR_CHARS));
    Ok(body)
}

/// 待ちの結果。`tokio::select!` の腕を型で分ける（`Result` の入れ子を読ませない）。
enum Waited {
    Finished(Result<std::io::Result<std::process::Output>, tokio::time::error::Elapsed>),
    Cancelled,
}

/// 出力を 1 節に整える。**打ち切りは母数と「続きが取れない」ことまで書く**
/// （failures.md #55 / #58 と `bundled_tools_contract` の規律）。
fn section(label: &str, text: &str, limit: usize) -> String {
    let trimmed = text.trim_end();
    if trimmed.is_empty() {
        return format!("{label}: (なし)\n");
    }
    let total = trimmed.chars().count();
    if total <= limit {
        return format!("{label}:\n{trimmed}\n");
    }
    let head: String = trimmed.chars().take(limit).collect();
    format!(
        "{label}（全 {total} 字のうち先頭 {limit} 字）:\n{head}\n\
         （表示上限に達したため打ち切りました。**続きを読む方法はありません** — \
         再実行しても同じ出力とは限りません。出力を絞る引数があるなら使ってください）\n"
    )
}

/// 実行ファイルの解決に使う `which` 相当。登録時に 1 回だけ呼ぶ想定で、
/// **実行時には使わない**（`program` は絶対パスで記録済み）。
pub fn resolve_program(name: &str) -> Option<PathBuf> {
    let candidate = Path::new(name);
    if candidate.is_absolute() {
        return candidate.is_file().then(|| candidate.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into())
            .split(';')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return candidate.canonicalize().ok().or(Some(candidate));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{CommandRegistration, DEFAULT_TIMEOUT_SECS};

    /// テスト用に「必ず在る」実行ファイルを 1 つ選ぶ。
    fn shell_program() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\Windows\System32\cmd.exe")
        } else {
            PathBuf::from("/bin/sh")
        }
    }

    /// 標準出力へ 1 行出すだけの登録。**シェルを引数として起動しているだけで、
    /// `run` 自身がシェルを介しているわけではない**（テストの都合）。
    fn echo_entry(text: &str, timeout_secs: u64) -> RegisteredCommand {
        let args = if cfg!(windows) {
            vec!["/C".to_string(), format!("echo {text}")]
        } else {
            vec!["-c".to_string(), format!("echo {text}")]
        };
        RegisteredCommand {
            spec: CommandRegistration {
                name: "echo".into(),
                description: String::new(),
                program: shell_program(),
                args,
                allow_extra_args: false,
                timeout_secs,
                cwd: None,
            },
            unavailable: None,
        }
    }

    fn sleeper(secs: u32, timeout_secs: u64) -> RegisteredCommand {
        let args = if cfg!(windows) {
            vec!["/C".to_string(), format!("ping -n {} 127.0.0.1 > NUL", secs + 1)]
        } else {
            vec!["-c".to_string(), format!("sleep {secs}")]
        };
        RegisteredCommand {
            spec: CommandRegistration {
                name: "sleep".into(),
                description: String::new(),
                program: shell_program(),
                args,
                allow_extra_args: false,
                timeout_secs,
                cwd: None,
            },
            unavailable: None,
        }
    }

    #[tokio::test]
    async fn it_returns_the_exit_code_and_stdout() {
        let entry = echo_entry("hello-concordia", DEFAULT_TIMEOUT_SECS);
        let out = run_registered(&entry, &[], Path::new("."), None).await.unwrap();
        assert!(out.contains("終了コード: 0"), "{out}");
        assert!(out.contains("hello-concordia"), "{out}");
    }

    #[tokio::test]
    async fn a_timeout_stops_the_process_and_says_how_to_fix_it() {
        let entry = sleeper(30, 1);
        let out = run_registered(&entry, &[], Path::new("."), None).await.unwrap();
        assert!(out.contains("秒で終わらなかった"), "{out}");
        assert!(
            out.contains("タイムアウトを延ばして"),
            "次の手を書くこと（#44）: {out}"
        );
    }

    #[tokio::test]
    async fn a_cancelled_run_stops_without_waiting_for_the_timeout() {
        let token = tokio_util::sync::CancellationToken::new();
        let entry = sleeper(30, 3600);
        let handle = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            handle.cancel();
        });

        let started = std::time::Instant::now();
        let out = run_registered(&entry, &[], Path::new("."), Some(token))
            .await
            .unwrap();

        assert!(out.contains("打ち切り"), "{out}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "周回境界まで待たずに止まること（実測 {:?}）",
            started.elapsed()
        );
    }

    #[test]
    fn the_output_split_is_fixed_and_never_borrows_from_the_other_side() {
        // stderr が短くても stdout の枠は広がらない（RepeatGuard の完全一致を守る）。
        let long = "あ".repeat(STDOUT_CHARS + 500);
        let body = section("stdout", &long, STDOUT_CHARS);
        assert!(body.contains(&format!("全 {} 字のうち先頭 {STDOUT_CHARS} 字", STDOUT_CHARS + 500)));
        assert!(
            body.contains("続きを読む方法はありません"),
            "続きが取れないことまで書くこと: {body}"
        );
        assert_eq!(STDERR_CHARS + STDOUT_CHARS, MAX_OUTPUT_CHARS);
    }

    #[test]
    fn an_empty_stream_is_stated_not_omitted() {
        assert_eq!(section("stderr", "   ", STDERR_CHARS), "stderr: (なし)\n");
    }

    #[tokio::test]
    async fn extra_args_are_appended_to_the_registered_args() {
        let mut entry = echo_entry("base", DEFAULT_TIMEOUT_SECS);
        entry.spec.allow_extra_args = true;
        // `echo base` の後ろへ `extra` を足す形になる（シェル引数として連結される）。
        let out = run_registered(&entry, &["extra".to_string()], Path::new("."), None)
            .await
            .unwrap();
        assert!(out.contains("base"), "{out}");
        assert!(out.contains("extra"), "登録の引数を置き換えないこと: {out}");
    }

    #[test]
    fn resolving_an_absolute_program_requires_it_to_exist() {
        assert!(resolve_program("/definitely/not/here/xyzzy").is_none());
        let real = shell_program();
        assert_eq!(
            resolve_program(real.to_str().unwrap()).as_deref(),
            Some(real.as_path())
        );
    }
}
