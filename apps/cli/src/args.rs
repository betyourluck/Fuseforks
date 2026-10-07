//! 引数の解析（Spec 64 D5 / D7 / D8）。
//!
//! **`--data-dir` と `--start` は必須で、既定値を持たない**（D2 / D6）— 既定で GUI と
//! 同じ村を開くと開発機で意図せず GUI の村をヘッドレスで触る形が一番起きやすく、
//! `--start` に既定値があると `ask` と `serve` を打ち間違えたときに一括起動の全員が
//! 動き出す。引数で書いたものがそのまま開く村・起動する集合になる。
//!
//! 依存を足さずに手で解析する（`--flag value` と `--flag=value` の 2 形・`--` の後は
//! 依頼文）。誤りは終了コード 2 で、標準エラーに理由と使い方を出す。

use std::path::PathBuf;

use fuseforks_core::command::RunApproval;
use fuseforks_core::headless::HeadlessMode;
use fuseforks_host::{SecretSource, StartSpec};

/// 使い方（`--help` と引数の誤りのときに出す）。
pub const USAGE: &str = "\
fuseforks-cli — Fuseforks を GUI なしで動かす（Spec 64）

使い方:
  fuseforks-cli check --for ask|serve --data-dir <dir> --start <集合> [--secrets keyring|env]
                      [--bypass-plan-review] [--run-approval <mode>] [--door-port <N>] [--json]
  fuseforks-cli ask   --data-dir <dir> --start <集合> [--secrets keyring|env] [--continue-session]
                      [--client <名前>] [--bypass-plan-review] [--run-approval <mode>]
                      [--events jsonl] [--verbose] <依頼文 | - で標準入力>
  fuseforks-cli serve --data-dir <dir> --start <集合> [--secrets keyring|env]
                      [--bypass-plan-review] [--run-approval <mode>] [--door-port <N>]
                      [--events jsonl]
  fuseforks-cli bake  --data-dir <GUI の dir> --out <写しの dir> --map <元>=<先> [--map …]
                      [--update] [--source-time-zone <IANA 名>] [--allow-plaintext-headers] [--json]
  fuseforks-cli --version | --help

  <集合>   batch（GUI の全体 ▶ と同じ対象）| reception（窓口だけ。ask 専用）| <id>,<id>,…
           ask はどの値でも窓口を足す。既定値は無い
  <mode>   required（既定）| auto-approve | no-approval
  <N>      扉のポート（1〜65535）。mcp_server.json を読まずに 127.0.0.1:N で開き、合鍵は秘密の
           door_token（env なら FUSEFORKS_SECRET_DOOR_TOKEN）。書かなければ mcp_server.json のとおり
  bake     GUI の村から、コンテナで回す写しを作る（Spec 65）。GUI を閉じてから GUI の端末で動かす。
           --map は作業フォルダ・rag の宣言・予定の cwd の絶対パスを置き換える（最長前方一致）。
           置き換えなかったパスが 1 つでもあれば写しを作らない。--update は写しを作り直す
           （会話・Memory・予定の消化・承認待ちは写し先のものを残す。--map を省けば前回のもの）
";

/// 解析した命令。
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// `--help`。
    Help,
    /// `--version`。
    Version,
    /// 起動前検査だけを走らせる。村を開かない。
    Check(CheckArgs),
    /// 1 通送って、答えを出して、閉じる。
    Ask(AskArgs),
    /// 常駐して、予定と扉を回す。
    Serve(ServeArgs),
    /// GUI の村から、コンテナで回す写しを作る（Spec 65）。
    Bake(BakeArgs),
}

/// `bake` の引数（Spec 65 D2）。`--start` も `--secrets` も取らない — 村を組み立てない。
#[derive(Debug, Clone, PartialEq)]
pub struct BakeArgs {
    /// `--data-dir`（必須）— GUI の data_dir（元の村）。
    pub data_dir: PathBuf,
    /// `--out`（必須）— 写しの data_dir。
    pub out: PathBuf,
    /// `--map <元>=<先>`（何度でも）。`--update` なら省ける。
    pub maps: Vec<fuseforks_host::bake::PathMap>,
    /// `--update`。
    pub update: bool,
    /// `--source-time-zone`。
    pub source_time_zone: Option<String>,
    /// `--allow-plaintext-headers`。
    pub allow_plaintext_headers: bool,
    /// `--json`。
    pub json: bool,
}

/// 3 つの命令に共通の引数。
#[derive(Debug, Clone, PartialEq)]
pub struct Common {
    /// `--data-dir`（必須）。GUI の `app_data_dir` に当たる場所で、村は `<dir>/workspace`。
    pub data_dir: PathBuf,
    /// `--start`（必須）。
    pub start: StartSpec,
    /// `--secrets`（既定 keyring）。
    pub secrets: SecretSource,
    /// `--bypass-plan-review`（Spec 53 のスイッチを立てる）。
    pub bypass_plan_review: bool,
    /// `--run-approval`（既定 required = コアの既定）。
    pub run_approval: RunApproval,
    /// `--door-port`（Spec 65 D9。`check` と `serve` だけ。`ask` は扉を開かない）。
    pub door_port: Option<u16>,
}

/// `check` の引数。
#[derive(Debug, Clone, PartialEq)]
pub struct CheckArgs {
    /// 共通。
    pub common: Common,
    /// `--for`（必須）— 検査したいコマンド。
    pub mode: HeadlessMode,
    /// `--json`。
    pub json: bool,
}

/// 依頼文の出どころ。
#[derive(Debug, Clone, PartialEq)]
pub enum MessageSource {
    /// 引数に書いた文。
    Text(String),
    /// `-` — 標準入力を全部読む。
    Stdin,
}

/// `ask` の引数。
#[derive(Debug, Clone, PartialEq)]
pub struct AskArgs {
    /// 共通。
    pub common: Common,
    /// `--continue-session` — 新しい会話を作らず、今の会話へ続ける。
    pub continue_session: bool,
    /// `--client`（既定 `fuseforks-cli`）— 外部クライアントとしての名乗り。
    pub client: String,
    /// `--events jsonl`。
    pub events_jsonl: bool,
    /// `--verbose` — 情報の指摘も出す。
    pub verbose: bool,
    /// 依頼文。
    pub message: MessageSource,
}

/// `serve` の引数。
#[derive(Debug, Clone, PartialEq)]
pub struct ServeArgs {
    /// 共通。
    pub common: Common,
    /// `--events jsonl`。
    pub events_jsonl: bool,
}

/// 既定の名乗り（`--client` を省いたとき）。村の `externalName` が設定されていればそれが勝つ。
pub const DEFAULT_CLIENT: &str = "fuseforks-cli";

/// 引数を解析する。`args` はプログラム名を除いたもの。
///
/// # Errors
/// 理由の 1 文（使い方は呼び出し側が添える）。
pub fn parse(args: &[String]) -> Result<Command, String> {
    let Some((first, rest)) = args.split_first() else {
        return Err("命令がありません（check / ask / serve）".to_owned());
    };
    match first.as_str() {
        "--help" | "-h" | "help" => Ok(Command::Help),
        "--version" | "-V" => Ok(Command::Version),
        "check" => parse_command(Kind::Check, rest),
        "ask" => parse_command(Kind::Ask, rest),
        "serve" => parse_command(Kind::Serve, rest),
        "bake" => parse_bake(rest),
        other => Err(format!("知らない命令です: {other}（check / ask / serve / bake）")),
    }
}

/// `bake` の引数（他の 3 命令と形が違う — `--map` を何度も書き、`--start` を取らない）。
fn parse_bake(args: &[String]) -> Result<Command, String> {
    let mut data_dir = None;
    let mut out = None;
    let mut maps = Vec::new();
    let mut source_time_zone = None;
    let mut switches: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_owned())),
            _ => (arg.as_str(), None),
        };
        match flag {
            "--data-dir" | "--out" | "--map" | "--source-time-zone" => {
                let value = match inline {
                    Some(value) => value,
                    None => {
                        let Some(value) = args.get(i) else {
                            return Err(format!("{flag} に値がありません"));
                        };
                        i += 1;
                        value.clone()
                    }
                };
                let once = |slot: &mut Option<String>| {
                    if slot.replace(value.clone()).is_some() {
                        Err(format!("{flag} が 2 回あります"))
                    } else {
                        Ok(())
                    }
                };
                match flag {
                    "--data-dir" => once(&mut data_dir)?,
                    "--out" => once(&mut out)?,
                    "--source-time-zone" => once(&mut source_time_zone)?,
                    _ => {
                        let Some((from, to)) = value.split_once('=') else {
                            return Err(format!("--map は <元>=<先> の形です: {value}"));
                        };
                        maps.push(fuseforks_host::bake::PathMap {
                            from: from.trim().to_owned(),
                            to: to.trim().to_owned(),
                        });
                    }
                }
            }
            "--update" | "--allow-plaintext-headers" | "--json" => {
                if inline.is_some() {
                    return Err(format!("{flag} は値を取りません"));
                }
                if switches.contains(&flag) {
                    return Err(format!("{flag} が 2 回あります"));
                }
                switches.push(flag);
            }
            other => return Err(format!("bake では使えない引数です: {other}")),
        }
    }
    let data_dir = data_dir.ok_or("--data-dir がありません（GUI の data_dir。既定値は無い）")?;
    let out = out.ok_or("--out がありません（写しの data_dir）")?;
    let update = switches.contains(&"--update");
    if maps.is_empty() && !update {
        return Err("--map がありません（<元>=<先> を 1 つ以上。--update なら前回のものを引き継ぎます）".to_owned());
    }
    Ok(Command::Bake(BakeArgs {
        data_dir: PathBuf::from(data_dir),
        out: PathBuf::from(out),
        maps,
        update,
        source_time_zone: source_time_zone.filter(|tz| !tz.trim().is_empty()),
        allow_plaintext_headers: switches.contains(&"--allow-plaintext-headers"),
        json: switches.contains(&"--json"),
    }))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Check,
    Ask,
    Serve,
}

/// 値を取る旗と、取らない旗。命令ごとに使えるものを閉じて持つ（知らない旗は誤り）。
fn takes_value(kind: Kind, flag: &str) -> Option<bool> {
    let common_value = matches!(flag, "--data-dir" | "--start" | "--secrets" | "--run-approval");
    let common_switch = flag == "--bypass-plan-review";
    let (value, switch) = match kind {
        Kind::Check => (matches!(flag, "--for" | "--door-port"), flag == "--json"),
        Kind::Ask => (
            matches!(flag, "--client" | "--events"),
            matches!(flag, "--continue-session" | "--verbose"),
        ),
        Kind::Serve => (matches!(flag, "--events" | "--door-port"), false),
    };
    if common_value || value {
        Some(true)
    } else if common_switch || switch {
        Some(false)
    } else {
        None
    }
}

fn parse_command(kind: Kind, args: &[String]) -> Result<Command, String> {
    let mut values: Vec<(String, String)> = Vec::new();
    let mut switches: Vec<String> = Vec::new();
    let mut positional: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        if arg == "--" {
            positional.extend(args[i..].iter().cloned());
            break;
        }
        // `-` は「標準入力」の位置引数。`--x` で始まるものだけを旗として読む。
        if !arg.starts_with("--") {
            positional.push(arg.clone());
            continue;
        }
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) => (flag.to_owned(), Some(value.to_owned())),
            None => (arg.clone(), None),
        };
        match takes_value(kind, &flag) {
            None => return Err(format!("この命令では使えない旗です: {flag}")),
            Some(true) => {
                let value = match inline {
                    Some(value) => value,
                    None => {
                        let Some(value) = args.get(i) else {
                            return Err(format!("{flag} に値がありません"));
                        };
                        i += 1;
                        value.clone()
                    }
                };
                if values.iter().any(|(f, _)| f == &flag) {
                    return Err(format!("{flag} が 2 回あります"));
                }
                values.push((flag, value));
            }
            Some(false) => {
                if inline.is_some() {
                    return Err(format!("{flag} は値を取りません"));
                }
                if switches.contains(&flag) {
                    return Err(format!("{flag} が 2 回あります"));
                }
                switches.push(flag);
            }
        }
    }

    let value = |flag: &str| {
        values
            .iter()
            .find(|(f, _)| f == flag)
            .map(|(_, v)| v.as_str())
    };
    let switch = |flag: &str| switches.iter().any(|s| s == flag);

    let data_dir = value("--data-dir")
        .ok_or("--data-dir がありません（既定値は無い — GUI の村を意図せず開かないため）")?;
    if data_dir.trim().is_empty() {
        return Err("--data-dir が空です".to_owned());
    }
    let start = parse_start(
        value("--start").ok_or("--start がありません（batch / reception / <id>,<id>,…。既定値は無い）")?,
    )?;
    let secrets = match value("--secrets") {
        None | Some("keyring") => SecretSource::Keyring,
        Some("env") => SecretSource::Env,
        Some(other) => return Err(format!("--secrets は keyring か env です: {other}")),
    };
    let run_approval = match value("--run-approval") {
        None | Some("required") => RunApproval::Required,
        Some("auto-approve") => RunApproval::AutoApprove,
        Some("no-approval") => RunApproval::NoApproval,
        Some(other) => {
            return Err(format!(
                "--run-approval は required / auto-approve / no-approval のどれかです: {other}"
            ));
        }
    };
    // ポート 0 は「OS が空きを選ぶ」の意味で、プロキシが向ける先が決まらないので受けない。
    let door_port = match value("--door-port") {
        None => None,
        Some(raw) => match raw.trim().parse::<u16>() {
            Ok(port) if port != 0 => Some(port),
            _ => return Err(format!("--door-port は 1〜65535 のポート番号です: {raw}")),
        },
    };
    let common = Common {
        data_dir: PathBuf::from(data_dir),
        start,
        secrets,
        bypass_plan_review: switch("--bypass-plan-review"),
        run_approval,
        door_port,
    };
    let events_jsonl = match value("--events") {
        None => false,
        Some("jsonl") => true,
        Some(other) => return Err(format!("--events は jsonl だけです: {other}")),
    };

    match kind {
        Kind::Check => {
            if !positional.is_empty() {
                return Err(format!("check は位置引数を取りません: {}", positional.join(" ")));
            }
            let mode = match value("--for") {
                Some("ask") => HeadlessMode::Ask,
                Some("serve") => HeadlessMode::Serve,
                Some(other) => return Err(format!("--for は ask か serve です: {other}")),
                None => return Err("--for がありません（ask / serve）".to_owned()),
            };
            Ok(Command::Check(CheckArgs {
                common,
                mode,
                json: switch("--json"),
            }))
        }
        Kind::Ask => {
            let message = match positional.as_slice() {
                [] => return Err("依頼文がありません（文を書くか、- で標準入力）".to_owned()),
                [one] if one == "-" => MessageSource::Stdin,
                [one] => MessageSource::Text(one.clone()),
                _ => {
                    return Err(
                        "依頼文は 1 つだけです（空白を含むなら引用符で囲むか、- で標準入力）"
                            .to_owned(),
                    );
                }
            };
            let client = value("--client").unwrap_or(DEFAULT_CLIENT).to_owned();
            if client.trim().is_empty() {
                return Err("--client が空です".to_owned());
            }
            Ok(Command::Ask(AskArgs {
                common,
                continue_session: switch("--continue-session"),
                client,
                events_jsonl,
                verbose: switch("--verbose"),
                message,
            }))
        }
        Kind::Serve => {
            if !positional.is_empty() {
                return Err(format!("serve は位置引数を取りません: {}", positional.join(" ")));
            }
            Ok(Command::Serve(ServeArgs {
                common,
                events_jsonl,
            }))
        }
    }
}

/// `--start` の値（D6）。`none` は持たない — `serve` で誰も起動しないと空転する。
fn parse_start(raw: &str) -> Result<StartSpec, String> {
    match raw.trim() {
        "batch" => Ok(StartSpec::Batch),
        "reception" => Ok(StartSpec::Reception),
        "" => Err("--start が空です".to_owned()),
        list => {
            let ids: Vec<String> = list
                .split(',')
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
                .collect();
            if ids.is_empty() {
                return Err(format!("--start に id がありません: {raw}"));
            }
            Ok(StartSpec::Ids(ids))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(line: &[&str]) -> Vec<String> {
        line.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn ask_takes_the_required_pair_and_one_message() {
        let cmd = parse(&argv(&[
            "ask", "--data-dir", "D", "--start=reception", "--secrets", "env", "やあ",
        ]))
        .unwrap();
        let Command::Ask(ask) = cmd else { panic!("{cmd:?}") };
        assert_eq!(ask.common.data_dir, PathBuf::from("D"));
        assert_eq!(ask.common.start, StartSpec::Reception);
        assert_eq!(ask.common.secrets, SecretSource::Env);
        assert_eq!(ask.common.run_approval, RunApproval::Required);
        assert_eq!(ask.client, DEFAULT_CLIENT);
        assert_eq!(ask.message, MessageSource::Text("やあ".into()));
    }

    /// `--start` と `--data-dir` は既定値を持たない（D2 / D6）。
    #[test]
    fn start_and_data_dir_are_required() {
        let no_start = parse(&argv(&["ask", "--data-dir", "D", "やあ"])).unwrap_err();
        assert!(no_start.contains("--start"), "{no_start}");
        let no_dir = parse(&argv(&["serve", "--start", "batch"])).unwrap_err();
        assert!(no_dir.contains("--data-dir"), "{no_dir}");
    }

    #[test]
    fn start_lists_ids_and_rejects_empty() {
        assert_eq!(
            parse_start("agent_1, agent_2,,").unwrap(),
            StartSpec::Ids(vec!["agent_1".into(), "agent_2".into()])
        );
        assert_eq!(parse_start("batch").unwrap(), StartSpec::Batch);
        assert!(parse_start(",").is_err());
        assert!(parse_start(" ").is_err());
    }

    /// 旗は命令ごとに閉じている（`serve` に `--client` は無い・`check` に依頼文は無い）。
    #[test]
    fn flags_are_closed_per_command() {
        let base = ["--data-dir", "D", "--start", "batch"];
        let serve_client = [&["serve"][..], &base, &["--client", "x"]].concat();
        assert!(parse(&argv(&serve_client)).unwrap_err().contains("--client"));
        let check_message = [&["check", "--for", "ask"][..], &base, &["やあ"]].concat();
        assert!(parse(&argv(&check_message)).is_err());
        let typo = [&["ask"][..], &base, &["--verbos", "やあ"]].concat();
        assert!(parse(&argv(&typo)).unwrap_err().contains("--verbos"));
    }

    #[test]
    fn dash_reads_stdin_and_double_dash_ends_flags() {
        let base = ["ask", "--data-dir", "D", "--start", "reception"];
        let Command::Ask(stdin) = parse(&argv(&[&base[..], &["-"]].concat())).unwrap() else {
            panic!()
        };
        assert_eq!(stdin.message, MessageSource::Stdin);
        let Command::Ask(dashed) =
            parse(&argv(&[&base[..], &["--", "--start は何？"]].concat())).unwrap()
        else {
            panic!()
        };
        assert_eq!(dashed.message, MessageSource::Text("--start は何？".into()));
    }

    #[test]
    fn values_are_closed_sets() {
        let base = ["serve", "--data-dir", "D", "--start", "batch"];
        for (flag, bad) in [
            ("--secrets", "vault"),
            ("--run-approval", "always"),
            ("--events", "json"),
        ] {
            let err = parse(&argv(&[&base[..], &[flag, bad]].concat())).unwrap_err();
            assert!(err.contains(flag), "{err}");
        }
        let Command::Serve(serve) = parse(&argv(
            &[&base[..], &["--run-approval", "no-approval", "--events", "jsonl"]].concat(),
        ))
        .unwrap() else {
            panic!()
        };
        assert_eq!(serve.common.run_approval, RunApproval::NoApproval);
        assert!(serve.events_jsonl);
    }

    /// `--door-port` は check と serve だけ。1〜65535 で、0 と数でない値は誤り。
    #[test]
    fn door_port_is_for_check_and_serve_only() {
        let base = ["--data-dir", "D", "--start", "batch"];
        let serve = [&["serve"][..], &base, &["--door-port", "39641"]].concat();
        let Command::Serve(serve) = parse(&argv(&serve)).unwrap() else { panic!() };
        assert_eq!(serve.common.door_port, Some(39641));
        let check = [&["check", "--for", "serve"][..], &base, &["--door-port=39641"]].concat();
        let Command::Check(check) = parse(&argv(&check)).unwrap() else { panic!() };
        assert_eq!(check.common.door_port, Some(39641));
        let ask = [&["ask"][..], &base, &["--door-port", "39641", "やあ"]].concat();
        assert!(parse(&argv(&ask)).unwrap_err().contains("--door-port"));
        for bad in ["0", "70000", "x"] {
            let serve = [&["serve"][..], &base, &["--door-port", bad]].concat();
            assert!(parse(&argv(&serve)).unwrap_err().contains("--door-port"), "{bad}");
        }
    }

    /// `bake` は `--map` を何度も書け、`--start` を取らない。`--update` なら `--map` を省ける。
    #[test]
    fn bake_takes_repeated_maps_and_no_start() {
        let cmd = parse(&argv(&[
            "bake", "--data-dir", "G", "--out=O", "--map", "D:\\a=/a", "--map=D:\\b = /b", "--json",
        ]))
        .unwrap();
        let Command::Bake(bake) = cmd else { panic!("{cmd:?}") };
        assert_eq!(bake.data_dir, PathBuf::from("G"));
        assert_eq!(bake.out, PathBuf::from("O"));
        assert_eq!(bake.maps.len(), 2);
        assert_eq!(bake.maps[1].from, "D:\\b");
        assert_eq!(bake.maps[1].to, "/b");
        assert!(bake.json && !bake.update);

        let update = parse(&argv(&["bake", "--data-dir", "G", "--out", "O", "--update"])).unwrap();
        let Command::Bake(update) = update else { panic!() };
        assert!(update.update && update.maps.is_empty());

        for bad in [
            &["bake", "--data-dir", "G", "--out", "O"][..],
            &["bake", "--out", "O", "--map", "a=/a"],
            &["bake", "--data-dir", "G", "--out", "O", "--map", "no-equals"],
            &["bake", "--data-dir", "G", "--out", "O", "--map", "a=/a", "--start", "batch"],
        ] {
            assert!(parse(&argv(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn check_needs_for() {
        let err = parse(&argv(&["check", "--data-dir", "D", "--start", "batch"])).unwrap_err();
        assert!(err.contains("--for"), "{err}");
    }
}
