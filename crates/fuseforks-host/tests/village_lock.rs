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
    build_host, HostBootOptions, HostError, HostPaths, LockError, VillageLock,
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
        // 扉は開かない（ポートを掴まない）。設定は無いので読んでも OFF。
        open_door: false,
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
