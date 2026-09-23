//! 子プロセス 1 本の**起動と待ち**。整形はしない。
//!
//! ## 2 つの呼び出し元が共有する理由
//!
//! - `run` ツール（Spec 15）— モデルが書いたコマンドを、閉じた許容の照合を
//!   通してから走らせる
//! - 予定の前判定（Spec 28）— 人が書いたコマンドを、端末の承認を確かめてから走らせる
//!
//! **照合と承認は共有しない。**囲いは 2 つの呼び出し元でそれぞれ別物で、
//! ここが持つのは「プロセスをどう起こし、どう確実に殺すか」だけ。
//! 写して 2 つ目を書くと、**木ごと kill の修正が片方にしか入らない**形になる
//! （孫が生き残る経路は静かで、テストにも画面にも出ない）。
//!
//! ## 生の結果を返す
//!
//! [`Ran`] は stdout / stderr / 終了コードをそのまま運ぶ。整形して文字列を返すと、
//! 前判定が「モデル向けに整えた本文」から合図を読み直す羽目になる —
//! 判定が表示の都合に依存する形は、表示を直した瞬間に黙って壊れる。

use std::path::{Path, PathBuf};
use std::process::Stdio;

use command_group::AsyncCommandGroup;

/// 子プロセスへ渡す環境変数の名前。
///
/// `env_clear` してからこの名前だけを親からコピーする。**これは安全対策ではない** —
/// `PATH` を渡す以上、子プロセスは端末上の任意の実行ファイルへ届く。可用性のための
/// 選択で、`echo $ANTHROPIC_API_KEY` のような最も稚拙な経路を 1 つ閉じるだけ。
const PASSED_ENV: [&str; 9] = [
    "PATH",
    "SYSTEMROOT",
    "TEMP",
    "TMP",
    "HOME",
    "USERPROFILE",
    "LANG",
    // **Windows OpenSSH はこれが環境ブロックに無いと起動直後に死ぬ**
    // （2026-08-12 実測）。`C:\Windows\System32\OpenSSH\ssh.exe` は
    // **`-V`（版を出すだけで接続も設定読みもしない）ですら
    // 出力ゼロで exit 255** になり、`ProgramData` を足すと通る。
    // 13 個の候補を 1 つずつ足して、通ったのはこれだけだった。
    //
    // **値は何でもよい。** 存在しないパスでも空文字でも通るので、
    // `%ProgramData%\ssh\ssh_config` を読むからではない
    // （この端末にそのファイルは無い）。**「環境ブロックに在る」ことだけが条件**で、
    // 内部で何が起きているかは分かっていない。**再現する規則は確定、機序は未確定。**
    //
    // Git 版の `ssh.exe` は無くても動くので、**どちらが PATH で先に来るかで
    // 症状が出たり出なかったりする**（`run:` 行の `resolved=` で読める）。
    //
    // 綴りは大小どれでも通ることを実測済み（`PROGRAMDATA` / `ProgramData` /
    // `programdata` の 3 通りとも exit 0）。**Windows の環境ブロックは
    // 参照が大小を区別しない**ので、`SYSTEMROOT` と同じ流儀に揃えてある。
    "PROGRAMDATA",
    // **PowerShell はこれが無いと `& 'C:\…\lake.exe' build` を黙って飛ばす**
    // （2026-09-24 実測。`failures.md` #138）。`pwsh -NoProfile -Command & '<絶対パス>.exe' …`
    // が **exit 0・stdout 0 字・stderr 0 字・0.6 秒**で返り、`$LASTEXITCODE` も空 =
    // ネイティブのコマンドを 1 度も起動していない。`PATHEXT` を足すと同じ呼び出しが
    // `Build completed successfully (141 jobs)` まで走る。12 個の候補（`LOCALAPPDATA` /
    // `APPDATA` / `COMSPEC` / `USERNAME` / `HOMEDRIVE` / `HOMEPATH` / `WINDIR` /
    // `PROGRAMFILES` / `ELAN_HOME` / `ALLUSERSPROFILE` / `PUBLIC` / これ）を 1 つずつ
    // 足して、通ったのはこれだけ。PowerShell は `.exe` を「実行できる拡張子」と
    // 判定するのに `PATHEXT` を読む — 無ければ `&` の対象が「アプリケーション」に
    // 分類されず、何もしない。`lake.exe` を直接起動する経路（`pwsh` を挟まない）は
    // `PATHEXT` 無しでも走るので、症状は **シェル経由のときだけ**出る。
    //
    // `PROGRAMDATA` と同じく可用性のための追加で、境界は動かない（`PATHEXT` の値は
    // 拡張子の一覧で、秘密でも権限でもない）。
    "PATHEXT",
];

/// プロセス 1 本を走らせた結果。**整形前の生の値。**
#[derive(Debug)]
pub enum Ran {
    /// 最後まで走った。
    Finished {
        /// 終了コード。シグナルで落ちた場合は `None`。
        code: Option<i32>,
        /// 標準出力（不正な UTF-8 は置換済み）。
        stdout: String,
        /// 標準エラー出力（同上）。
        stderr: String,
    },
    /// 起動そのものに失敗した（権限・実行形式など）。
    SpawnFailed(String),
    /// 起動はしたが待ちで失敗した。
    WaitFailed(String),
    /// 打ち切り時間に達した。**プロセスは木ごと停止済み。**
    TimedOut,
    /// 利用者の打ち切り（`CancellationToken`）で停止した。
    Cancelled,
}

/// 子プロセスを木ごと起こし、終わるまで待つ。
///
/// **木ごと起動する**のは、直接 spawn すると孫（`pytest` が起動した子）が
/// kill から漏れるため。Windows は Job Object、Unix は `setsid` + `kill(-pgid)` が
/// 要るが、どちらも `unsafe` を書くことになる — このクレートは
/// `unsafe_code = "forbid"` なので、`command-group` に委ねている。
pub async fn spawn_and_wait(
    program: &Path,
    argv: &[String],
    cwd: &Path,
    timeout_secs: u64,
    cancel: Option<tokio_util::sync::CancellationToken>,
) -> Ran {
    let mut command = tokio::process::Command::new(program);
    command
        .args(argv)
        .current_dir(cwd)
        // **`Stdio::null()` ではなく、開いてすぐ閉じたパイプ。**
        // どちらも「入力は無い」つもりだが、**Windows OpenSSH は NUL から
        // EOF を受け取らない** — `ssh outcasts` が MOTD まで出したあと
        // リモートの bash が stdin を待ち続け、107 秒ぶら下がって exit 255
        // になる（2026-08-12 実測。`-T` を足すと 420 秒でも返らず悪化した）。
        // **閉じたパイプなら 989 ms・exit 0 で返る。**
        //
        // **tty の問題ではない。** MOTD は `pam_motd` が tty の有無に関係なく
        // 出すので、出ていることは pty の確保を意味しない。
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // **親の環境を丸ごと渡さない。** 名前を指定した分だけコピーする。
    command.env_clear();
    for key in PASSED_ENV {
        if let Ok(value) = std::env::var(key) {
            command.env(key, value);
        }
    }

    let mut child = match command.group_spawn() {
        Ok(child) => child,
        Err(err) => return Ran::SpawnFailed(err.to_string()),
    };

    // **パイプは待ちと並行に汲む。** 汲まずに `wait()` だけすると、出力の多い子が
    // パイプ満杯で止まり、待ちが永久に返らない（`wait_with_output` が中で
    // やっていることを、こちらは `child` を手放さずにやる）。
    //
    // **`wait_with_output` を使わないのはそのため** — あれは `self` を取るので、
    // 打ち切りのときに `child.kill()` を呼ぶ手が残らない。**それがこの関数に
    // kill が 1 つも無かった理由**で、`group_spawn` で殺せる形に作ってあるのに
    // 殺す呼び出しが無いまま `Ran::Cancelled` を返していた（実機で ssh が
    // 打ち切り後も 5 分生き残った。2026-08-12）。
    // **握った stdin は即座に落とす。** これが EOF になる。
    // 持ったままだと閉じないので、`Stdio::null()` のときと同じ形で
    // 相手が待ち続ける — **開くことではなく閉じることが処方**。
    drop(child.inner().stdin.take());

    let out_pipe = child.inner().stdout.take();
    let err_pipe = child.inner().stderr.take();
    let drain = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let (mut out, mut err) = (Vec::new(), Vec::new());
        match (out_pipe, err_pipe) {
            (Some(mut o), Some(mut e)) => {
                // **同時に読む。** 片方ずつ読むと、もう片方のパイプが満杯に
                // なった子が止まり、読んでいる側も終わらない。
                let _ = tokio::try_join!(o.read_to_end(&mut out), e.read_to_end(&mut err));
            }
            (Some(mut o), None) => {
                let _ = o.read_to_end(&mut out).await;
            }
            (None, Some(mut e)) => {
                let _ = e.read_to_end(&mut err).await;
            }
            (None, None) => {}
        }
        (out, err)
    });

    let timeout = std::time::Duration::from_secs(timeout_secs);
    let waited = match cancel {
        Some(token) => {
            tokio::select! {
                result = tokio::time::timeout(timeout, child.wait()) => match result {
                    Ok(status) => Waited::Finished(status),
                    Err(_elapsed) => Waited::TimedOut,
                },
                () = token.cancelled() => Waited::Cancelled,
            }
        }
        None => match tokio::time::timeout(timeout, child.wait()).await {
            Ok(status) => Waited::Finished(status),
            Err(_elapsed) => Waited::TimedOut,
        },
    };

    // **走り切らなかったときは必ずプロセス木ごと落とす**（`command-group` の
    // `kill` は Windows の Job Object / Unix のプロセスグループへ効く）。
    // ここを省くと、`cmd /c` が起こした孫だけでなく**本人も生き残る**。
    if matches!(waited, Waited::TimedOut | Waited::Cancelled) {
        let _ = child.kill().await;
    }

    // kill か終了でパイプが閉じるので、汲み取りはここで必ず返る。
    let (stdout, stderr) = drain.await.unwrap_or_default();

    match waited {
        Waited::Finished(Ok(status)) => Ran::Finished {
            code: status.code(),
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        },
        Waited::Finished(Err(err)) => Ran::WaitFailed(err.to_string()),
        Waited::TimedOut => Ran::TimedOut,
        Waited::Cancelled => Ran::Cancelled,
    }
}

/// 待ちの結果。`tokio::select!` の腕を型で分ける（`Result` の入れ子を読ませない）。
enum Waited {
    Finished(std::io::Result<std::process::ExitStatus>),
    TimedOut,
    Cancelled,
}

/// 表示用にパスを整える。**Windows の冗長プレフィックスを剥がす。**
///
/// `canonicalize()` は Windows で `\\?\C:\...` を返す。これがそのまま結果本文へ
/// 入ると、**モデルが読むテキストに OS の内部表現が漏れる**（実機で観測、
/// 2026-08-04 — `resolved=\\?\C:\Windows\System32\curl.exe`）。
/// Spec 09 Notes 1 が「観測されたら `dunce` を入れる」と書いた条件だが、
/// **crate を足さずに済む** — 剥がすのは前置 4 文字だけ。ただし UNC 形
/// （`\\?\UNC\...`）は**触らない**（剥がすと別のホストを指す）。
pub fn display_path(path: &Path) -> String {
    let text = path.display().to_string();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with(r"UNC\") => rest.to_owned(),
        _ => text,
    }
}

/// 実行ファイルの解決（`which` 相当）。
///
/// **呼ぶたびに引き直す。キャッシュしない** — `PATH` を直したら次の実行から
/// 変わってほしい。解決結果は呼び出し元が計器へ出す（どのバイナリが走ったかは
/// `PATH` 次第で変わるので、「構造的に決まる」と言わずに毎回見せる）。
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

    /// **`PATHEXT` を渡さないと、PowerShell は `& '<絶対パス>.exe'` を黙って飛ばす**
    /// （2026-09-24 実機。`failures.md` #138）。exit 0・出力 0 字で返るので、
    /// `run` の結果だけ見ると「走ったが何も出なかった」と読めてしまう。
    ///
    /// Windows だけ。`pwsh` が PATH に無い端末では何も主張しない（skip）。
    #[cfg(windows)]
    #[tokio::test]
    async fn powershell_can_invoke_an_exe_by_path_with_the_passed_env() {
        let Some(pwsh) = resolve_program("pwsh") else {
            eprintln!("pwsh が PATH に無いので skip");
            return;
        };
        let system_root = std::env::var("SYSTEMROOT").unwrap_or_else(|_| r"C:\Windows".into());
        let whoami = format!(r"{system_root}\System32\whoami.exe");
        let argv: Vec<String> = ["-NoProfile", "-Command", "&", &format!("'{whoami}'")]
            .map(String::from)
            .to_vec();
        let cwd = std::env::temp_dir();
        match spawn_and_wait(&pwsh, &argv, &cwd, 60, None).await {
            Ran::Finished { code, stdout, .. } => {
                assert_eq!(code, Some(0));
                assert!(
                    !stdout.trim().is_empty(),
                    "`& '<絶対パス>.exe'` が何も出さずに exit 0 で返った = PATHEXT が渡っていない"
                );
            }
            other => panic!("Finished を期待したが {other:?}"),
        }
    }
}
