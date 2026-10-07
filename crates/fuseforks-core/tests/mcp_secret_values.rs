//! Spec 65 P4 — GUI の「秘密の値」の欄が使うコアの 3 本（`set_mcp_secret` /
//! `has_mcp_secret` / `clear_mcp_secret`）。
//!
//! 留めるのは 3 点:
//! - **鍵は接続が引くものと同じ `mcp:NAME`**（[`mcp_secret_key`]）。GUI で保存した値を
//!   接続が引けない、を作らない
//! - 前後の空白を落とす（貼り付けの改行が 401 としてしか表面化しない — `set_credential` と同じ）
//! - 名前は `${secret:NAME}` に書ける形（`[A-Z0-9_]+`）だけ。それ以外は
//!   `INVALID_SECRET_NAME` で、ストアに触れない

use std::path::PathBuf;
use std::sync::Arc;

use fuseforks_core::mcp::mcp_secret_key;
use fuseforks_core::{
    ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
    SecretStore,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-mcpsecret-{tag}-{}-{}",
            std::process::id(),
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

async fn setup(tag: &str) -> (TempDir, Orchestrator, Arc<InMemorySecretStore>) {
    let dir = TempDir::new(tag);
    let secrets = Arc::new(InMemorySecretStore::new());
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(dir.0.join("workspace")),
        Arc::new(FixedBackendFactory::echo("[echo]")),
        secrets.clone(),
        OrchestratorConfig::default(),
    )
    .await
    .expect("bootstrap できること");
    (dir, orchestrator, secrets)
}

#[tokio::test]
async fn a_saved_value_lands_under_the_key_the_connector_reads() {
    let (_dir, orchestrator, secrets) = setup("save").await;
    assert!(!orchestrator.has_mcp_secret("OUTCASTS_TOKEN").unwrap());

    orchestrator
        .set_mcp_secret("OUTCASTS_TOKEN", "  token-value\n")
        .unwrap();
    assert_eq!(
        secrets.get(&mcp_secret_key("OUTCASTS_TOKEN")).unwrap().as_deref(),
        Some("token-value"),
        "鍵は mcp:NAME・前後の空白は落とす"
    );
    assert!(orchestrator.has_mcp_secret("OUTCASTS_TOKEN").unwrap());

    orchestrator.clear_mcp_secret("OUTCASTS_TOKEN").unwrap();
    assert!(!orchestrator.has_mcp_secret("OUTCASTS_TOKEN").unwrap());
    assert_eq!(secrets.get(&mcp_secret_key("OUTCASTS_TOKEN")).unwrap(), None);
}

#[tokio::test]
async fn a_name_that_cannot_be_referenced_is_rejected_without_touching_the_store() {
    let (_dir, orchestrator, secrets) = setup("name").await;
    for bad in ["", "outcasts", "OUT-CASTS", "OUT CASTS", "A}B"] {
        let err = orchestrator.set_mcp_secret(bad, "v").unwrap_err();
        assert_eq!(err.code(), "INVALID_SECRET_NAME", "{bad:?}");
        assert_eq!(secrets.get(&mcp_secret_key(bad)).unwrap(), None, "{bad:?}");
        assert_eq!(
            orchestrator.has_mcp_secret(bad).unwrap_err().code(),
            "INVALID_SECRET_NAME"
        );
    }
}
