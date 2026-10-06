//! `sessions.redb` を別のプロセスが開いているとき、`bootstrap` が
//! `SESSION_STORE_LOCKED` で止まること（Spec 64 凍結 4 = 二重の網）。
//!
//! 「別のプロセス」はここでは同じプロセスの別ハンドルで代える — redb は 2 つを
//! 区別しない（どちらも `DatabaseAlreadyOpen`。Spec 64 P0 の実測）ので、
//! 子プロセスを起こさなくても同じ経路を踏める。
//!
//! **止めるのは 2 重オープンだけ。** 壊れたファイルは今までどおり WARN で
//! 「会話を保存しない起動」を続ける — その対照をここで一緒に留める。

use std::path::PathBuf;
use std::sync::Arc;

use fuseforks_core::{
    ConfigStore, CoreError, HttpBackendFactory, InMemorySecretStore, Orchestrator,
    OrchestratorConfig,
};

/// テスト用の一時ディレクトリ。終了時に破棄する。
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-store-locked-{tag}-{}",
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

async fn boot(dir: &TempDir) -> Result<Orchestrator, CoreError> {
    let secrets: Arc<dyn fuseforks_core::SecretStore> = Arc::new(InMemorySecretStore::new());
    Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(HttpBackendFactory::echo_on_failure(Arc::clone(&secrets))),
        secrets,
        OrchestratorConfig::default(),
    )
    .await
}

/// 別のハンドルが開いている間は `SESSION_STORE_LOCKED` で止まり、閉じれば開ける。
#[tokio::test(flavor = "multi_thread")]
async fn bootstrap_stops_while_another_handle_holds_the_session_store() {
    let dir = TempDir::new("held");
    let held = redb::Database::create(dir.0.join("sessions.redb")).expect("先に開けること");

    let err = boot(&dir).await.err().expect("2 重オープンでは起動しない");
    assert!(
        matches!(err, CoreError::SessionStoreLocked { .. }),
        "別の variant で返る: {err}"
    );
    assert_eq!(err.code(), "SESSION_STORE_LOCKED");

    drop(held);
    boot(&dir).await.expect("相手が閉じれば開ける");
}

/// 壊れたファイルでは止めない（WARN で続ける — 今までどおり）。
#[tokio::test(flavor = "multi_thread")]
async fn a_corrupt_session_store_still_boots() {
    let dir = TempDir::new("corrupt");
    std::fs::write(dir.0.join("sessions.redb"), b"not a redb file").unwrap();

    boot(&dir)
        .await
        .expect("壊れたファイルは 2 重オープンではないので起動を止めない");
}
