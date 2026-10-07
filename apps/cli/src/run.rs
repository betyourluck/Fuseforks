//! `check` / `ask` / `serve` の本体（Spec 64 D5 / D7 / D8）。
//!
//! 3 つとも**最初に同じ起動前検査**（[`fuseforks_host::preflight`]）を走らせる。検査は村を
//! 開かず、LLM にも MCP にも触れない — `ask` と `serve` は拒否が 1 件でもあればここで止まる。
//!
//! 閉じ方は `ask` と `serve` で共通（凍結 11）: 扉を閉じる → 飛行中のターンに打ち切り →
//! 起動した個体を止める → `Host` を落とす（= ロックを外す）。猶予 30 秒・超えたら払いの
//! 記録が欠けうることを 1 行・2 回目の Ctrl+C は猶予を待たない。

use std::io::{Read, Write};
use std::time::Duration;

use fuseforks_core::headless::HeadlessMode;
use fuseforks_core::model::AgentId;
use fuseforks_core::CoreError;
use fuseforks_host::{
    build_host, preflight, Host, HostBootOptions, HostPaths, PreflightReport, PreflightRequest,
};
use tokio::sync::broadcast::error::RecvError;

use crate::args::{AskArgs, CheckArgs, Command, Common, MessageSource, ServeArgs};
use crate::exit;
use crate::output::{check_report, Level, Output};

/// 閉じるときの猶予（凍結 11）。
const CLOSE_GRACE: Duration = Duration::from_secs(30);

/// 版番号（`build.rs`。D11）。
pub const VERSION: &str = env!("FUSEFORKS_CLI_VERSION");

/// 命令を実行して終了コードを返す。`Help` / `Version` は呼び出し側（`main`）が先に処理する。
pub async fn dispatch(command: Command) -> u8 {
    match command {
        Command::Check(args) => check(args).await,
        Command::Ask(args) => ask(args).await,
        Command::Serve(args) => serve(args).await,
        Command::Bake(args) => bake(args).await,
        Command::Help | Command::Version => exit::OK,
    }
}

/// `bake` — GUI の村から写しを作る（Spec 65 D2）。村を組み立てない（LLM も MCP も呼ばない）。
/// 結果は標準出力（`--json` なら [`fuseforks_host::bake::BakeReport`] の形）、失敗は標準エラーに 1 行。
async fn bake(args: crate::args::BakeArgs) -> u8 {
    use fuseforks_host::bake::{bake, BakeRequest};
    let out = Output { jsonl: false };
    let request = BakeRequest {
        source: args.data_dir,
        out: args.out,
        maps: args.maps,
        update: args.update,
        source_time_zone: args.source_time_zone,
        allow_plaintext_headers: args.allow_plaintext_headers,
        app_version: VERSION.to_owned(),
        env_out: args.env_out,
    };
    // 資格情報ストアを読むのは --env-out のときだけ（Spec 66）。GUI と同じサービス名。
    let secrets = fuseforks_core::KeyringSecretStore::new();
    let report = match bake(&request, &secrets).await {
        Ok(report) => report,
        Err(err) => {
            out.cli(Level::Error, "BAKE", &err.to_string());
            return exit::for_bake_error(&err);
        }
    };
    if args.json {
        println!("{}", serde_json::to_string(&report).unwrap_or_default());
    } else {
        println!(
            "{}写しを作りました: {} ファイル・置き換えたパス {}・運んだ承認 {}・時刻帯 {}",
            if report.update { "（作り直し）" } else { "" },
            report.written.len(),
            report.mapped_fields,
            report.carried_approvals,
            report.source_time_zone
        );
        for removed in &report.removed {
            println!("  消した: {removed}");
        }
        for id in &report.seeded_memories {
            println!("  Memory を写した: {id}");
        }
        if let Some(env) = &report.env {
            println!(
                ".env を{}: 値を書いた鍵 {}・ストアに無い {}・書けない {}{}",
                if report.update { "更新しました" } else { "書きました" },
                env.written.len(),
                env.missing.len(),
                env.unwritable.len(),
                if env.door_token_created { "・扉の合鍵を新しく作った" } else { "" }
            );
        }
        for warning in &report.warnings {
            println!("警告 {}: {}", warning.code, warning.message);
        }
    }
    let _ = std::io::stdout().flush();
    exit::OK
}

fn request(common: &Common, mode: HeadlessMode) -> PreflightRequest {
    PreflightRequest {
        mode,
        start: common.start.clone(),
        secrets: common.secrets,
        bypass_plan_review: common.bypass_plan_review,
        run_approval: common.run_approval,
        door_port: common.door_port,
    }
}

/// 検査を走らせる。材料が揃わなければ理由を 1 行出して `Err(終了コード)`。
async fn run_preflight(
    out: Output,
    paths: &HostPaths,
    common: &Common,
    mode: HeadlessMode,
) -> Result<PreflightReport, u8> {
    preflight(paths, &request(common, mode)).await.map_err(|err| {
        let code = exit::for_preflight_error(&err);
        let id = if code == exit::USAGE { "USAGE" } else { "PREFLIGHT" };
        out.cli(Level::Error, id, &err.to_string());
        code
    })
}

/// `check` — 検査だけ。村を開かない。拒否が 1 件でもあれば 3。
async fn check(args: CheckArgs) -> u8 {
    // `check` は `--events` を持たない — 標準エラーは素の行のまま。
    let out = Output { jsonl: false };
    let paths = HostPaths::new(&args.common.data_dir);
    let report = match run_preflight(out, &paths, &args.common, args.mode).await {
        Ok(report) => report,
        Err(code) => return code,
    };
    let start: Vec<String> = report.start.iter().map(ToString::to_string).collect();
    print!("{}", check_report(args.json, &report.findings, &start));
    if args.json {
        println!();
    }
    let _ = std::io::stdout().flush();
    if report.rejected() { exit::REJECTED } else { exit::OK }
}

/// 組み立てる。失敗は理由を 1 行出して `Err(終了コード)`。
async fn open(
    out: Output,
    paths: &HostPaths,
    common: &Common,
    run_schedules: bool,
    open_door: bool,
) -> Result<Host, u8> {
    let opts = HostBootOptions {
        app_version: VERSION.to_owned(),
        secrets: common.secrets,
        run_schedules,
        open_door,
        door_port: common.door_port,
    };
    let host = build_host(paths, opts).await.map_err(|err| {
        let code = exit::for_host_error(&err);
        let id = if code == exit::LOCKED { "LOCKED" } else { "BOOT" };
        out.cli(Level::Error, id, &err.to_string());
        code
    })?;
    host.orchestrator.set_plan_review_bypass(common.bypass_plan_review);
    host.orchestrator.set_run_approval(common.run_approval);
    Ok(host)
}

/// コアのエラーを 1 行（`code` と文面）。
fn core_error(out: Output, err: &CoreError) -> u8 {
    out.cli(Level::Error, err.code(), &err.to_string());
    exit::CORE
}

/// `--events jsonl` のときだけ CoreEvent を標準エラーへ流すタスク。
fn forward_events(out: Output, host: &Host) -> Option<tokio::task::JoinHandle<()>> {
    if !out.jsonl {
        return None;
    }
    let mut rx = host.orchestrator.subscribe();
    Some(tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => out.event(&event),
                Err(RecvError::Lagged(n)) => out.cli(
                    Level::Warn,
                    "EVENTS_LAGGED",
                    &format!("イベントを {n} 件取りこぼしました（読み手が遅れています）"),
                ),
                Err(RecvError::Closed) => break,
            }
        }
    }))
}

/// 起動する集合を立ち上げる。立ち上がった id を返す（閉じるときに止める対象）。
async fn start_agents(out: Output, host: &Host, ids: &[AgentId]) -> Result<Vec<AgentId>, u8> {
    let mut started = Vec::new();
    for id in ids {
        match host.orchestrator.start_agent(id).await {
            Ok(()) => started.push(id.clone()),
            Err(err) => {
                // 途中で落ちたら、それまでに立ち上げた個体は閉じ方で止める。
                close(out, host, &started, &mut Signals::new()).await;
                return Err(core_error(out, &err));
            }
        }
    }
    Ok(started)
}

/// 閉じ方（凍結 11）。猶予を超える・2 回目のシグナルが来たら待たずに返る。
async fn close(out: Output, host: &Host, started: &[AgentId], signals: &mut Signals) {
    let graceful = async {
        host.mcp_server.lock().await.stop();
        host.orchestrator.interrupt_all().await;
        for id in started {
            // 既に落ちている個体（失敗した個体）は NotRunning — 止める対象が無いだけ。
            let _ = host.orchestrator.stop_agent(id).await;
        }
    };
    tokio::select! {
        () = graceful => {}
        () = tokio::time::sleep(CLOSE_GRACE) => out.cli(
            Level::Warn,
            "CLOSE_TIMEOUT",
            "30 秒待っても飛行中のターンが終わりませんでした。待たずに閉じます（このターンの払いの記録が欠けることがあります）",
        ),
        () = signals.next() => out.cli(
            Level::Warn,
            "CLOSE_FORCED",
            "2 回目の中断で、待たずに閉じます（飛行中のターンの払いの記録が欠けることがあります）",
        ),
    }
}

/// 転送タスクに残りのイベントを書き切る時間を少しだけ与えてから止める。
async fn finish_events(task: Option<tokio::task::JoinHandle<()>>) {
    if let Some(task) = task {
        tokio::time::sleep(Duration::from_millis(200)).await;
        task.abort();
    }
}

/// 依頼文を読む（`-` なら標準入力を全部）。空なら引数の誤り。
fn read_message(source: &MessageSource) -> Result<String, String> {
    let text = match source {
        MessageSource::Text(text) => text.clone(),
        MessageSource::Stdin => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|err| format!("標準入力を読めませんでした: {err}"))?;
            buf
        }
    };
    if text.trim().is_empty() {
        return Err("依頼文が空です".to_owned());
    }
    Ok(text)
}

/// `ask` — 1 通送って、答えを標準出力へ出して、閉じる（D7）。
async fn ask(args: AskArgs) -> u8 {
    let out = Output {
        jsonl: args.events_jsonl,
    };
    // 受け口は**最初に**作る（P6 の穴）。作るまでの Ctrl+C は既定の処理でプロセスごと落ち、
    // 8 にも閉じ方にもならない — MCP の起動を待つ 20 秒余りがまるごと素通しだった。
    let mut signals = Signals::new();
    let message = match read_message(&args.message) {
        Ok(message) => message,
        Err(reason) => {
            out.cli(Level::Error, "USAGE", &reason);
            return exit::USAGE;
        }
    };
    let paths = HostPaths::new(&args.common.data_dir);
    let report = match run_preflight(out, &paths, &args.common, HeadlessMode::Ask).await {
        Ok(report) => report,
        Err(code) => return code,
    };
    out.findings(&report.findings, args.verbose);
    if report.rejected() {
        return exit::REJECTED;
    }

    let host = tokio::select! {
        opened = open(out, &paths, &args.common, false, false) => match opened {
            Ok(host) => host,
            Err(code) => return code,
        },
        () = signals.next() => {
            // 組み立ての途中で落とす — 握ったロックは future と一緒に外れる。
            out.cli(Level::Info, "INTERRUPTED", "組み立ての途中で中断しました（何も送っていません）");
            return exit::INTERRUPTED;
        }
    };
    let events = forward_events(out, &host);
    let code = ask_in(out, &host, &report, &args, &message, &mut signals).await;
    drop(host);
    finish_events(events).await;
    code
}

async fn ask_in(
    out: Output,
    host: &Host,
    report: &PreflightReport,
    args: &AskArgs,
    message: &str,
    signals: &mut Signals,
) -> u8 {
    let prepare = async {
        if !args.continue_session
            && let Err(err) = host.orchestrator.reset_conversation().await
        {
            return Err(core_error(out, &err));
        }
        start_agents(out, host, &report.start).await
    };
    let started = tokio::select! {
        prepared = prepare => match prepared {
            Ok(started) => started,
            Err(code) => return code,
        },
        () = signals.next() => {
            // 起動の途中 — どこまで立ち上がったか分からないので、集合の全員に止めを送る
            // （立ち上がっていない個体は NotRunning で返るだけ）。
            out.cli(Level::Info, "INTERRUPTED", "起動の途中で中断しました（何も送っていません）");
            close(out, host, &report.start, signals).await;
            return exit::INTERRUPTED;
        }
    };

    let asked = host.orchestrator.ask_external_outcome(&args.client, message);
    tokio::pin!(asked);
    let mut interrupted = false;
    let result = loop {
        tokio::select! {
            result = &mut asked => break result,
            () = signals.next() => {
                if interrupted {
                    // 2 回目は猶予を待たない（凍結 11）。
                    out.cli(Level::Warn, "CLOSE_FORCED", "2 回目の中断で、待たずに閉じます");
                    return exit::INTERRUPTED;
                }
                interrupted = true;
                out.cli(Level::Info, "INTERRUPTING", "中断します（もう一度押すと待たずに閉じます）");
                host.orchestrator.interrupt_all().await;
            }
        }
    };

    let code = match result {
        Ok((answer, state)) => {
            // 6〜9 でも定型文は標準出力へ（凍結 10 — 本文は人が読む結末）。
            println!("{answer}");
            let _ = std::io::stdout().flush();
            exit::for_outcome(state)
        }
        Err(err) => core_error(out, &err),
    };
    close(out, host, &started, signals).await;
    code
}

/// `serve` — 常駐して予定と扉を回す（D8）。シグナルで閉じて 0。
async fn serve(args: ServeArgs) -> u8 {
    let out = Output {
        jsonl: args.events_jsonl,
    };
    let mut signals = Signals::new();
    let paths = HostPaths::new(&args.common.data_dir);
    let report = match run_preflight(out, &paths, &args.common, HeadlessMode::Serve).await {
        Ok(report) => report,
        Err(code) => return code,
    };
    out.findings(&report.findings, true);
    if report.rejected() {
        return exit::REJECTED;
    }

    let host = tokio::select! {
        opened = open(out, &paths, &args.common, true, true) => match opened {
            Ok(host) => host,
            Err(code) => return code,
        },
        // serve のシグナルは正常な閉じ方（0）。
        () = signals.next() => return exit::OK,
    };
    let events = forward_events(out, &host);
    let code = serve_in(out, &host, &report, &mut signals).await;
    drop(host);
    finish_events(events).await;
    code
}

async fn serve_in(
    out: Output,
    host: &Host,
    report: &PreflightReport,
    signals: &mut Signals,
) -> u8 {
    let door_error = host
        .mcp_server_error
        .lock()
        .ok()
        .and_then(|guard| guard.clone());
    if let Some(reason) = door_error {
        out.cli(Level::Warn, "DOOR_NOT_OPEN", &reason);
    }
    let started = tokio::select! {
        started = start_agents(out, host, &report.start) => match started {
            Ok(started) => started,
            Err(code) => return code,
        },
        () = signals.next() => {
            close(out, host, &report.start, signals).await;
            return exit::OK;
        }
    };
    out.cli(
        Level::Info,
        "SERVING",
        &format!(
            "{} 体を起動しました。Ctrl+C で閉じます",
            started.len()
        ),
    );
    signals.next().await;
    out.cli(Level::Info, "CLOSING", "閉じます（もう一度押すと待たずに閉じます）");
    close(out, host, &started, signals).await;
    exit::OK
}

/// Ctrl+C（SIGINT）と、Unix では SIGTERM。コンテナの停止は SIGTERM で来る。
///
/// **作った時点で受け口を登録する**（`tokio::signal::ctrl_c()` は async fn で、poll される
/// まで何も登録しない — 待っていない間の Ctrl+C は既定の処理でプロセスごと落ちる）。
/// 登録した後に来たシグナルは、次の [`Signals::next`] で受け取れる。
struct Signals {
    #[cfg(windows)]
    ctrl_c: Option<tokio::signal::windows::CtrlC>,
    #[cfg(unix)]
    interrupt: Option<tokio::signal::unix::Signal>,
    #[cfg(unix)]
    terminate: Option<tokio::signal::unix::Signal>,
}

impl Signals {
    fn new() -> Self {
        #[cfg(windows)]
        {
            Self {
                ctrl_c: tokio::signal::windows::ctrl_c().ok(),
            }
        }
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            Self {
                interrupt: signal(SignalKind::interrupt()).ok(),
                terminate: signal(SignalKind::terminate()).ok(),
            }
        }
    }

    /// 次のシグナルを待つ。受け口を 1 つも登録できなかった環境では待ち続ける（シグナル以外で
    /// 閉じる手段はプロセスを殺すことだけ — ロックは OS が外す）。
    async fn next(&mut self) {
        #[cfg(windows)]
        {
            match self.ctrl_c.as_mut() {
                Some(ctrl_c) => {
                    ctrl_c.recv().await;
                }
                None => std::future::pending::<()>().await,
            }
        }
        #[cfg(unix)]
        {
            async fn recv(signal: Option<&mut tokio::signal::unix::Signal>) {
                match signal {
                    Some(signal) => {
                        signal.recv().await;
                    }
                    None => std::future::pending::<()>().await,
                }
            }
            tokio::select! {
                () = recv(self.interrupt.as_mut()) => {}
                () = recv(self.terminate.as_mut()) => {}
            }
        }
    }
}
