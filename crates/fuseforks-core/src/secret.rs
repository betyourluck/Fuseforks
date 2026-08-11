//! 秘密の保管。
//!
//! # なぜ設定ファイルでも環境変数でもないのか
//!
//! - **設定ファイル**: 平文で保存される。運用初日に実キーが貼られ、`world.json` に
//!   そのまま書き込まれた。「注意書きを添える」では防げない。
//! - **環境変数**: 端末から起動する開発者向けの作法で、デスクトップ GUI に合わない。
//!   利用者に「OS の環境変数を設定して、新しいターミナルから起動し直す」ことを
//!   要求する時点で、素人が使える道具ではなくなる。しかも Windows は設定済みの
//!   変数を起動済みプロセスへ伝播しないため、設定したのに効かない状態が普通に起きる。
//!
//! 残るのは **OS の資格情報ストア**である。ユーザー単位で OS が保護し、
//! アプリの画面だけで登録が完結する。
//!
//! この層は差し替え可能にしてある。テストは [`InMemorySecretStore`] を使い、
//! 実際の資格情報ストアに触れずに全経路を検証できる。

use std::collections::HashMap;
use std::sync::Mutex;

use crate::error::{CoreError, CoreResult};

/// 資格情報ストアのサービス名。OS 上ではこの名前で束ねられる。
pub const SERVICE_NAME: &str = "jp.outcasts.fuseforks";

/// 秘密の保管先。
///
/// **取得系は値を返すが、それ以外の経路へ値を出さないこと。**
/// エラーメッセージ・イベント・ログのいずれにも秘密を載せない。
pub trait SecretStore: Send + Sync {
    /// 秘密を取り出す。未登録なら `Ok(None)`。
    fn get(&self, key: &str) -> CoreResult<Option<String>>;

    /// 秘密を保存する。既存の値は置き換える。
    fn set(&self, key: &str, secret: &str) -> CoreResult<()>;

    /// 秘密を削除する。未登録でも成功として扱う（削除は冪等）。
    fn delete(&self, key: &str) -> CoreResult<()>;

    /// 登録済みかどうかだけを返す。値そのものは返さない。
    ///
    /// UI の「登録済み / 未登録」表示はこちらを使う。
    /// 表示のために値を取り出すと、秘密が UI 層のメモリへ載る理由が無いのに載る。
    fn contains(&self, key: &str) -> CoreResult<bool> {
        Ok(self.get(key)?.is_some())
    }
}

/// OS の資格情報ストアを使う実装。
///
/// - Windows: 資格情報マネージャー
/// - macOS: キーチェーン
/// - Linux: freedesktop Secret Service
pub struct KeyringSecretStore {
    service: String,
}

impl KeyringSecretStore {
    /// 既定のサービス名で作る。
    pub fn new() -> Self {
        Self {
            service: SERVICE_NAME.to_owned(),
        }
    }

    /// サービス名を指定して作る。テストで実ストアを汚したくない場合に使う。
    pub fn with_service(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    fn entry(&self, key: &str) -> CoreResult<keyring::Entry> {
        keyring::Entry::new(&self.service, key).map_err(|err| CoreError::SecretStore {
            operation: "エントリの解決",
            message: err.to_string(),
        })
    }
}

impl Default for KeyringSecretStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretStore for KeyringSecretStore {
    fn get(&self, key: &str) -> CoreResult<Option<String>> {
        match self.entry(key)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            // 未登録は失敗ではない。呼び出し側は「まだ入れていない」と扱う。
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(CoreError::SecretStore {
                operation: "取得",
                message: err.to_string(),
            }),
        }
    }

    fn set(&self, key: &str, secret: &str) -> CoreResult<()> {
        self.entry(key)?
            .set_password(secret)
            .map_err(|err| CoreError::SecretStore {
                operation: "保存",
                message: err.to_string(),
            })
    }

    fn delete(&self, key: &str) -> CoreResult<()> {
        match self.entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(CoreError::SecretStore {
                operation: "削除",
                message: err.to_string(),
            }),
        }
    }
}

/// プロセス内に閉じた実装。テストと、資格情報ストアが使えない環境の退避先。
#[derive(Default)]
pub struct InMemorySecretStore {
    entries: Mutex<HashMap<String, String>>,
}

impl InMemorySecretStore {
    /// 空のストアを作る。
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretStore for InMemorySecretStore {
    fn get(&self, key: &str) -> CoreResult<Option<String>> {
        Ok(self
            .entries
            .lock()
            .expect("SecretStore のロックが毒された")
            .get(key)
            .cloned())
    }

    fn set(&self, key: &str, secret: &str) -> CoreResult<()> {
        self.entries
            .lock()
            .expect("SecretStore のロックが毒された")
            .insert(key.to_owned(), secret.to_owned());
        Ok(())
    }

    fn delete(&self, key: &str) -> CoreResult<()> {
        self.entries
            .lock()
            .expect("SecretStore のロックが毒された")
            .remove(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_store_round_trips() {
        let store = InMemorySecretStore::new();

        assert_eq!(store.get("tpl").unwrap(), None);
        assert!(!store.contains("tpl").unwrap());

        store.set("tpl", "secret-value").unwrap();
        assert_eq!(store.get("tpl").unwrap().as_deref(), Some("secret-value"));
        assert!(store.contains("tpl").unwrap());

        store.set("tpl", "replaced").unwrap();
        assert_eq!(store.get("tpl").unwrap().as_deref(), Some("replaced"));

        store.delete("tpl").unwrap();
        assert_eq!(store.get("tpl").unwrap(), None);
    }

    #[test]
    fn deleting_a_missing_entry_succeeds() {
        // 削除は冪等。存在しないことを失敗にすると、UI 側が
        // 「消えているのにエラーが出る」不可解な状態になる。
        let store = InMemorySecretStore::new();
        assert!(store.delete("never-existed").is_ok());
    }
}
