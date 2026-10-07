//! 村の排他ロックの結合テスト（Spec 64 D3 / P2）。
//!
//! `lock.rs` の単体は `VillageLock` だけを見る。ここで見るのは**配線** —
//! `build_host` が最初の手でロックを取ること、`Host` の Drop で外れること、
//! 別プロセスでも効くこと、ロックを迂回して `sessions.redb` だけを開かれても
//! `bootstrap` が止まること（二重の網）。
//!
//! `build_host` は `open_log` を呼ぶ。1 つのテストバイナリでは最初の 1 回しか
//! 効かない（`OnceLock`）ので、最初に走ったテストの一時フォルダのログに以後の
//! 行が溜まり、そのフォルダは Windows では開いたまま消せずに残る。中身は診断の
//! 行だけで、実害は無い。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use fuseforks_core::CoreError;
use fuseforks_host::{
    build_host, HostBootOptions, HostError, HostPaths, LockError, SecretSource, VillageLock,
};

/// 持ち主役の子プロセスへ渡す環境変数（値は workspace のパス）。
const HOLDER_ENV: &str = "FUSEFORKS_LOCK_HOLDER";

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-host-it-{tag}-{}",
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

fn opts() -> HostBootOptions {
    HostBootOptions {
        app_version: "test".to_owned(),
        // 開発機の資格情報ストアに触れない（村にテンプレートが無いので読まれもしない）。
        secrets: SecretSource::Env,
        // ティッカーも扉も起こさない（ポートを掴まない）。
        run_schedules: false,
        open_door: false,
        door_port: None,
    }
}

/// 同じ村を同じプロセスで 2 回組むと 2 回目が `Locked`。1 回目を落とせば取れる。
#[tokio::test(flavor = "multi_thread")]
async fn the_same_village_cannot_be_built_twice_in_one_process() {
    let dir = TempDir::new("twice");
    let paths = HostPaths::new(&dir.0);

    let first = build_host(&paths, opts()).await.expect("1 回目は開ける");
    let second = build_host(&paths, opts()).await;
    assert!(
        matches!(second, Err(HostError::Lock(LockError::Held { .. }))),
        "2 回目は Held で止まる: {:?}",
        second.err().map(|e| e.to_string())
    );

    // Host の Drop で外れる（ロックの寿命 = Host の寿命）。取れたことは
    // ロックそのもので確かめる — もう 1 度 build_host すると、1 回目の
    // オーケストレーターが握ったままの sessions.redb に当たりうる。
    drop(first);
    VillageLock::acquire(&paths.workspace()).expect("Host を落とした後は取れる");
}

/// 別プロセスが持っている間は `Locked`。殺せば取れる（ファイルは残る）。
#[tokio::test(flavor = "multi_thread")]
async fn another_process_holding_the_lock_blocks_build_host() {
    let dir = TempDir::new("other-process");
    let paths = HostPaths::new(&dir.0);
    let workspace = paths.workspace();
    std::fs::create_dir_all(&workspace).unwrap();

    // 自分（このテストバイナリ）を持ち主役として起こす。
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "lock_holder_process", "--nocapture"])
        .env(HOLDER_ENV, &workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("持ち主役を起こせる");
    let ready = workspace.join("holder.ready");
    let started = Instant::now();
    while !ready.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "持ち主役が 30 秒で準備できない"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let result = build_host(&paths, opts()).await;
    assert!(
        matches!(result, Err(HostError::Lock(LockError::Held { .. }))),
        "別プロセスが持つ間は Held: {:?}",
        result.err().map(|e| e.to_string())
    );

    child.kill().expect("持ち主役を殺せる");
    let _ = child.wait();
    VillageLock::acquire(&workspace).expect("殺した後は取れる（OS が外す）");
    assert!(
        workspace.join(fuseforks_host::LOCK_FILE).exists(),
        "ファイルは残る（存在で判定していない）"
    );
}

/// ロックを迂回して `sessions.redb` だけを開かれていても、`bootstrap` が
/// `SESSION_STORE_LOCKED` で止まる（二重の網 — 凍結 4）。
#[tokio::test(flavor = "multi_thread")]
async fn a_session_store_held_without_the_lock_still_stops_the_boot() {
    let dir = TempDir::new("bypass");
    let paths = HostPaths::new(&dir.0);
    let workspace = paths.workspace();
    std::fs::create_dir_all(&workspace).unwrap();
    let _held = redb::Database::create(workspace.join("sessions.redb")).expect("先に開けること");

    let err = build_host(&paths, opts()).await.err().expect("止まること");
    assert!(
        matches!(err, HostError::Core(CoreError::SessionStoreLocked { .. })),
        "{err}"
    );
}

/// 環境変数から読むとき、2 つの鍵が同じ変数名に写る村は組み立てを止める（Spec 64 D4）。
/// エラー文は変数名と鍵だけで、値は載らない（値はそもそも読んでいない）。
///
/// keyring のときは数えない（変数名に写さないので衝突が無い）が、それはここでは
/// 確かめない — keyring の組み立ては Jev の鍵を資格情報ストアから読みに行くので、
/// テストから開発機や CI の資格情報ストアに触れることになる。
#[tokio::test(flavor = "multi_thread")]
async fn colliding_secret_names_stop_an_env_boot() {
    let dir = TempDir::new("collide");
    let paths = HostPaths::new(&dir.0);
    std::fs::create_dir_all(paths.workspace()).unwrap();

    let mut world = fuseforks_core::world::World::new();
    world.upsert_template(fuseforks_core::model::ModelTemplate::new("a-b", "A", "m"));
    world.upsert_template(fuseforks_core::model::ModelTemplate::new("a_b", "B", "m"));
    fuseforks_core::ConfigStore::new(paths.workspace())
        .save_world(&world.to_persisted())
        .await
        .unwrap();

    let err = build_host(&paths, opts()).await.err().expect("衝突で止まる");
    let HostError::SecretNameCollision(found) = &err else {
        panic!("SecretNameCollision で止まること: {err}");
    };
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].variable, "FUSEFORKS_SECRET_A_B");
    assert_eq!(found[0].keys, vec!["a-b".to_owned(), "a_b".to_owned()]);
    let text = err.to_string();
    assert!(text.contains("FUSEFORKS_SECRET_A_B") && text.contains("a-b") && text.contains("a_b"), "{text}");
}

/// 数える鍵には**コードが持つ固定の鍵**（Jev のトークン `jev_api_token`）も入る。
/// テンプレート ID が 1 つしか無くても、それが固定の鍵と同じ変数名に写れば止める
/// （テンプレート同士だけを数える実装では素通りする形）。
#[tokio::test(flavor = "multi_thread")]
async fn a_template_colliding_with_the_jev_token_key_stops_the_boot() {
    let dir = TempDir::new("collide-jev");
    let paths = HostPaths::new(&dir.0);
    std::fs::create_dir_all(paths.workspace()).unwrap();

    let mut world = fuseforks_core::world::World::new();
    world.upsert_template(fuseforks_core::model::ModelTemplate::new("jev-api-token", "J", "m"));
    fuseforks_core::ConfigStore::new(paths.workspace())
        .save_world(&world.to_persisted())
        .await
        .unwrap();

    let err = build_host(&paths, opts()).await.err().expect("固定の鍵との衝突で止まる");
    let HostError::SecretNameCollision(found) = &err else {
        panic!("SecretNameCollision で止まること: {err}");
    };
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].variable, "FUSEFORKS_SECRET_JEV_API_TOKEN");
    assert_eq!(
        found[0].keys,
        vec!["jev-api-token".to_owned(), "jev_api_token".to_owned()]
    );
}

/// 持ち主役。環境変数が無ければ何もしない（普通に走ると空のテストとして緑）。
///
/// あればロックを取って `holder.ready` を作り、標準入力が閉じるか殺されるまで持つ。
#[test]
fn lock_holder_process() {
    let Ok(workspace) = std::env::var(HOLDER_ENV) else {
        return;
    };
    let workspace = Path::new(&workspace);
    let _lock = VillageLock::acquire(workspace).expect("持ち主役がロックを取れること");
    std::fs::write(workspace.join("holder.ready"), b"").expect("ready");
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
}
