//! `bake` の結合テスト（Spec 65 P2・`container_contract` 1〜9）。
//!
//! 元の村は**本物の保存の経路で作る** — `Orchestrator::bootstrap` → テンプレート・個体を登録 →
//! `ConfigStore` の書き込み口で個体別のファイル、`save_schedules` で予定、`ApprovalStore::approve` で
//! 承認。`world.json` を手で書くと、欄の名前を写し間違えたときに「読めない村」を作る。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fuseforks_core::command::{CommandPolicy, PendingCommand};
use fuseforks_core::model::{AgentId, AgentSpec, ConfigFileKind, CredentialSource, ModelTemplate};
use fuseforks_core::orchestrator::ProbeApprovals;
use fuseforks_core::schedule::{Acceptance, Recurrence, ScheduledTask, Weekday};
use fuseforks_core::schedule_probe::{ScheduleProbe, SessionMode};
use fuseforks_core::secret::SecretStore;
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
};
use fuseforks_host::bake::{bake, BakeError, BakeManifest, BakeRequest, PathMap};
use fuseforks_host::probe_approvals::ApprovalStore;
use fuseforks_host::{HostPaths, VillageLock};

struct TempDir(PathBuf);

/// 一時フォルダの通し番号。**時刻だけで名前を作らない** — 並列に走るテストが同じ札（`src` / `out`）で
/// 同じナノ秒を引くと 1 つのフォルダを共有し、6 回に 1 回ほど落ちた（Windows の時刻の分解能は 100 ns）。
static NEXT_DIR: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-bake-{tag}-{}-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const WORK: &str = "D:\\Github\\Outcasts-MathLab";
const RAG: &str = "D:\\ManualeRAG";

fn id(raw: &str) -> AgentId {
    AgentId::from(raw)
}

fn probe(command: &str, cwd: Option<&str>) -> ScheduleProbe {
    ScheduleProbe {
        command: command.to_owned(),
        args: vec!["check.py".to_owned()],
        expect: "ok".to_owned(),
        timeout_secs: 10,
        cwd: cwd.map(str::to_owned),
    }
}

fn task(task_id: &str, probe_cwd: Option<&str>) -> ScheduledTask {
    ScheduledTask {
        id: task_id.to_owned(),
        to: id("agent_1"),
        message: "見張って".to_owned(),
        recurrence: Recurrence::Weekly {
            weekday: Weekday::Tue,
            hour: 16,
            minute: 38,
        },
        created_at_ms: 0,
        last_consumed_due_ms: Some(1_000),
        enabled: true,
        probe: Some(probe("python", probe_cwd)),
        session_mode: SessionMode::Continue,
        summarize_after: false,
        acceptance: Some(Acceptance {
            probe: probe("pytest", probe_cwd),
            max_attempts: 2,
        }),
        auto_approve_plans: false,
    }
}

/// 元の村（GUI の data_dir）を作る。個体 `agent_1` は Windows の作業フォルダと rag の宣言を持つ。
async fn make_source(dir: &Path) -> String {
    let workspace = HostPaths::new(dir).workspace();
    let store = ConfigStore::new(&workspace);
    let secrets = InMemorySecretStore::new();
    secrets.set("stub", "setup-only").unwrap();
    secrets.set("spare", "setup-only").unwrap();
    let orchestrator = Orchestrator::bootstrap(
        store.clone(),
        Arc::new(FixedBackendFactory::echo("[echo]")),
        Arc::new(secrets),
        OrchestratorConfig {
            run_schedules: false,
            ..OrchestratorConfig::default()
        },
    )
    .await
    .unwrap();
    let mut template = ModelTemplate::new("stub", "スタブ", "stub-model");
    template.credential = CredentialSource::Keyring;
    orchestrator.upsert_template(template).await.unwrap();
    // 個体が使わないテンプレート（--env-out は書き出さない — Spec 66 D3）。
    let mut spare = ModelTemplate::new("spare", "予備", "spare-model");
    spare.credential = CredentialSource::Keyring;
    orchestrator.upsert_template(spare).await.unwrap();
    let mut spec = AgentSpec::new(id("agent_1"), "窓口", "stub");
    spec.work_dir = Some(WORK.to_owned());
    spec.rag_sources = vec![RAG.to_owned()];
    orchestrator.create_agent(spec).await.unwrap();
    drop(orchestrator);

    let agent = id("agent_1");
    store
        .write_config(&agent, ConfigFileKind::Construct, "# 窓口\n作業は D:\\Github\\Outcasts-MathLab で行う")
        .await
        .unwrap();
    store
        .write_config(&agent, ConfigFileKind::Memory, "GUI で覚えたこと")
        .await
        .unwrap();
    store
        .write_config(
            &agent,
            ConfigFileKind::Mcp,
            r#"{ "mcpServers": { "outcasts": { "type": "http", "url": "https://outcasts.example/mcp",
                 "headers": { "Authorization": "Bearer ${secret:OUTCASTS}" } } } }"#,
        )
        .await
        .unwrap();
    let policy = CommandPolicy {
        allow: vec!["git status".to_owned(), "git log *".to_owned()],
        ..CommandPolicy::default()
    };
    store
        .write_config(&agent, ConfigFileKind::Run, &serde_json::to_string(&policy).unwrap())
        .await
        .unwrap();
    store
        .save_schedules(&[task("t-cwd", Some(WORK)), task("t-plain", None)])
        .await
        .unwrap();
    let village_id = store.village_id().await.unwrap();
    // 2 件とも人が承認した（前判定・後判定の 4 本）。
    let approvals = ApprovalStore::load(dir);
    for t in [task("t-cwd", Some(WORK)), task("t-plain", None)] {
        for p in [t.probe.as_ref().unwrap(), &t.acceptance.as_ref().unwrap().probe] {
            approvals.approve(p.approval_key(&village_id)).unwrap();
        }
    }
    // 扉の合鍵（写さない）と、写す棚。
    std::fs::write(dir.join("mcp_server.json"), r#"{"enabled":true,"port":39641,"token":"t"}"#).unwrap();
    std::fs::write(dir.join("jev.json"), r#"{"enabled":false}"#).unwrap();
    village_id
}

fn pending(command: &str, args: &[&str]) -> PendingCommand {
    PendingCommand {
        command: command.to_owned(),
        args: args.iter().map(|a| (*a).to_owned()).collect(),
        first_requested_at_ms: 1,
        count: 1,
    }
}

fn maps() -> Vec<PathMap> {
    vec![
        PathMap { from: "D:\\Github".to_owned(), to: "/work".to_owned() },
        PathMap { from: WORK.to_owned(), to: "/work/mathlab".to_owned() },
        PathMap { from: RAG.to_owned(), to: "/work/manuale-rag".to_owned() },
    ]
}

fn request(source: &Path, out: &Path, maps: Vec<PathMap>, update: bool) -> BakeRequest {
    BakeRequest {
        source: source.to_path_buf(),
        out: out.to_path_buf(),
        maps,
        update,
        source_time_zone: Some("Asia/Tokyo".to_owned()),
        allow_plaintext_headers: false,
        app_version: "test".to_owned(),
        env_out: None,
    }
}

/// 初回: パスの欄を最長一致で置き換え、承認を運び、Memory を seed し、扉の合鍵は写さない。
#[tokio::test]
async fn an_initial_bake_maps_paths_carries_approvals_and_skips_the_door() {
    let src = TempDir::new("src");
    let village_id = make_source(&src.0).await;
    let out_dir = TempDir::new("out");
    let out = out_dir.0.join("copy");

    let report = bake(&request(&src.0, &out, maps(), false), &InMemorySecretStore::new()).await.unwrap();
    assert_eq!(report.mapped_fields, 4, "workDir 1 + ragSources 1 + cwd 2（前判定と後判定）");
    assert_eq!(report.carried_approvals, 4);
    assert_eq!(report.seeded_memories, vec!["agent_1".to_owned()]);

    let out_store = ConfigStore::new(HostPaths::new(&out).workspace());
    let world = out_store.load_world().await.unwrap();
    assert_eq!(world.agents[0].work_dir.as_deref(), Some("/work/mathlab"), "最長一致");
    assert_eq!(world.agents[0].rag_sources, vec!["/work/manuale-rag".to_owned()]);
    // 置き換えた欄以外は値として等しい（契約 4）。
    let mut original = ConfigStore::new(HostPaths::new(&src.0).workspace()).load_world().await.unwrap();
    original.agents[0].work_dir = world.agents[0].work_dir.clone();
    original.agents[0].rag_sources = world.agents[0].rag_sources.clone();
    assert_eq!(
        serde_json::to_value(&original).unwrap(),
        serde_json::to_value(&world).unwrap()
    );

    let tasks = out_store.load_schedules().await.unwrap().tasks;
    let cwd_task = tasks.iter().find(|t| t.id == "t-cwd").unwrap();
    assert_eq!(cwd_task.probe.as_ref().unwrap().cwd.as_deref(), Some("/work/mathlab"));
    assert!(tasks.iter().all(|t| t.last_consumed_due_ms.is_none()), "初回は消化の記録を捨てる");

    // 承認は置き換えた後の鍵で運ばれ、同じ村の識別子のまま。
    let carried = ApprovalStore::load(&out);
    for t in &tasks {
        for p in [t.probe.as_ref().unwrap(), &t.acceptance.as_ref().unwrap().probe] {
            assert!(carried.is_approved(&p.approval_key(&village_id)), "{}", t.id);
        }
    }
    assert_eq!(out_store.read_village_id().await.as_deref(), Some(village_id.as_str()));

    assert!(!out.join("mcp_server.json").exists(), "扉の合鍵は写さない");
    assert!(out.join("jev.json").is_file());
    assert!(!HostPaths::new(&out).workspace().join("sessions.redb").exists(), "会話は写さない");
    let manifest: BakeManifest =
        serde_json::from_str(&std::fs::read_to_string(out.join("bake.json")).unwrap()).unwrap();
    assert_eq!(manifest.source_time_zone, "Asia/Tokyo");
    assert_eq!(manifest.source_village_id, village_id);
    assert_eq!(manifest.maps, maps());
    // Construct.md の Windows パスは書き換えず、警告で名指しする。
    let construct = std::fs::read_to_string(
        HostPaths::new(&out).workspace().join("agents/agent_1/Construct.md"),
    )
    .unwrap();
    assert!(construct.contains(WORK));
    assert!(
        report.warnings.iter().any(|w| w.code == "FREE_TEXT_WINDOWS_PATH" && w.message.contains("Construct.md")),
        "{:?}",
        report.warnings
    );
}

/// 置き換えなかったパスが 1 つでもあれば止め、写しを 1 バイトも書かない（契約 3）。
#[tokio::test]
async fn an_unmapped_path_stops_without_writing() {
    let src = TempDir::new("src");
    make_source(&src.0).await;
    let out_dir = TempDir::new("out");
    let out = out_dir.0.join("copy");
    let only_work = vec![PathMap { from: WORK.to_owned(), to: "/work/mathlab".to_owned() }];
    let err = bake(&request(&src.0, &out, only_work, false), &InMemorySecretStore::new()).await.unwrap_err();
    let BakeError::Unmapped(found) = &err else { panic!("{err}") };
    assert_eq!(found, &vec![format!("agents[agent_1].ragSources[0] = {RAG}")]);
    assert!(!out.exists(), "写し先は作らない");
    assert!(
        std::fs::read_dir(&out_dir.0).unwrap().next().is_none(),
        "一時フォルダも残さない"
    );
}

/// 平文の鍵は止める（10 相当）。`--allow-plaintext-headers` なら通して警告する。
#[tokio::test]
async fn plaintext_headers_stop_unless_allowed() {
    let src = TempDir::new("src");
    make_source(&src.0).await;
    let store = ConfigStore::new(HostPaths::new(&src.0).workspace());
    store
        .write_config(
            &id("agent_1"),
            ConfigFileKind::Mcp,
            r#"{ "mcpServers": { "elyth": { "type": "http", "url": "https://elyth.example/mcp", "enabled": false,
                 "headers": { "Authorization": "Bearer plain-token", "Accept": "application/json" } } } }"#,
        )
        .await
        .unwrap();
    let out_dir = TempDir::new("out");
    let out = out_dir.0.join("copy");
    let err = bake(&request(&src.0, &out, maps(), false), &InMemorySecretStore::new()).await.unwrap_err();
    let BakeError::PlaintextHeaders(found) = &err else { panic!("{err}") };
    assert_eq!(found, &vec!["agents/agent_1/mcp.json: elyth / Authorization".to_owned()], "無効なサーバーも数える");
    assert!(!err.to_string().contains("plain-token"), "値は載せない: {err}");
    assert!(!out.exists());

    let mut allowed = request(&src.0, &out, maps(), false);
    allowed.allow_plaintext_headers = true;
    let report = bake(&allowed, &InMemorySecretStore::new()).await.unwrap();
    assert!(report.warnings.iter().any(|w| w.code == "PLAINTEXT_HEADER_COPIED"));
}

/// 再 bake: 設計は置き換え、実行（Memory・会話）は触らず、同居は欄ごとに合流する（契約 2）。
#[tokio::test]
async fn an_update_replaces_design_and_keeps_what_grew_in_the_container() {
    let src = TempDir::new("src");
    make_source(&src.0).await;
    let out_dir = TempDir::new("out");
    let out = out_dir.0.join("copy");
    bake(&request(&src.0, &out, maps(), false), &InMemorySecretStore::new()).await.unwrap();

    // コンテナの側で育ったもの。
    let out_ws = HostPaths::new(&out).workspace();
    let out_store = ConfigStore::new(&out_ws);
    let agent = id("agent_1");
    out_store.write_config(&agent, ConfigFileKind::Memory, "コンテナで覚えたこと").await.unwrap();
    std::fs::write(out_ws.join("sessions.redb"), b"container sessions").unwrap();
    let mut tasks = out_store.load_schedules().await.unwrap().tasks;
    for t in &mut tasks {
        t.last_consumed_due_ms = Some(42);
    }
    out_store.save_schedules(&tasks).await.unwrap();
    out_store
        .update_command_policy(&agent, |p| {
            p.allow.push("make test".to_owned()); // 自動承認で書き足された行
            p.pending.push(pending("cargo", &["build"]));
            p.pending.push(pending("git", &["log", "-5"]));
        })
        .await
        .unwrap();

    // GUI の側で直したもの: Construct・予定を 1 件消す。
    let src_store = ConfigStore::new(HostPaths::new(&src.0).workspace());
    src_store.write_config(&agent, ConfigFileKind::Construct, "# 窓口 v2").await.unwrap();
    src_store.save_schedules(&[task("t-plain", None)]).await.unwrap();

    let report = bake(&request(&src.0, &out, Vec::new(), true), &InMemorySecretStore::new()).await.unwrap();
    assert!(report.update);
    assert_eq!(report.maps, maps(), "--map を省けば前回のものを引き継ぐ");

    assert_eq!(
        std::fs::read_to_string(out_ws.join("agents/agent_1/Construct.md")).unwrap(),
        "# 窓口 v2",
        "設計は置き換える"
    );
    assert_eq!(
        std::fs::read_to_string(out_ws.join("agents/agent_1/Memory.md")).unwrap(),
        "コンテナで覚えたこと",
        "Memory は触らない"
    );
    assert!(report.seeded_memories.is_empty());
    assert_eq!(std::fs::read(out_ws.join("sessions.redb")).unwrap(), b"container sessions");

    let tasks = out_store.load_schedules().await.unwrap().tasks;
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].last_consumed_due_ms, Some(42), "消化の記録は id ごとに残す");
    assert!(report.warnings.iter().any(|w| w.code == "CONSUMED_RECORD_DROPPED" && w.message.contains("t-cwd")));

    let policy = out_store.read_command_policy(&agent).await.unwrap();
    assert_eq!(policy.allow, vec!["git status".to_owned(), "git log *".to_owned()], "規則は GUI");
    let pending: Vec<&str> = policy.pending.iter().map(|p| p.command.as_str()).collect();
    assert_eq!(pending, vec!["cargo"], "決着した git log -5 は落ち、未決着の cargo は残る");
    assert!(report.warnings.iter().any(|w| w.code == "ALLOW_LINE_DROPPED" && w.message.contains("make test")));
}

/// 写し先の状態（11 相当）と、元の村のロック（4 相当）。
#[tokio::test]
async fn out_state_and_the_source_lock_stop_the_bake() {
    let src = TempDir::new("src");
    make_source(&src.0).await;
    let out_dir = TempDir::new("out");

    // --update なしで空でない写し先。
    std::fs::write(out_dir.0.join("something"), b"x").unwrap();
    let err = bake(&request(&src.0, &out_dir.0, maps(), false), &InMemorySecretStore::new()).await.unwrap_err();
    assert!(matches!(err, BakeError::OutState(_)), "{err}");
    // --update で bake.json が無い写し先。
    let err = bake(&request(&src.0, &out_dir.0, maps(), true), &InMemorySecretStore::new()).await.unwrap_err();
    assert!(matches!(err, BakeError::OutState(_)), "{err}");

    // GUI が元の村を開いている（ロックを持っている）。
    let _held = VillageLock::acquire(&HostPaths::new(&src.0).workspace()).unwrap();
    let err = bake(&request(&src.0, &out_dir.0.join("copy"), maps(), false), &InMemorySecretStore::new()).await.unwrap_err();
    assert!(matches!(err, BakeError::Lock(_)), "{err}");
}


// ---- Spec 66: --env-out ----

fn with_env(source: &Path, out: &Path, env: &Path, update: bool) -> BakeRequest {
    let mut req = request(source, out, if update { Vec::new() } else { maps() }, update);
    req.env_out = Some(env.to_path_buf());
    req
}

fn store_with(pairs: &[(&str, &str)]) -> InMemorySecretStore {
    let store = InMemorySecretStore::new();
    for (key, value) in pairs {
        store.set(key, value).unwrap();
    }
    store
}

fn active_lines(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(str::to_owned)
        .collect()
}

/// 書き出すのは村が要る鍵だけ（使わないテンプレート・無効な Jev は書かない）。ストアに無い MCP の参照は
/// 印つきコメント。値は単一引用符。扉の合鍵は 32 桁 hex で新しく作る。報告に値は載らない。
#[tokio::test]
async fn env_out_writes_only_what_the_village_needs() {
    let src = TempDir::new("src");
    make_source(&src.0).await;
    let out_dir = TempDir::new("out");
    let out = out_dir.0.join("copy");
    let env = out_dir.0.join("deploy").join(".env");
    let secrets = store_with(&[("stub", "sk-$x#y"), ("spare", "must-not-appear")]);

    let report = bake(&with_env(&src.0, &out, &env, false), &secrets).await.unwrap();
    let text = std::fs::read_to_string(&env).unwrap();
    let active = active_lines(&text);
    assert_eq!(active[0], "TZ='Asia/Tokyo'");
    assert_eq!(active[1], "FUSEFORKS_SECRET_STUB='sk-$x#y'");
    let door = active[2]
        .strip_prefix("FUSEFORKS_SECRET_DOOR_TOKEN='")
        .and_then(|rest| rest.strip_suffix('\''))
        .unwrap();
    assert!(door.len() == 32 && door.chars().all(|c| c.is_ascii_hexdigit()), "{door}");
    assert_eq!(active.len(), 3, "{text}");
    assert!(text.contains("# FUSEFORKS_SECRET_MCP_OUTCASTS=   # bake:"), "{text}");
    assert!(!text.contains("SPARE") && !text.contains("must-not-appear"), "{text}");
    assert!(!text.contains("JEV"), "Jev は無効・判断役なし: {text}");

    let env_report = report.env.clone().unwrap();
    assert_eq!(env_report.written, ["TZ", "FUSEFORKS_SECRET_STUB"]);
    assert_eq!(env_report.missing, ["FUSEFORKS_SECRET_MCP_OUTCASTS"]);
    assert!(env_report.door_token_created);
    let json = serde_json::to_string(&report).unwrap();
    assert!(!json.contains("sk-$x#y") && !json.contains(door), "報告に値が出た: {json}");
    assert!(out.join("bake.json").is_file(), "写しも作る");
}

/// 圧縮が有効なら Jev の鍵も対象（ストアに無ければ印つきコメント）。
#[tokio::test]
async fn env_out_includes_jev_only_when_pruning_or_judges_are_on() {
    let src = TempDir::new("src");
    make_source(&src.0).await;
    std::fs::write(src.0.join("jev.json"), r#"{"enabled":true}"#).unwrap();
    let out_dir = TempDir::new("out");
    let env = out_dir.0.join(".env");
    bake(&with_env(&src.0, &out_dir.0.join("copy"), &env, false), &store_with(&[("stub", "k")]))
        .await
        .unwrap();
    let text = std::fs::read_to_string(&env).unwrap();
    assert!(text.contains("# FUSEFORKS_SECRET_JEV_API_TOKEN=   # bake:"), "{text}");
}

/// 既にある .env は上書きしない — `--update` なしなら 11 で止め、写しも作らない（計画の段）。
#[tokio::test]
async fn an_existing_env_without_update_stops_before_writing_anything() {
    let src = TempDir::new("src");
    make_source(&src.0).await;
    let out_dir = TempDir::new("out");
    let out = out_dir.0.join("copy");
    let env = out_dir.0.join(".env");
    std::fs::write(&env, "FUSEFORKS_DOMAIN=example.com\n").unwrap();
    let err = bake(&with_env(&src.0, &out, &env, false), &store_with(&[("stub", "k")]))
        .await
        .unwrap_err();
    assert!(matches!(err, BakeError::OutState(ref m) if m.contains("--env-out")), "{err}");
    assert!(!out.exists(), "写しも作らない");
    assert_eq!(std::fs::read_to_string(&env).unwrap(), "FUSEFORKS_DOMAIN=example.com\n");
}

/// 写しと同じ・写しの下・`..` を挟んで写しの下 → 引数の誤り（2）。何も書かない。
#[tokio::test]
async fn an_env_inside_the_copy_is_a_usage_error() {
    let src = TempDir::new("src");
    make_source(&src.0).await;
    let out_dir = TempDir::new("out");
    let out = out_dir.0.join("copy");
    for env in [
        out.clone(),
        out.join("workspace").join(".env"),
        out_dir.0.join("deploy").join("..").join("copy").join(".env"),
    ] {
        let err = bake(&with_env(&src.0, &out, &env, false), &store_with(&[("stub", "k")]))
            .await
            .unwrap_err();
        assert!(matches!(err, BakeError::Usage(_)), "{}: {err}", env.display());
        assert!(!out.exists(), "{}", env.display());
    }
}

/// `--update`: 既にある行（運用者の行・扉の合鍵）は残し、印つきコメントは値が揃えば置き換える。
#[tokio::test]
async fn an_update_fills_placeholders_and_keeps_the_operators_lines() {
    let src = TempDir::new("src");
    make_source(&src.0).await;
    let out_dir = TempDir::new("out");
    let out = out_dir.0.join("copy");
    let env = out_dir.0.join(".env");
    bake(&with_env(&src.0, &out, &env, false), &store_with(&[("stub", "k1")]))
        .await
        .unwrap();
    let first = std::fs::read_to_string(&env).unwrap();
    let door = first
        .lines()
        .find(|l| l.starts_with("FUSEFORKS_SECRET_DOOR_TOKEN="))
        .unwrap()
        .to_owned();
    std::fs::write(&env, format!("{first}FUSEFORKS_DOMAIN=fuseforks.example.com\n")).unwrap();

    let report = bake(
        &with_env(&src.0, &out, &env, true),
        &store_with(&[("stub", "k2"), ("mcp:OUTCASTS", "o1")]),
    )
    .await
    .unwrap();
    let text = std::fs::read_to_string(&env).unwrap();
    assert!(text.lines().any(|l| l == "FUSEFORKS_SECRET_MCP_OUTCASTS='o1'"), "{text}");
    assert!(!text.contains("# FUSEFORKS_SECRET_MCP_OUTCASTS="), "{text}");
    assert!(text.lines().any(|l| l == "FUSEFORKS_SECRET_STUB='k1'"), "値は書き換えない: {text}");
    assert!(text.lines().any(|l| l == door), "扉の合鍵は作り直さない");
    assert!(text.lines().any(|l| l == "FUSEFORKS_DOMAIN=fuseforks.example.com"), "{text}");
    let env_report = report.env.clone().unwrap();
    assert_eq!(env_report.written, ["FUSEFORKS_SECRET_MCP_OUTCASTS"]);
    assert_eq!(env_report.differs, ["FUSEFORKS_SECRET_STUB"]);
    assert!(!env_report.door_token_created);
    assert!(report
        .warnings
        .iter()
        .any(|w| w.code == "ENV_VALUE_DIFFERS" && !w.message.contains("k2")));
}

/// 資格情報ストアが読めなければ 5 — 写しも .env も書かない（計画の段）。
#[tokio::test]
async fn an_unreadable_store_stops_before_writing_anything() {
    struct Broken;
    impl SecretStore for Broken {
        fn get(&self, _: &str) -> fuseforks_core::CoreResult<Option<String>> {
            Err(fuseforks_core::CoreError::SecretStore {
                operation: "取得",
                message: "no service".into(),
            })
        }
        fn set(&self, _: &str, _: &str) -> fuseforks_core::CoreResult<()> {
            unreachable!()
        }
        fn delete(&self, _: &str) -> fuseforks_core::CoreResult<()> {
            unreachable!()
        }
    }
    let src = TempDir::new("src");
    make_source(&src.0).await;
    let out_dir = TempDir::new("out");
    let out = out_dir.0.join("copy");
    let env = out_dir.0.join(".env");
    let err = bake(&with_env(&src.0, &out, &env, false), &Broken).await.unwrap_err();
    assert!(matches!(err, BakeError::Io(ref m) if m.contains("資格情報ストア")), "{err}");
    assert!(!out.exists() && !env.exists());
}
