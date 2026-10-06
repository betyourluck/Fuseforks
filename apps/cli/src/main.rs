//! `fuseforks-cli` — Fuseforks を GUI なしで動かす（Spec 64）。
//!
//! 命令は 3 つ: `check`（起動前検査だけ）/ `ask`（1 通送って答えを出して閉じる）/
//! `serve`（常駐して予定と扉を回す）。組み立ては GUI と同じ `fuseforks_host::build_host`
//! の 1 実装で、違いは `HostBootOptions` の 4 欄だけ。

mod args;
mod exit;
mod output;
mod run;

use std::process::ExitCode;
use std::time::Duration;

use args::Command;
use output::{cli_line, Level};

fn main() -> ExitCode {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let command = match args::parse(&raw) {
        Ok(command) => command,
        Err(reason) => {
            // 引数の誤りでも `--events jsonl` を書いていたなら JSON で出す（凍結 13 —
            // 標準エラーの全行が JSON。解析に失敗した後でも約束は守る）。
            if wants_jsonl(&raw) {
                eprintln!("{}", cli_line(true, Level::Error, "USAGE", &reason));
            } else {
                eprintln!("{}\n\n{}", cli_line(false, Level::Error, "USAGE", &reason), args::USAGE);
            }
            return ExitCode::from(exit::USAGE);
        }
    };
    match command {
        Command::Help => {
            print!("{}", args::USAGE);
            return ExitCode::from(exit::OK);
        }
        Command::Version => {
            println!("fuseforks-cli {}", run::VERSION);
            return ExitCode::from(exit::OK);
        }
        _ => {}
    }
    if wants_jsonl(&raw) {
        // ログを開く前の `note!` から JSON にする（組み立ての最初の行で破れないように）。
        fuseforks_core::diag::set_stderr_json(true);
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!(
                "{}",
                cli_line(wants_jsonl(&raw), Level::Error, "RUNTIME", &err.to_string())
            );
            return ExitCode::from(exit::BOOT);
        }
    };
    let code = runtime.block_on(run::dispatch(command));
    // MCP の子プロセスや待ちの残ったタスクで終了が延びないよう、短く打ち切る。
    runtime.shutdown_timeout(Duration::from_secs(2));
    ExitCode::from(code)
}

/// 生の引数に `--events jsonl`（`--events=jsonl`）が書かれているか。`--` の後は依頼文
/// なので数えない。
fn wants_jsonl(raw: &[String]) -> bool {
    let mut iter = raw.iter().take_while(|arg| arg.as_str() != "--").peekable();
    while let Some(arg) = iter.next() {
        if arg == "--events=jsonl" {
            return true;
        }
        if arg == "--events" && iter.peek().is_some_and(|next| next.as_str() == "jsonl") {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::wants_jsonl;

    fn argv(line: &[&str]) -> Vec<String> {
        line.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn jsonl_is_detected_in_both_forms_but_not_after_double_dash() {
        assert!(wants_jsonl(&argv(&["ask", "--events", "jsonl"])));
        assert!(wants_jsonl(&argv(&["serve", "--events=jsonl"])));
        assert!(!wants_jsonl(&argv(&["ask", "--", "--events", "jsonl"])));
        assert!(!wants_jsonl(&argv(&["ask", "--events", "json"])));
    }
}
