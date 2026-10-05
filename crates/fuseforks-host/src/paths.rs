//! 村とその棚の置き場（Spec 64 D2）。
//!
//! **村の場所は `data_dir` から導く 1 通りだけ。** 欄を 2 つ持つと、将来どちらかを
//! 別に指したくなったときに 2 つが食い違う形が生まれる。GUI は Tauri の
//! `app_data_dir()` を渡し（Windows は `%APPDATA%\jp.outcasts.fuseforks`）、
//! `fuseforks-cli` は `--data-dir` を必須で受ける（既定値を持たない）。
//!
//! 棚（`mcp_server.json` / `pricing.json` / `probe_approvals.json` / `jev.json`）は
//! `data_dir` 直下、村（`world.json` / `sessions.redb` / `agents/` / …）は
//! `data_dir/workspace`。**村を配っても扉は開かず・承認は付いてこず・単価の取得先も
//! 付いてこない**のは、この 2 層の置き場で成立している。

use std::path::{Path, PathBuf};

/// 村とその棚の置き場。
#[derive(Debug, Clone)]
pub struct HostPaths {
    data_dir: PathBuf,
}

impl HostPaths {
    /// `data_dir`（端末ごとの棚のルート）から作る。
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
        }
    }

    /// 端末ごとの棚のルート。
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// 村のルート。**常に `data_dir/workspace`**。
    pub fn workspace(&self) -> PathBuf {
        self.data_dir.join("workspace")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_workspace_is_always_under_the_data_dir() {
        let paths = HostPaths::new("/tmp/fuseforks-data");
        assert_eq!(paths.data_dir(), Path::new("/tmp/fuseforks-data"));
        assert_eq!(
            paths.workspace(),
            Path::new("/tmp/fuseforks-data").join("workspace")
        );
    }
}
