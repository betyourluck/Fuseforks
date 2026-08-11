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
const PASSED_ENV: [&str; 8] = [
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

    let child = match command.group_spawn() {
        Ok(child) => child,
        Err(err) => return Ran::SpawnFailed(err.to_string()),
    };

    let timeout = std::time::Duration::from_secs(timeout_secs);
    let waited = match cancel {
        Some(token) => {
            tokio::select! {
                result = tokio::time::timeout(timeout, child.wait_with_output()) => Waited::Finished(result),
                () = token.cancelled() => Waited::Cancelled,
            }
        }
        None => Waited::Finished(tokio::time::timeout(timeout, child.wait_with_output()).await),
    };

    match waited {
        Waited::Finished(Ok(Ok(output))) => Ran::Finished {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        },
        Waited::Finished(Ok(Err(err))) => Ran::WaitFailed(err.to_string()),
        Waited::Finished(Err(_elapsed)) => Ran::TimedOut,
        Waited::Cancelled => Ran::Cancelled,
    }
}

/// 待ちの結果。`tokio::select!` の腕を型で分ける（`Result` の入れ子を読ませない）。
enum Waited {
    Finished(Result<std::io::Result<std::process::Output>, tokio::time::error::Elapsed>),
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
