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
                    Stub::ToolEveryRound => serde_json::json!({
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
