//! 許可されたコマンドを実行するツール（Spec 15 rev4）。
//!
//! # シェルを介さない
//!
//! `cmd /c` / `sh -c` を経由せず、実行ファイル + 引数配列で起動する。
//! シェルへ 1 本の文字列を渡すと allow / deny の照合は文字列のパースでしか効かず、
//! `allowed && rm -rf ..` から実行対象の集合を取り出すには shell の文法を
//! 実装することになる。**パースで守る境界は必ず抜けられる。**
//!
//! **argv 配列で受ければ、照合対象も argv 配列になる** — `&&` も `|` も
//! `$(...)` も構造的に存在しない。登録制から allow / deny へ変えても、
//! この根拠は 1 文字も弱まらない。
//!
//! # PATH は実行時に引く
//!
//! 登録制と違い絶対パスを持たないので、**「構造的に決まる」とは言えない**。
//! 代わりに**解決した結果を毎回、結果本文とログへ出す**。キャッシュしないのは、
//! `PATH` を直したら次の実行から変わってほしいため。
//!
//! # ここは安全機構ではない
//!
//! 囲いは **allow / deny ただ 1 つ**。作業フォルダ境界も、ごみ箱限定も、
//! 環境変数の遮断も境界ではない（`command_tool_contract`）。`env_clear` は
//! 環境変数経路だけを閉じ、`~/.aws/credentials` はファイル経路で読める。
//! **deny も網羅できない** — `rm` を弾いても `python -c` が残る。

use std::path::Path;

use async_trait::async_trait;
use serde_json::Value;

use crate::command::{CommandPolicy, Decision};
use crate::config_store::ConfigStore;
use crate::error::CoreResult;
use crate::llm::ToolSpec;
use crate::model::AgentId;
use crate::process::{Ran, display_path, resolve_program};
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

/// 許可されたコマンドを実行するツール。
///
/// **ポリシーは呼び出しの瞬間にファイルから読む。** 保持しないのは、利用者が
/// `run.json` を手で直したら**次のターンから効いてほしい**ため
/// （`ToolContext.work_dir` を「呼び出しの瞬間に world から引く」のと同じ考え方）。
pub struct RunTool {
    store: ConfigStore,
}

impl RunTool {
    /// 設定ファイルの置き場を指定して作る。
    pub fn new(store: ConfigStore) -> Self {
        Self { store }
    }

    /// 照合のために `run.json` を読む。**判定は必ず fail closed 側へ倒す。**
    ///
    /// 未作成・空・壊れた JSON はすべて既定（`allow` も `deny` も空 = 全部
    /// pending）。**読みでは既定へ落としてよい** — 何も実行できなくなるだけで、
    /// 危険側へは倒れない。
    ///
    /// **書き戻す経路では既定へ落とさない**（`update_command_policy`）。
    /// 落とした既定をファイルへ書くと、人が書いた `allow` / `deny` が消える。
    async fn load(&self, id: &AgentId) -> CommandPolicy {
        match self.store.read_command_policy(id).await {
            Ok(policy) => policy,
            Err(err) => {
                crate::note!("command policy broken: agent={id} err={err}");
                CommandPolicy::default()
            }
        }
    }

    /// `pending` を 1 件足して書き戻す。
    ///
    /// **全文上書きにしない。** 読み直し → 差分適用 → 3 回 retry は
    /// `ConfigStore::update_command_policy` が持つ（承認経路と同じ 1 実装）。
    /// 失敗は WARN 1 行で続行する — **保存の失敗で実行の答えを変えない**。
    ///
    /// **壊れた JSON では何も書かない。** 以前はここで既定へ落として書き戻して
    /// おり、実測で人の `allow` / `deny` が両方消えた。
    async fn note_pending(&self, id: &AgentId, command: &str, args: &[String]) {
        let result = self
            .store
            .update_command_policy(id, |policy| {
                policy.note_pending(command, args, crate::command::now_ms())
            })
            .await;
        match result {
            Ok(Some(note)) => crate::note!("command pending evicted: agent={id} {note}"),
            Ok(None) => {}
            Err(err) => {
                crate::note!("command pending save failed: agent={id} command={command} err={err}")
            }
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
        // そこで**その個体の allow だけ**を列挙する。
        "利用者が許可したコマンドを実行する。".to_owned()
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "実行ファイル名。パス区切り（/ \\ :）を含めてはいけない"
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "引数。シェルは介さないので、パイプやリダイレクトは使えない"
                }
            },
            "required": ["command"],
            "additionalProperties": false
        })
    }

    /// **その個体の `allow` だけ**を列挙する。1 件も無ければ提示しない。
    ///
    /// `deny` は見せない — 見せると「やってはいけないこと」の一覧を毎ターン積む
    /// ことになり、**トークンを払って禁止の方法を教える**形になる。
    async fn spec_for(&self, ctx: &ToolContext) -> Option<ToolSpec> {
        // **提示も判定も同じ経路でファイルを読む。** 写しを持つと、
        // 「利用者が今さっき許可したのに提示されない」が生まれる。
        let policy = self.load(&ctx.agent_id).await;

        // **`allow` が空でも提示する**（2026-08-06 利用者裁定）。提示しないと
        // 呼び出しがフィルタで弾かれ、`pending` へ 1 件も積めない —
        // 承認画面に何も出ず、**1 件目だけは人が JSON を手で書くしかない**
        // 閉じた輪になっていた。**fail closed は実行についての性質**で、
        // `decide` が単独で守る（提示しても `allow` に無いものは 1 つも走らない）。
        if !policy.allows_anything() {
            return Some(ToolSpec {
                name: self.name().to_owned(),
                description: String::from(
                    "利用者が許可したコマンドを実行する。**シェルは介さない**（パイプ・\
                     リダイレクト・変数展開は使えない）。\n\
                     **いま許可されているコマンドは 1 つも無いので、この道具では\
                     何も実行できない。** 呼ぶと利用者への要求として記録され、\
                     利用者が承認すれば次のターンから実行できるようになる。\
                     急ぐなら利用者へ依頼するか、別の手段で進めること。\n",
                ),
                parameters: self.parameters(),
            });
        }

        let mut text = String::from(
            "利用者が許可したコマンドを実行する。**シェルは介さない**（パイプ・\
             リダイレクト・変数展開は使えない）。実行できるのは次のパターンに\
             一致する呼び出しだけ:\n",
        );
        for pattern in &policy.allow {
            text.push_str(&format!("- `{pattern}`\n"));
        }
        text.push_str(
            "パターン末尾の `*` は「以降の引数は自由」。`*` が無いパターンは\
             **引数なしの呼び出しにしか一致しない**。\
             一致しない呼び出しは実行されず、利用者への要求として記録される。\n",
        );

        Some(ToolSpec {
            name: self.name().to_owned(),
            description: text,
            parameters: self.parameters(),
        })
    }

    async fn call(&self, ctx: &ToolContext, args: &Value) -> CoreResult<String> {
        let Some(command) = args.get("command").and_then(Value::as_str) else {
            return Ok("引数 `command` が必要です。".into());
        };
        let argv: Vec<String> = args
            .get("args")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();

        // **判定は必ずファイルを読み直す。** 利用者が今さっき許可したものが
        // その周で通ってほしい。
        let policy = self.load(&ctx.agent_id).await;

        // **判断は必ず 1 行残す。** 通ったときだけログに出す形にしていたので、
        // 「deny が効いたのか、deny を書く前に積まれた行が残っているのか」を
        // 実機のログから区別できなかった（2026-08-04 の利用者報告）。
        // 機構が動いているかどうかは、動いた記録が無ければ確かめられない（#58 の同型）。
        let decision = policy.decide(command, &argv);
        crate::note!(
            "run decision: agent={} command={command} args={} decision={decision:?} \
             allow={} deny={} pending={}",
            ctx.agent_id,
            argv.len(),
            policy.allow.len(),
            policy.deny.len(),
            policy.pending.len(),
        );

        // 文面は 3 つとも別にする（#44。理由が違えば次の手も違う）。
        match decision {
            Decision::Malformed => {
                return Ok(format!(
                    "`{command}` は実行ファイル名として受け付けられません\
                     （パス区切り `/` `\\` `:` を含めないでください）。\n\
                     名前だけで呼び直してください（探索は PATH に任せます）。"
                ));
            }
            Decision::Denied => {
                return Ok(format!(
                    "`{command}` は利用者が禁止しています。\n\
                     **この呼び出しは何度試しても通りません。** 別の手段で進めてください。"
                ));
            }
            Decision::Unknown => {
                self.note_pending(&ctx.agent_id, command, &argv).await;
                return Ok(format!(
                    "`{command}` は許可されていないため実行しませんでした。\
                     利用者への要求として記録しました。\n\
                     利用者が `agents/{}/run.json` の `allow` へ追加すると、\
                     次から実行できます。\n\
                     **このターンでは実行できません。** 利用者へ依頼するか、\
                     別の手段で進めてください。",
                    ctx.agent_id
                ));
            }
            Decision::Allowed => {}
        }

        let Some(cwd) = ctx.work_dir.clone() else {
            return Ok(
                "作業フォルダが設定されていないため実行できません。\
                 利用者にエージェント設定の「作業フォルダ」欄を設定してもらってください。"
                    .into(),
            );
        };

        // **実行時に PATH で解決する。** 登録制と違い絶対パスを持っていないので、
        // 「構造的に決まる」とは言えない — 代わりに**決まった結果を毎回見せる**。
        // キャッシュしないのは、PATH を直したら次の実行から変わってほしいため。
        let Some(program) = resolve_program(command) else {
            return Ok(format!(
                "`{command}` が PATH に見つかりません。\n\
                 利用者に、そのコマンドがこの端末へ入っているか確かめてもらってください。"
            ));
        };

        crate::note!(
            "run: agent={} command={command} resolved={} args={}",
            ctx.agent_id,
            display_path(&program),
            argv.len()
        );

        run_program(
            command,
            &program,
            &argv,
            &cwd,
            policy.timeout_secs(),
            ctx.cancel.clone(),
        )
        .await
    }
}

/// 1 本のコマンドを実際に走らせる。**モデルへ返す文字列を組み立てるところまで。**
///
/// 単体テストから直接呼べるよう、ツールから切り離してある。
/// **rev4 で解決の側だけを差し替え、この関数の中身は触っていない。**
pub(crate) async fn run_program(
    label: &str,
    program: &Path,
    argv: &[String],
    cwd: &Path,
    timeout_secs: u64,
    cancel: Option<tokio_util::sync::CancellationToken>,
) -> CoreResult<String> {
    // 起動と待ちは crate::process が持つ（Spec 28 で予定の前判定と共有した）。
    // **ここに残っているのはモデルへ返す本文の組み立てだけ。**
    let (code, stdout, stderr) =
        match crate::process::spawn_and_wait(program, argv, cwd, timeout_secs, cancel).await {
            Ran::Finished {
                code,
                stdout,
                stderr,
            } => (
                code.map_or_else(|| "シグナルで終了".to_owned(), |c| c.to_string()),
                stdout,
                stderr,
            ),
            Ran::SpawnFailed(err) => {
                return Ok(format!("`{label}` を起動できませんでした: {err}"));
            }
            Ran::WaitFailed(err) => {
                return Ok(format!("`{label}` の実行に失敗しました: {err}"));
            }
            Ran::TimedOut => {
                return Ok(format!(
                    "`{label}` は {timeout_secs} 秒で終わらなかったため中断しました\
                     （プロセスは停止済み）。\n\
                     時間のかかる処理なら、利用者に `run.json` の `timeoutSecs` を\
                     延ばしてもらってください。"
                ));
            }
            Ran::Cancelled => {
                return Ok(format!("`{label}` は利用者の打ち切りにより停止しました。"));
            }
        };

    // **どのバイナリが走ったかを毎回見せる**（PATH で変わるため）。
    let mut body = format!("実行: {}\n終了コード: {code}\n", display_path(program));
    body.push_str(&section("stdout", &stdout, STDOUT_CHARS));
    body.push_str(&section("stderr", &stderr, STDERR_CHARS));
    Ok(body)
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::command::DEFAULT_TIMEOUT_SECS;

    /// テスト用に「必ず在る」実行ファイルを 1 つ選ぶ。
    fn shell_program() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\Windows\System32\cmd.exe")
        } else {
            PathBuf::from("/bin/sh")
        }
    }

    /// 標準出力へ 1 行出すだけの引数列。**`run` 自身がシェルを介しているわけでは
    /// ない** — シェルを引数として起動しているだけ（テストの都合）。
    fn echo_args(text: &str) -> Vec<String> {
        if cfg!(windows) {
            vec!["/C".to_string(), format!("echo {text}")]
        } else {
            vec!["-c".to_string(), format!("echo {text}")]
        }
    }

    fn sleep_args(secs: u32) -> Vec<String> {
        if cfg!(windows) {
            vec!["/C".to_string(), format!("ping -n {} 127.0.0.1 > NUL", secs + 1)]
        } else {
            vec!["-c".to_string(), format!("sleep {secs}")]
        }
    }

    /// **壊れた `run.json` を、既定で上書きしない。**
    ///
    /// 旧実装は読みで既定へ落とし、その既定を書き戻していた。実測で人が書いた
    /// `allow: ["git log *"]` と `deny: ["rm *"]` が**両方消えた**。
    /// 読みの寛容さ（fail closed）と書きの寛容さ（データ損失）は別物。
    #[tokio::test]
    async fn a_broken_policy_is_never_overwritten_with_the_default() {
        let dir = std::env::temp_dir().join(format!(
            "fuseforks-broken-policy-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("agents").join("agent_01")).unwrap();
        let path = dir.join("agents").join("agent_01").join("run.json");
        let broken = r#"{"allow":["git log *"],"deny":["rm *"]} oops"#;
        std::fs::write(&path, broken).unwrap();

        let tool = RunTool::new(ConfigStore::new(&dir));
        let id = AgentId::from("agent_01");
        tool.note_pending(&id, "cargo", &["test".to_string()]).await;

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            broken,
            "壊れたファイルは 1 バイトも変えない（人が直せる状態のまま残す）"
        );
        // 照合側は既定へ落ちるので、実行は許されない（fail closed は保つ）。
        assert_eq!(tool.load(&id).await.decide("git", &["log".to_string()]), Decision::Unknown);

        let _ = std::fs::remove_dir_all(&dir);
    }
    /// **`allow` が空でも `run` を提示する。**（2026-08-06 利用者裁定）
    ///
    /// 提示しないと呼び出しがフィルタで弾かれ、`pending` へ 1 件も積めない。
    /// 承認画面に何も出ないので、**1 件目だけは人が JSON を手で書くしかない**
    /// 閉じた輪になっていた（allow 空 → 非提示 → 呼べない → pending 空 →
    /// 承認できない → allow 空のまま）。
    ///
    /// **このテストが留めているのは「親切心で 2 段目のゲートを戻さない」こと。**
    /// 決定を文章だけで残すと、次に読む人が fail closed のつもりで足す
    /// （Spec 18 D13 と同じ形）。**fail closed は `decide` が単独で守る。**
    #[tokio::test]
    async fn it_is_offered_even_when_nothing_is_allowed_yet() {
        let dir = std::env::temp_dir().join(format!(
            "fuseforks-empty-allow-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let tool = RunTool::new(ConfigStore::new(&dir));
        let ctx = ToolContext {
            agent_id: AgentId::from("agent_01"),
            work_dir: Some(dir.clone()),
            cancel: None,
            rag_roots: Vec::new(),
        };

        let spec = tool
            .spec_for(&ctx)
            .await
            .expect("allow が空でも提示する（要求を積む経路がここにしか無い）");
        assert!(
            spec.description.contains("何も実行できない"),
            "提示する以上、いま何もできないことを本文で言い切る: {}",
            spec.description
        );

        // 提示しても実行はされない。fail closed は decide が守る。
        let policy = tool.load(&ctx.agent_id).await;
        assert_eq!(policy.decide("cargo", &["test".to_string()]), Decision::Unknown);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn it_returns_the_resolved_path_exit_code_and_stdout() {
        let program = shell_program();
        let out = run_program(
            "echo",
            &program,
            &echo_args("hello-concordia"),
            Path::new("."),
            DEFAULT_TIMEOUT_SECS,
            None,
        )
        .await
        .unwrap();

        assert!(out.contains("終了コード: 0"), "{out}");
        assert!(out.contains("hello-concordia"), "{out}");
        assert!(
            out.contains(&program.display().to_string()),
            "どのバイナリが走ったかを毎回見せること（PATH で変わるため）: {out}"
        );
    }

    #[tokio::test]
    async fn a_timeout_stops_the_process_and_says_how_to_fix_it() {
        let out = run_program("sleep", &shell_program(), &sleep_args(30), Path::new("."), 1, None)
            .await
            .unwrap();
        assert!(out.contains("秒で終わらなかった"), "{out}");
        assert!(
            out.contains("timeoutSecs"),
            "次の手を書くこと（#44。直す場所の名前まで出す）: {out}"
        );
    }

    #[tokio::test]
    async fn a_cancelled_run_stops_without_waiting_for_the_timeout() {
        let token = tokio_util::sync::CancellationToken::new();
        let handle = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            handle.cancel();
        });

        let started = std::time::Instant::now();
        let out = run_program(
            "sleep",
            &shell_program(),
            &sleep_args(30),
            Path::new("."),
            3600,
            Some(token),
        )
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

    #[test]
    fn the_verbatim_prefix_never_reaches_the_model() {
        // canonicalize が返す Windows の冗長形。モデルの読む本文へ漏らさない。
        assert_eq!(display_path(Path::new(r"\\?\C:\bin\x.exe")), r"C:\bin\x.exe");
        // UNC は剥がさない（剥がすと別のホストを指す）。
        assert_eq!(
            display_path(Path::new(r"\\?\UNC\host\share\x")),
            r"\\?\UNC\host\share\x"
        );
        assert_eq!(display_path(Path::new("/usr/bin/x")), "/usr/bin/x");
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
