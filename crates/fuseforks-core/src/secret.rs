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
//! **コンテナには資格情報ストアが無い**（Spec 64 D4）。そこではデプロイ時に注入する
//! 環境変数が正しい置き場なので、[`EnvSecretStore`] を 3 つ目の実装として持つ —
//! 上の「環境変数は合わない」はデスクトップの話で、ヘッドレスでは逆になる。
//! **読み取り専用にしたのは #1 の教訓の側** — 画面から設定したキーが環境変数や
//! 平文のファイルへ流れる経路を作らない。デスクトップの GUI は今までどおり
//! 資格情報ストアだけを使う。
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

/// 環境変数から読む秘密の名前の接頭辞（Spec 64 D4）。
pub const ENV_SECRET_PREFIX: &str = "FUSEFORKS_SECRET_";

/// 鍵 → 環境変数名。`claude_sonnet` → `FUSEFORKS_SECRET_CLAUDE_SONNET`。
///
/// 英数字は大文字化し、それ以外は `_` へ。**2 つの鍵が同じ名前に写りうる**
/// （`a-b` と `a_b`）ので、起動時に [`secret_name_collisions`] で数えてから使う。
pub fn env_secret_name(key: &str) -> String {
    let mut out = String::with_capacity(ENV_SECRET_PREFIX.len() + key.len());
    out.push_str(ENV_SECRET_PREFIX);
    for c in key.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_uppercase());
        } else {
            out.push('_');
        }
    }
    out
}

/// 同じ環境変数名に写ってしまう鍵の組。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretNameCollision {
    /// 衝突した変数名。
    pub variable: String,
    /// その名前に写る鍵（2 つ以上。辞書順）。
    pub keys: Vec<String>,
}

/// 鍵の集合のうち、同じ環境変数名に写る組を返す。空なら衝突なし。
///
/// 数える鍵は呼び出し側が全部渡す（村のテンプレート ID の全部 + コードが持つ固定の鍵）。
/// 衝突があると「どちらの鍵の値か」を決められないので、組み立てを止める材料になる。
pub fn secret_name_collisions<'a>(keys: impl IntoIterator<Item = &'a str>) -> Vec<SecretNameCollision> {
    let mut by_variable: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
        std::collections::BTreeMap::new();
    for key in keys {
        by_variable
            .entry(env_secret_name(key))
            .or_default()
            .insert(key.to_owned());
    }
    by_variable
        .into_iter()
        .filter(|(_, keys)| keys.len() > 1)
        .map(|(variable, keys)| SecretNameCollision {
            variable,
            keys: keys.into_iter().collect(),
        })
        .collect()
}

/// 環境変数から読む実装（Spec 64 D4）。**読み取り専用。**
///
/// 起動時に 1 回だけ `FUSEFORKS_SECRET_` で始まる変数を全部写し、以後は環境を見ない
/// （ターンの途中で値が変わる経路を作らない）。`set` / `delete` はエラーを返す —
/// 画面から設定したキーが環境変数へ流れる経路を作らない（#1 の教訓）。
/// **値をどこにも出さない**のは他の実装と同じ。
pub struct EnvSecretStore {
    /// 変数名 → 値。鍵ではなく変数名で引く（写像は [`env_secret_name`]）。
    entries: HashMap<String, String>,
}

impl EnvSecretStore {
    /// プロセスの環境から写す。UTF-8 でない変数は読まない。
    pub fn from_env() -> Self {
        Self::from_vars(std::env::vars_os().filter_map(|(name, value)| {
            Some((name.into_string().ok()?, value.into_string().ok()?))
        }))
    }

    /// 与えた変数の列から写す（テストと、環境を差し替えたい呼び出し側のため）。
    pub fn from_vars(vars: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            entries: vars
                .into_iter()
                .filter(|(name, _)| name.starts_with(ENV_SECRET_PREFIX))
                .collect(),
        }
    }

    /// 読めた変数の名前だけ（辞書順）。`check` が「どの変数が有るか」を出すために使う。
    /// 値は返さない。
    pub fn variable_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.entries.keys().cloned().collect();
        names.sort();
        names
    }
}

impl SecretStore for EnvSecretStore {
    fn get(&self, key: &str) -> CoreResult<Option<String>> {
        Ok(self.entries.get(&env_secret_name(key)).cloned())
    }

    fn set(&self, key: &str, _secret: &str) -> CoreResult<()> {
        Err(CoreError::SecretStore {
            operation: "保存",
            message: format!(
                "環境変数から読むストアは書き込めません（{} を設定して起動し直してください）",
                env_secret_name(key)
            ),
        })
    }

    fn delete(&self, key: &str) -> CoreResult<()> {
        Err(CoreError::SecretStore {
            operation: "削除",
            message: format!(
                "環境変数から読むストアは削除できません（{} を環境から外して起動し直してください）",
                env_secret_name(key)
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_names_are_prefixed_upper_cased_and_sanitised() {
        assert_eq!(env_secret_name("claude_sonnet"), "FUSEFORKS_SECRET_CLAUDE_SONNET");
        assert_eq!(env_secret_name("jev_api_token"), "FUSEFORKS_SECRET_JEV_API_TOKEN");
        assert_eq!(env_secret_name("gpt-5.6/terra"), "FUSEFORKS_SECRET_GPT_5_6_TERRA");
        assert_eq!(env_secret_name("日本語"), "FUSEFORKS_SECRET____");
    }

    #[test]
    fn the_env_store_reads_only_prefixed_variables_and_maps_keys() {
        let store = EnvSecretStore::from_vars([
            ("FUSEFORKS_SECRET_CLAUDE_SONNET".to_owned(), "sk-1".to_owned()),
            ("PATH".to_owned(), "/usr/bin".to_owned()),
            ("fuseforks_secret_lower".to_owned(), "ignored".to_owned()),
        ]);
        assert_eq!(store.get("claude_sonnet").unwrap().as_deref(), Some("sk-1"));
        assert!(store.contains("claude_sonnet").unwrap());
        assert_eq!(store.get("other").unwrap(), None);
        assert_eq!(
            store.variable_names(),
            vec!["FUSEFORKS_SECRET_CLAUDE_SONNET".to_owned()],
            "接頭辞の無い変数と小文字の接頭辞は読まない"
        );
    }

    /// 書けない。エラー文には**変数名だけ**が載り、値は載らない。
    #[test]
    fn the_env_store_refuses_writes_without_leaking_values() {
        let store = EnvSecretStore::from_vars([(
            "FUSEFORKS_SECRET_TPL".to_owned(),
            "existing-value".to_owned(),
        )]);
        let err = store.set("tpl", "new-secret-value").unwrap_err();
        let text = err.to_string();
        assert!(text.contains("FUSEFORKS_SECRET_TPL"), "{text}");
        assert!(!text.contains("new-secret-value"), "値が漏れている: {text}");
        assert!(!text.contains("existing-value"), "値が漏れている: {text}");
        assert_eq!(
            store.get("tpl").unwrap().as_deref(),
            Some("existing-value"),
            "失敗した set は何も変えない"
        );

        let err = store.delete("tpl").unwrap_err();
        assert!(err.to_string().contains("FUSEFORKS_SECRET_TPL"));
        assert!(store.contains("tpl").unwrap(), "失敗した delete は何も消さない");
    }

    #[test]
    fn collisions_are_counted_per_variable_name() {
        let found = secret_name_collisions(["a-b", "a_b", "jev_api_token", "A.B", "solo"]);
        assert_eq!(
            found,
            vec![SecretNameCollision {
                variable: "FUSEFORKS_SECRET_A_B".to_owned(),
                keys: vec!["A.B".to_owned(), "a-b".to_owned(), "a_b".to_owned()],
            }]
        );
        assert!(secret_name_collisions(["x", "y"]).is_empty());
        assert!(
            secret_name_collisions(["x", "x"]).is_empty(),
            "同じ鍵が 2 回来ても衝突ではない"
        );
    }

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
