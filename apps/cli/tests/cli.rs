//! `fuseforks-cli` の結合テスト（Spec 64 P4）。
//!
//! 実行ファイルを**子プロセスとして**起こし、終了コード・標準出力・標準エラーを見る。
//! LLM の代わりにループバックの OpenAI 互換スタブを立てる（`attachment_fallback.rs` と
//! 同じ形。ただし子プロセスの実行中ずっと応答するので std のスレッドで持つ）。
//!
//! 村は**本物の保存の経路で作る** — テストの中で別のランタイムを起こして
//! `Orchestrator::bootstrap` → テンプレート・個体・窓口を登録 → ランタイムごと落とす
//! （`sessions.redb` を手放させる）。`world.json` を手で書くと、欄の名前を写し間違えた
//! ときに「CLI が読めない村」を作ってしまう。
//!
//! **秘密は `--secrets env` で渡す** — keyring だと組み立てが資格情報ストアを読みに行く
//! （P3 で同じ理由で keyring の結合テストを外した）。

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fuseforks_core::model::{AgentId, AgentSpec, CredentialSource, ModelTemplate};
use fuseforks_core::secret::SecretStore;
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
    Provider,
};

const BIN: &str = env!("CARGO_BIN_EXE_fuseforks-cli");
const ANSWER: &str = "スタブの答えです";
/// テンプレート ID `stub` の秘密の変数名（凍結 5 の写像）。
const SECRET_VAR: &str = "FUSEFORKS_SECRET_STUB";

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-cli-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn workspace(&self) -> PathBuf {
        self.0.join("workspace")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// スタブの振る舞い。
#[derive(Clone, Copy)]
enum Stub {
    /// 1 周で本文を返す。
    Answer,
    /// 毎周ツールを呼ぶ（本文を返さない）。予算の天井に当てるため。
    ToolEveryRound,
    /// 3 秒待ってから毎周ツールを呼ぶ（Ctrl+C を飛行中のターンへ当てるため）。
    /// **1 周で答えるスタブでは打ち切りにならない** — 打ち切りは周回の境目で効く（Spec 10）ので、
    /// 飛行中の 1 周で答え終わるターンは答えが返って 0 になる（P6 の実機で観測）。
    #[cfg_attr(not(unix), allow(dead_code))]
    SlowToolEveryRound,
}

/// ループバックに OpenAI 互換スタブを立て、`(base_url, 受けた要求の数)` を返す。
/// スレッドはテストのプロセスが終わるまで生きる。
fn spawn_stub(stub: Stub) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&hits);
    std::thread::spawn(move || {
        for (round, socket) in listener.incoming().enumerate() {
            let Ok(mut socket) = socket else { continue };
            counted.fetch_add(1, Ordering::SeqCst);
            std::thread::spawn(move || {
                // ヘッダと本文を読み切る（`content-length` を見る）。
                let mut raw = Vec::new();
                let mut buf = [0u8; 8192];
                loop {
                    let Ok(n) = socket.read(&mut buf) else { return };
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&raw);
                    let Some(head_end) = text.find("\r\n\r\n") else { continue };
                    let len: usize = text[..head_end]
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse().ok())?
                        })
                        .unwrap_or(0);
                    if raw.len() >= head_end + 4 + len {
                        break;
                    }
                }
                if matches!(stub, Stub::SlowToolEveryRound) {
                    std::thread::sleep(Duration::from_secs(3));
                }
                let payload = match stub {
                    Stub::Answer => serde_json::json!({
                        "choices": [{
                            "message": { "role": "assistant", "content": ANSWER },
                            "finish_reason": "stop",
                        }],
                        "usage": { "prompt_tokens": 50, "completion_tokens": 5 },
                    }),
                    // 呼び出しの id と引数を周ごとに変える（RepeatGuard に止めさせない —
                    // 止まるべきは予算の天井）。名前は提示外でもよい（L3 が結果を返して周が続く）。
                    Stub::ToolEveryRound | Stub::SlowToolEveryRound => serde_json::json!({
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": null,
                                "tool_calls": [{
                                    "id": format!("call_{round}"),
                                    "type": "function",
                                    "function": {
                                        "name": "nonexistent_tool",
                                        "arguments": format!("{{\"n\":{round}}}"),
                                    },
                                }],
                            },
                            "finish_reason": "tool_calls",
                        }],
                        "usage": { "prompt_tokens": 50, "completion_tokens": 5 },
                    }),
                }
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = socket.write_all(response.as_bytes());
                let _ = socket.flush();
            });
        }
    });
    (format!("http://127.0.0.1:{port}/v1"), hits)
}

/// 村を作る。窓口 `agent_1` がテンプレート `stub`（資格情報は keyring 形 = 変数名で読む）を使う。
fn make_village(dir: &TempDir, base_url: &str, budget: Option<u64>) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        // 村を作る側のストアにも鍵を置く — `upsert_template` は秘密の裏付けの無い
        // keyring 主張を「未登録」へ引き戻す（`failures.md` #16 の網）。CLI が読むのは
        // 環境変数のほうで、ここの値は使われない。
        let secrets = InMemorySecretStore::new();
        secrets.set("stub", "setup-only").unwrap();
        let orchestrator = Orchestrator::bootstrap(
            ConfigStore::new(dir.workspace()),
            Arc::new(FixedBackendFactory::echo("[echo]")),
            Arc::new(secrets),
            OrchestratorConfig::default(),
        )
        .await
        .expect("bootstrap できること");
        orchestrator
            .set_language(fuseforks_core::world::Language::Ja)
            .await
            .unwrap();
        let mut template = ModelTemplate::new("stub", "スタブ", "stub-model");
        template.base_url = base_url.to_owned();
        template.credential = CredentialSource::Keyring;
        template.provider = Some(Provider::OpenAiCompat);
        template.max_retries = 1;
        orchestrator.upsert_template(template).await.unwrap();
        let id = AgentId::from("agent_1");
        orchestrator
            .create_agent(AgentSpec::new(id.clone(), "窓口", "stub"))
            .await
            .unwrap();
        orchestrator.set_reception(Some(&id)).await.unwrap();
        orchestrator.set_token_budget(budget).await.unwrap();
    });
    // ランタイムごと落として、予定のティッカーが握る `sessions.redb` も手放させる。
    drop(runtime);
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

/// 実行ファイルを起こして終わりを待つ。**120 秒で殺す**（実装の待ちが壊れても
/// テストが永久に返らない形を作らない — `failures.md` #86）。
fn run(args: &[&str], secret: bool) -> Run {
    let mut command = Command::new(BIN);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if secret {
        command.env(SECRET_VAR, "dummy-key");
    } else {
        command.env_remove(SECRET_VAR);
    }
    let mut child = command.spawn().expect("実行ファイルを起こせること");
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out_reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let err_reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let deadline = Instant::now() + Duration::from_secs(120);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("120 秒で終わらなかった: {args:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    Run {
        code: status.code().unwrap_or(-1),
        stdout: out_reader.join().unwrap(),
        stderr: err_reader.join().unwrap(),
    }
}

fn data_dir(dir: &TempDir) -> &str {
    dir.0.to_str().unwrap()
}

/// 秘密の無いテンプレートを使う集合で `check --for ask` は拒否 = 3。村を開かない。
#[test]
fn check_rejects_a_template_without_its_secret() {
    let dir = TempDir::new("check");
    make_village(&dir, "http://127.0.0.1:9/v1", None);
    let run = run(
        &[
            "check", "--for", "ask", "--data-dir", data_dir(&dir), "--start", "reception",
            "--secrets", "env",
        ],
        false,
    );
    assert_eq!(run.code, 3, "stdout={} stderr={}", run.stdout, run.stderr);
    assert!(run.stdout.contains("SECRET_MISSING"), "{}", run.stdout);
}

/// 検査で拒否されたら `ask` は組み立ての前に止まる = 3。**LLM を 1 回も呼ばない**
/// （D5）— スタブが受けた要求が 0 件であることで読む。
#[test]
fn ask_stops_on_a_rejection_without_calling_the_llm() {
    let dir = TempDir::new("ask-reject");
    let (base_url, hits) = spawn_stub(Stub::Answer);
    make_village(&dir, &base_url, None);
    let run = run(
        &[
            "ask", "--data-dir", data_dir(&dir), "--start", "reception", "--secrets", "env",
            "こんにちは",
        ],
        false,
    );
    assert_eq!(run.code, 3, "stdout={} stderr={}", run.stdout, run.stderr);
    assert!(run.stderr.contains("SECRET_MISSING"), "{}", run.stderr);
    assert!(run.stdout.is_empty(), "答えは出ない: {}", run.stdout);
    assert_eq!(hits.load(Ordering::SeqCst), 0, "LLM へ 1 通も送っていない");
    // 組み立ての前に止まった — ロックファイルも作られていない。
    assert!(!dir.workspace().join(fuseforks_host::LOCK_FILE).exists());
}

/// スタブの答えが標準出力にそのまま出て 0。標準出力は答えだけ（改行 1 つ）。
#[test]
fn ask_prints_the_answer_and_exits_zero() {
    let dir = TempDir::new("ask");
    make_village(&dir, &spawn_stub(Stub::Answer).0, None);
    let run = run(
        &[
            "ask", "--data-dir", data_dir(&dir), "--start", "reception", "--secrets", "env",
            "こんにちは",
        ],
        true,
    );
    assert_eq!(run.code, 0, "stderr={}", run.stderr);
    assert_eq!(run.stdout, format!("{ANSWER}\n"));
}

/// 村のロックを別のプロセスが握っている → 4。検査は通る（ロックを取らない）ので、
/// 止まるのは組み立ての最初の手。
#[test]
fn ask_against_a_locked_village_exits_four() {
    let dir = TempDir::new("locked");
    make_village(&dir, &spawn_stub(Stub::Answer).0, None);
    let _held = fuseforks_host::VillageLock::acquire(&dir.workspace()).expect("ロックを取れること");
    let run = run(
        &[
            "ask", "--data-dir", data_dir(&dir), "--start", "reception", "--secrets", "env",
            "こんにちは",
        ],
        true,
    );
    assert_eq!(run.code, 4, "stderr={}", run.stderr);
    assert!(run.stdout.is_empty(), "答えは出ない: {}", run.stdout);
}

/// `--events jsonl` の間は標準エラーの全行が JSON（凍結 13）。標準出力は答えのまま。
#[test]
fn events_jsonl_makes_every_stderr_line_json() {
    let dir = TempDir::new("jsonl");
    make_village(&dir, &spawn_stub(Stub::Answer).0, None);
    let run = run(
        &[
            "ask", "--data-dir", data_dir(&dir), "--start", "reception", "--secrets", "env",
            "--events", "jsonl", "こんにちは",
        ],
        true,
    );
    assert_eq!(run.code, 0, "stderr={}", run.stderr);
    assert_eq!(run.stdout, format!("{ANSWER}\n"));
    let lines: Vec<&str> = run.stderr.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(!lines.is_empty(), "イベントが 1 行も出ていない");
    let mut types = std::collections::BTreeSet::new();
    for line in &lines {
        let value: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|err| panic!("JSON でない行: {line} ({err})"));
        let kind = value["type"].as_str().unwrap_or_else(|| panic!("type が無い: {line}"));
        types.insert(kind.to_owned());
    }
    // 診断の行（`note!` の `version:` 等）が JSON で混ざっていること = 素の行が 1 行も無い証拠。
    assert!(types.contains("log"), "{types:?}");
}

/// MCP の stdio サーバーの子が標準エラーへ素の行を出しても、`--events jsonl` の間は
/// 全行が JSON（凍結 13）。**P6 の実機で破れているのが見つかった穴** — rmcp の既定は子の
/// 標準エラーを継ぐので、Docker の MCP ゲートウェイや memoria の出力が素のまま流れていた。
/// 子には実行ファイル自身を誤った引数で起こす（標準エラーへ使い方を出して 2 で終わる。
/// OS を問わずある実行ファイルで、MCP としては接続に失敗するだけ）。
#[test]
fn events_jsonl_wraps_the_stderr_of_mcp_children() {
    let dir = TempDir::new("jsonl-mcp");
    make_village(&dir, &spawn_stub(Stub::Answer).0, None);
    let mcp = serde_json::json!({
        "mcpServers": { "noisy": { "command": BIN, "args": ["nonsense"] } }
    });
    std::fs::write(dir.workspace().join("mcp.json"), mcp.to_string()).unwrap();
    let run = run(
        &[
            "ask", "--data-dir", data_dir(&dir), "--start", "reception", "--secrets", "env",
            "--events", "jsonl", "こんにちは",
        ],
        true,
    );
    assert_eq!(run.code, 0, "stderr={}", run.stderr);
    let mut from_child = 0;
    for line in run.stderr.lines().filter(|l| !l.trim().is_empty()) {
        let value: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|err| panic!("JSON でない行: {line} ({err})"));
        if value["source"] == "mcp:noisy" {
            from_child += 1;
        }
    }
    assert!(from_child > 0, "子の行が 1 行も写っていない: {}", run.stderr);
}

/// 天井 5 で毎周ツールを呼ぶ → 2 周目の予約が通らず予算切れ = 9。
/// **1 周で答えるスタブでは 9 にならない**（予約の見積もりは min(床, 天井) で 1 周目が通る）。
#[test]
fn a_budget_stop_exits_nine() {
    let dir = TempDir::new("budget");
    make_village(&dir, &spawn_stub(Stub::ToolEveryRound).0, Some(5));
    let run = run(
        &[
            "ask", "--data-dir", data_dir(&dir), "--start", "reception", "--secrets", "env",
            "こんにちは",
        ],
        true,
    );
    assert_eq!(run.code, 9, "stdout={} stderr={}", run.stdout, run.stderr);
    assert!(!run.stdout.is_empty(), "定型文は標準出力へ書く（凍結 10）");
}

/// `ask` の途中の SIGINT（端末の Ctrl+C）は打ち切りになり 8。閉じ方を通るので、飛行中の
/// ターンの払いの記録（`turn:` 行）も残る（凍結 11・#103）。スタブは毎周ツールを呼ぶ —
/// 1 周目の飛行中に送り、2 周目の境目で打ち切りが効く。**Unix だけ** — Windows で子へ
/// Ctrl+C を送るにはコンソールに付く必要があり（unsafe が要る）、P6 の実機で確かめた。
#[cfg(unix)]
#[test]
fn sigint_during_ask_interrupts_and_exits_eight() {
    let dir = TempDir::new("sigint");
    make_village(&dir, &spawn_stub(Stub::SlowToolEveryRound).0, None);
    let mut child = Command::new(BIN)
        .args([
            "ask", "--data-dir", data_dir(&dir), "--start", "reception", "--secrets", "env",
            "こんにちは",
        ])
        .env(SECRET_VAR, "dummy-key")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // スタブが 3 秒待っている間（ターンの飛行中）に送る。
    std::thread::sleep(Duration::from_millis(1500));
    let status = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let deadline = Instant::now() + Duration::from_secs(60);
    let code = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status.code().unwrap_or(-1);
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("60 秒で終わらなかった");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut stderr = String::new();
    child.stderr.take().unwrap().read_to_string(&mut stderr).unwrap();
    assert_eq!(code, 8, "stderr={stderr}");
    let log = std::fs::read_to_string(dir.workspace().join("fuseforks.log")).unwrap();
    // 打ち切りの出口の払いの行は `turn interrupted:`（`turn: agent=` ではない — P6 の実機で確認）。
    assert!(log.contains("] turn interrupted: agent="), "払いの行が残る: {log}");
}

/// 扉の合鍵（`door_token`）が無いまま `--door-port` を付けた `check --for serve` は拒否 = 3
/// （Spec 65 D9 — 開くと言ったのに開けない、を起動前に止める）。
#[test]
fn a_door_port_without_its_token_is_rejected() {
    let dir = TempDir::new("doorcheck");
    make_village(&dir, "http://127.0.0.1:9/v1", None);
    let run = run(
        &[
            "check", "--for", "serve", "--data-dir", data_dir(&dir), "--start", "batch",
            "--secrets", "env", "--door-port", "39641",
        ],
        true,
    );
    assert_eq!(run.code, 3, "stdout={} stderr={}", run.stdout, run.stderr);
    assert!(run.stdout.contains("DOOR_TOKEN_MISSING"), "{}", run.stdout);
    assert!(run.stdout.contains("FUSEFORKS_SECRET_DOOR_TOKEN"), "{}", run.stdout);
}

/// 空いているループバックのポートを 1 つ選ぶ（選んで手放す。直後に別のプロセスに取られる
/// ことはありうるが、テストの間の競合は起きない程度）。
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// 扉へ MCP の initialize を 1 回送り、HTTP の状態コードを返す。届かなければ `None`。
fn door_status(port: u16, token: Option<&str>) -> Option<u16> {
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#;
    let auth = token
        .map(|t| format!("authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "POST /mcp HTTP/1.1\r\nhost: 127.0.0.1:{port}\r\ncontent-type: application/json\r\n\
         accept: application/json, text/event-stream\r\n{auth}content-length: {}\r\n\
         connection: close\r\n\r\n{body}",
        body.len()
    );
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok()?;
    stream.write_all(request.as_bytes()).ok()?;
    let mut head = [0u8; 12];
    stream.read_exact(&mut head).ok()?;
    String::from_utf8_lossy(&head[9..12]).parse().ok()
}

/// `serve --door-port` は `mcp_server.json` を読まずに、秘密の `door_token` を合鍵にして扉を開く
/// （Spec 65 D9）。合鍵が合えば 200、無ければ 401。村には `mcp_server.json` が無い（= GUI の設定では
/// 扉は閉じている）ので、開いたのは引数と秘密だけによる。
#[test]
fn serve_with_a_door_port_opens_the_door_with_the_secret_token() {
    let dir = TempDir::new("door");
    make_village(&dir, &spawn_stub(Stub::Answer).0, None);
    assert!(!dir.0.join("mcp_server.json").exists(), "GUI の扉の設定は無い");
    let port = free_port();
    let mut child = Command::new(BIN)
        .args([
            "serve", "--data-dir", data_dir(&dir), "--start", "batch", "--secrets", "env",
            "--door-port", &port.to_string(),
        ])
        .env(SECRET_VAR, "dummy-key")
        .env("FUSEFORKS_SECRET_DOOR_TOKEN", "door-test-token")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    let opened = loop {
        if let Some(code) = door_status(port, Some("door-test-token")) {
            break code;
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!("serve が先に終わった: {status:?}");
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("60 秒で扉が開かなかった");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let without = door_status(port, None);
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(opened, 200, "合鍵が合えば通る");
    assert_eq!(without, Some(401), "合鍵が無ければ通さない");
    assert!(!dir.0.join("mcp_server.json").exists(), "設定ファイルは書かない");
}

/// `bake` → 写しに対する `check --for serve` → 同じ写し先への 2 回目の `bake` は 11（Spec 65）。
/// 写しは別の data_dir として開けて、写しの棚に `bake.json` がある。
#[test]
fn bake_makes_a_copy_that_check_can_open() {
    let dir = TempDir::new("bakesrc");
    make_village(&dir, "http://127.0.0.1:9/v1", None);
    let out_root = TempDir::new("bakeout");
    let out = out_root.0.join("copy");
    let out_str = out.to_str().unwrap();
    let bake = |extra: &[&str]| {
        let mut args = vec![
            "bake", "--data-dir", data_dir(&dir), "--out", out_str, "--map", "D:\\Github=/work",
            "--source-time-zone", "Asia/Tokyo",
        ];
        args.extend_from_slice(extra);
        run(&args, true)
    };
    let first = bake(&["--json"]);
    assert_eq!(first.code, 0, "stdout={} stderr={}", first.stdout, first.stderr);
    let report: serde_json::Value = serde_json::from_str(first.stdout.trim()).unwrap();
    assert_eq!(report["sourceTimeZone"], "Asia/Tokyo");
    assert!(out.join("bake.json").is_file());

    let check = run(
        &["check", "--for", "serve", "--data-dir", out_str, "--start", "batch", "--secrets", "env"],
        true,
    );
    assert_eq!(check.code, 0, "stdout={} stderr={}", check.stdout, check.stderr);

    let again = bake(&[]);
    assert_eq!(again.code, 11, "stderr={}", again.stderr);
    let update = run(
        &["bake", "--data-dir", data_dir(&dir), "--out", out_str, "--update", "--source-time-zone", "Asia/Tokyo"],
        true,
    );
    assert_eq!(update.code, 0, "stdout={} stderr={}", update.stdout, update.stderr);
    assert!(update.stdout.contains("作り直し"), "{}", update.stdout);
}

/// `--start` を省くと引数の誤り = 2（既定値は無い）。
#[test]
fn omitting_start_is_a_usage_error() {
    let dir = TempDir::new("usage");
    let run = run(&["ask", "--data-dir", data_dir(&dir), "こんにちは"], true);
    assert_eq!(run.code, 2, "stderr={}", run.stderr);
    assert!(run.stderr.contains("--start"), "{}", run.stderr);
    // 村を開いていない — workspace すら作られない。
    assert!(!Path::new(&dir.workspace()).exists());
}

/// 像を確かめる村（`deploy/fixtures/village`。Spec 65 D10）がこの版の CLI で読めて、
/// 秘密があれば 0・無ければ 3 になる。**読めないと `verify-image.yml` が像の不具合に見える** —
/// 村の欄が変わって fixture が古くなったら、ここで先に落ちる。`check` は何も書かないので
/// リポジトリの中の fixture を直接開いてよい（終わったあと何も増えていないことも見る）。
#[test]
fn the_image_fixture_passes_check() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/fixtures/village");
    let fixture = fixture.to_str().unwrap();
    let args = ["check", "--for", "serve", "--data-dir", fixture, "--start", "batch", "--secrets", "env"];
    let with_secret = run(&args, true);
    assert_eq!(with_secret.code, 0, "stdout={} stderr={}", with_secret.stdout, with_secret.stderr);
    let without = run(&args, false);
    assert_eq!(without.code, 3, "stdout={}", without.stdout);
    assert!(without.stdout.contains(SECRET_VAR), "{}", without.stdout);

    let mut files: Vec<_> = std::fs::read_dir(Path::new(fixture).join("workspace"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    files.sort();
    assert_eq!(files, ["village_id", "world.json"], "check が fixture に書いた");
}
