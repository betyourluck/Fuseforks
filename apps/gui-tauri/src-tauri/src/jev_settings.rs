//! ツール結果の即時圧縮の設定（Spec 59 P2）。**`pricing_source.rs` と同じ第 3 の棚。**
//!
//! **`{app_data_dir}/jev.json` に置く理由は `pricing.json` と同じ** — 村を配ったとき、
//! **受け取った人の村が、その人の知らない送信先へツール結果を送る状態を作らない**。
//! API トークンだけは資格情報ストア（鍵 [`TOKEN_KEY`]）で、値を返す IPC は作らない。
//!
//! **この層が持つのは設定の読み書きと、採点器を差し込むかの判定だけ。**
//! 圧縮そのものはコア（`fuseforks_core::prune` / `fuseforks_core::jev`）が持つ。
//!
//! # 差し込みの規則は 1 実装
//!
//! 「いま圧縮が掛かっているか」は 4 つの条件の積（設定が ON / Account ID がある /
//! トークンがある / 設定ファイルが読める）で、これを [`is_active`] の 1 本に閉じた。
//! 起動時の差し込み・設定変更後の差し込み・画面のチェックの `disabled`・画面の
//! 「いま有効か」の 4 箇所が同じ述語を読む。**2 箇所に書くと「画面では ON なのに
//! 掛かっていない」が作れる**（Spec 20 の提示集合と判定集合がずれた形）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use fuseforks_core::jev::{JevConfig, JevScorer, DEFAULT_BASE_URL, DEFAULT_MODEL};
use fuseforks_core::prune::ParagraphScorer;
use fuseforks_core::secret::SecretStore;
use fuseforks_core::Orchestrator;

/// 設定ファイルの名前（`{app_data_dir}/jev.json`）。
pub const CONFIG_FILE: &str = "jev.json";

/// API トークンの鍵（OS の資格情報ストア）。
///
/// **モデルのテンプレート id と衝突しない名前**にする — 同じストアを共有しており、
/// テンプレート id は `tpl_...` 形式だが、利用者が手で作れば何でも入りうる。
pub const TOKEN_KEY: &str = "jev_api_token";

/// 閾値の離散 4 値（`tool_prune_contract`）。**この順で画面に並ぶ。**
pub const THRESHOLDS: [f32; 4] = [0.1, 0.2, 0.3, 0.5];

/// 既定の閾値。**P0 の要否判定で確定した**（帯 0.15〜0.28 の 46 段落で、
/// 合計の誤りが最小なのは 0.22 だが、誤って落とす側のコストが高いので 0.2）。
pub const DEFAULT_THRESHOLD: f32 = 0.2;

/// 集合に無い値は既定へ落とす（`theme` / 表示倍率と同じ）。
///
/// 手で `jev.json` を書き換えた村でも、**連続量が入り込まない**。閾値を範囲で
/// 受けると「0.2 と 0.21 の違いに意味がある」という読みを作ってしまう。
#[must_use]
pub fn normalize_threshold(value: f32) -> f32 {
    THRESHOLDS
        .iter()
        .copied()
        .find(|t| (t - value).abs() < 1e-6)
        .unwrap_or(DEFAULT_THRESHOLD)
}

/// 圧縮の設定。`{app_data_dir}/jev.json` の中身そのもの。
///
/// **トークンはここに入らない**（資格情報ストア）。Account ID は秘密ではないので
/// 画面へそのまま返す — 見て変えられることが「自分で設定した送信先」の根拠になる。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JevSettingsConfig {
    /// ツール結果を圧縮するか。**既定は OFF。**
    #[serde(default)]
    pub enabled: bool,
    /// Cloudflare のアカウント ID。空なら圧縮は掛からない。
    #[serde(default)]
    pub account_id: String,
    /// 閾値。読み込み時に [`normalize_threshold`] を通す。
    #[serde(default = "default_threshold")]
    pub threshold: f32,
}

fn default_threshold() -> f32 {
    DEFAULT_THRESHOLD
}

impl Default for JevSettingsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            account_id: String::new(),
            threshold: DEFAULT_THRESHOLD,
        }
    }
}

/// 設定の読み書き。**`McpServerStore` / `PricingSourceStore` と同じ作法** —
/// 読めないあいだは書き込みを拒む（既定値を書き戻すと Account ID と閾値を消す。
/// `failures.md` #70）。
#[derive(Debug)]
pub struct JevSettingsStore {
    path: PathBuf,
    /// 読み込みに失敗した理由。`Some` の間は書き込みを拒み、圧縮も掛けない。
    blocked: Option<String>,
    config: JevSettingsConfig,
}

impl JevSettingsStore {
    /// 設定を読み込む。**ファイルが無いのは失敗ではない**（既定で OFF）。
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(CONFIG_FILE);
        match std::fs::read_to_string(&path) {
            Ok(raw) => match serde_json::from_str::<JevSettingsConfig>(&raw) {
                Ok(mut config) => {
                    config.threshold = normalize_threshold(config.threshold);
                    Self {
                        path,
                        blocked: None,
                        config,
                    }
                }
                Err(err) => {
                    // **黙って OFF にしない**（D9）。利用者の意図は失われるが、
                    // 失われたことは画面（`blocked`）とログの両方に出る。
                    fuseforks_core::note!(
                        "jev: {CONFIG_FILE} を読めませんでした。圧縮は掛からず、設定の保存も拒みます（{err}）"
                    );
                    Self {
                        path,
                        blocked: Some(err.to_string()),
                        config: JevSettingsConfig::default(),
                    }
                }
            },
            // 未作成。初回はこれが正常。
            Err(_) => Self {
                path,
                blocked: None,
                config: JevSettingsConfig::default(),
            },
        }
    }

    /// 現在の設定。
    pub fn config(&self) -> &JevSettingsConfig {
        &self.config
    }

    /// 読み込みに失敗した理由（`Some` の間は保存できない）。
    pub fn blocked(&self) -> Option<&str> {
        self.blocked.as_deref()
    }

    /// 設定を差し替えて保存する。**閾値は集合へ丸めてから書く。**
    ///
    /// # Errors
    /// 読み込みに失敗している間、または書き込みに失敗した場合。
    pub fn save(&mut self, mut config: JevSettingsConfig) -> Result<(), String> {
        if let Some(reason) = &self.blocked {
            return Err(format!(
                "{CONFIG_FILE} が読めないため保存できません。ファイルを直すか削除してください（{reason}）"
            ));
        }
        config.threshold = normalize_threshold(config.threshold);
        config.account_id = config.account_id.trim().to_owned();
        let raw = serde_json::to_string_pretty(&config).map_err(|err| err.to_string())?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        std::fs::write(&self.path, raw).map_err(|err| err.to_string())?;
        self.config = config;
        Ok(())
    }
}

/// 圧縮を**掛けられる**状態か（ON かどうかは見ない）。
///
/// 画面のチェックの `disabled` はこの否定。**「ON にできない理由」と「ON にして
/// いない」を分ける** — 畳むと、キーを入れ忘れた人には「チェックが効かない」と
/// しか見えない。
#[must_use]
pub fn can_enable(config: &JevSettingsConfig, has_token: bool, blocked: bool) -> bool {
    !blocked && !config.account_id.trim().is_empty() && has_token
}

/// **いま圧縮が掛かっているか。** 起動時・設定変更後・画面の 3 者が読む 1 実装。
#[must_use]
pub fn is_active(config: &JevSettingsConfig, has_token: bool, blocked: bool) -> bool {
    config.enabled && can_enable(config, has_token, blocked)
}

/// 登録済みのトークン。**空白だけは未登録として扱う**（貼り付け事故の吸収）。
#[must_use]
pub fn stored_token(secrets: &dyn SecretStore) -> Option<String> {
    secrets
        .get(TOKEN_KEY)
        .ok()
        .flatten()
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
}

/// 設定と鍵から採点器を組む（**差し込まない**）。
///
/// `apply` と「接続を確かめる」が共有する 1 実装。ここが 2 つに割れると、
/// 確認だけ通って本番が別の設定で動く形が作れる。
///
/// # Errors
///
/// Account ID かトークンが無い、または HTTP クライアントを作れないとき。
pub fn scorer_for(
    config: &JevSettingsConfig,
    secrets: &dyn SecretStore,
) -> Result<JevScorer, String> {
    let account_id = config.account_id.trim();
    if account_id.is_empty() {
        return Err("Account ID が設定されていません".to_owned());
    }
    let Some(api_token) = stored_token(secrets) else {
        return Err("API トークンが登録されていません".to_owned());
    };
    JevScorer::new(JevConfig {
        account_id: account_id.to_owned(),
        api_token,
        base_url: DEFAULT_BASE_URL.to_owned(),
        model: DEFAULT_MODEL.to_owned(),
        threshold: config.threshold,
    })
}

/// 設定から採点器を組み、オーケストレーターへ差し込む（または外す）。
///
/// **呼ぶのは起動時と、設定・トークンを変えた直後だけ。** 返るのは「掛かったか」で、
/// 画面はこの値をそのまま出す（`enabled` と別に持つのは、`mcp_server` の
/// `enabled` / `listening` と同じ理由 — 設定上の ON と実際に効いているかは別）。
///
/// **ここで外へは 1 バイトも出ない。** 採点器を作るのは HTTP クライアントを
/// 組むだけで、送信が起きるのはツールが 4,000 字以上を返したときから（D10）。
pub async fn apply(
    orchestrator: &Orchestrator,
    config: &JevSettingsConfig,
    blocked: bool,
    secrets: &dyn SecretStore,
) -> bool {
    if !is_active(config, stored_token(secrets).is_some(), blocked) {
        orchestrator.set_paragraph_scorer(None).await;
        return false;
    }
    match scorer_for(config, secrets) {
        Ok(scorer) => {
            orchestrator
                .set_paragraph_scorer(Some(Arc::new(scorer) as Arc<dyn ParagraphScorer>))
                .await;
            true
        }
        Err(reason) => {
            // **失敗したら掛けない**（fail closed 側）。作れない採点器を
            // 差し込むと、ツールが返るたびに同じ失敗を踏んで全文へ落ちる。
            fuseforks_core::note!("jev: 採点器を組めませんでした。圧縮は掛かりません（{reason}）");
            orchestrator.set_paragraph_scorer(None).await;
            false
        }
    }
}

/// 画面へ返す状態を、棚と資格情報ストアから組む。**判定は 1 実装から引く。**
#[must_use]
pub fn view(store: &JevSettingsStore, secrets: &dyn SecretStore) -> JevSettingsView {
    JevSettingsView::new(
        store.config(),
        stored_token(secrets).is_some(),
        store.blocked().map(str::to_owned),
    )
}

/// 画面へ返す状態。**トークンの値は返さない**（`hasToken` だけ）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JevSettingsView {
    /// 設定上の ON / OFF。
    pub enabled: bool,
    /// Cloudflare のアカウント ID。**秘密ではない。**
    pub account_id: String,
    /// 閾値（集合に丸めた後の値）。
    pub threshold: f32,
    /// トークンが登録済みか。**値は返さない。**
    pub has_token: bool,
    /// ON にできる状態か（チェックの `disabled` はこの否定）。
    pub can_enable: bool,
    /// **いま実際に掛かっているか。** `enabled` と別に持つ。
    pub active: bool,
    /// 設定ファイルが読めない理由（`null` 以外の間は保存できない）。
    pub blocked: Option<String>,
}

impl JevSettingsView {
    /// 設定とトークンの有無から組む。**画面が同じ判定を書かないための 1 実装。**
    #[must_use]
    pub fn new(config: &JevSettingsConfig, has_token: bool, blocked: Option<String>) -> Self {
        let is_blocked = blocked.is_some();
        Self {
            enabled: config.enabled,
            account_id: config.account_id.clone(),
            threshold: config.threshold,
            has_token,
            can_enable: can_enable(config, has_token, is_blocked),
            active: is_active(config, has_token, is_blocked),
            blocked,
        }
    }
}

/// 接続を確かめた結果（画面へそのまま出す）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JevProbeView {
    /// サーバーが名乗ったモデル版。
    pub model: String,
    /// 往復の実測（ms）。
    pub elapsed_ms: u64,
    /// 依頼の核として送った段落の点。
    pub relevant: Option<f32>,
    /// 定型文として送った段落の点。
    pub boilerplate: Option<f32>,
}

impl From<fuseforks_core::jev::JevProbe> for JevProbeView {
    fn from(p: fuseforks_core::jev::JevProbe) -> Self {
        Self {
            model: p.model,
            elapsed_ms: p.elapsed_ms,
            relevant: p.relevant,
            boilerplate: p.boilerplate,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_id() -> JevSettingsConfig {
        JevSettingsConfig {
            enabled: true,
            account_id: "abc123".into(),
            threshold: DEFAULT_THRESHOLD,
        }
    }

    /// 集合に無い値は既定へ落ちる。**範囲で受けない**（連続量にしない）。
    #[test]
    fn a_threshold_outside_the_set_falls_back_to_the_default() {
        for good in THRESHOLDS {
            assert!((normalize_threshold(good) - good).abs() < 1e-6);
        }
        for bad in [0.0_f32, 0.15, 0.22, 0.9, -1.0, f32::NAN] {
            assert!((normalize_threshold(bad) - DEFAULT_THRESHOLD).abs() < 1e-6, "{bad}");
        }
    }

    /// **既定は 0.2。** P0 の要否判定で確定した値なので、定数を動かすならもう一度測る。
    #[test]
    fn the_default_threshold_is_the_measured_one() {
        assert!((DEFAULT_THRESHOLD - 0.2).abs() < 1e-6);
        assert!(THRESHOLDS.contains(&DEFAULT_THRESHOLD));
    }

    /// ON にできる条件は 3 つの積。**どれが欠けても掛からない。**
    #[test]
    fn enabling_needs_the_id_the_token_and_a_readable_file() {
        assert!(can_enable(&with_id(), true, false));
        assert!(!can_enable(&with_id(), false, false), "トークンが無い");
        assert!(!can_enable(&with_id(), true, true), "設定ファイルが読めない");
        let mut blank = with_id();
        blank.account_id = "   ".into();
        assert!(!can_enable(&blank, true, false), "Account ID が空白だけ");
    }

    /// **`enabled` と「掛かっている」は別。** 条件が揃っていても OFF なら掛からない。
    #[test]
    fn being_able_to_enable_is_not_the_same_as_being_on() {
        let mut off = with_id();
        off.enabled = false;
        assert!(can_enable(&off, true, false));
        assert!(!is_active(&off, true, false));
        assert!(is_active(&with_id(), true, false));
    }

    /// 読めない設定ファイルは**書き込みも拒む**（既定値で Account ID を消さない）。
    #[test]
    fn a_blocked_store_refuses_to_save() {
        let dir = std::env::temp_dir().join(format!("fuseforks-jev-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(CONFIG_FILE), "{ broken").unwrap();

        let mut store = JevSettingsStore::load(&dir);
        assert!(store.blocked().is_some());
        assert_eq!(store.config(), &JevSettingsConfig::default());
        let err = store.save(with_id()).unwrap_err();
        assert!(err.contains(CONFIG_FILE), "{err}");
        // ファイルは 1 バイトも変わっていない。
        assert_eq!(
            std::fs::read_to_string(dir.join(CONFIG_FILE)).unwrap(),
            "{ broken"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 保存は閾値を丸め、Account ID の前後空白を落とす。読み直しても同じ。
    #[test]
    fn saving_normalizes_the_threshold_and_trims_the_id() {
        let dir = std::env::temp_dir().join(format!("fuseforks-jev-ok-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _ = std::fs::remove_file(dir.join(CONFIG_FILE));

        let mut store = JevSettingsStore::load(&dir);
        store
            .save(JevSettingsConfig {
                enabled: true,
                account_id: "  abc123  ".into(),
                threshold: 0.22,
            })
            .unwrap();
        assert_eq!(store.config().account_id, "abc123");
        assert!((store.config().threshold - DEFAULT_THRESHOLD).abs() < 1e-6);

        let reloaded = JevSettingsStore::load(&dir);
        assert_eq!(reloaded.config(), store.config());
        assert!(reloaded.blocked().is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// **トークンはファイルに書かない。** 保存した JSON に鍵の名前も値も出ない。
    #[test]
    fn the_token_never_reaches_the_file() {
        let dir = std::env::temp_dir().join(format!("fuseforks-jev-tok-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _ = std::fs::remove_file(dir.join(CONFIG_FILE));

        let mut store = JevSettingsStore::load(&dir);
        store.save(with_id()).unwrap();
        let raw = std::fs::read_to_string(dir.join(CONFIG_FILE)).unwrap();
        assert!(!raw.contains("token"), "{raw}");
        assert!(!raw.contains(TOKEN_KEY), "{raw}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 画面へ返す状態は、判定を 1 実装から引く（画面側で組み直さない）。
    #[test]
    fn the_view_carries_the_same_verdicts() {
        let view = JevSettingsView::new(&with_id(), true, None);
        assert!(view.can_enable && view.active && view.has_token);
        assert_eq!(view.account_id, "abc123");

        let no_token = JevSettingsView::new(&with_id(), false, None);
        assert!(!no_token.can_enable && !no_token.active);

        let broken = JevSettingsView::new(&with_id(), true, Some("壊れています".into()));
        assert!(!broken.can_enable && !broken.active);
        assert_eq!(broken.blocked.as_deref(), Some("壊れています"));
    }

    /// ワイヤに出る欄を固定する（`types.ts` の `JevSettingsView` / `JevProbeView`）。
    ///
    /// **落ちたら `apps/gui-tauri/src/types.ts` を直すこと。** 型検査は 2 言語の
    /// 境界に届かないので、欄を増減しても TS 側は黙って古いままになる
    /// （`ipc_contract.rs` と同じ形。期待値だけ更新して通すのは意味を消す）。
    #[test]
    fn the_wire_shape_is_frozen() {
        let view = serde_json::to_value(JevSettingsView::new(&with_id(), true, None)).unwrap();
        let mut keys: Vec<&String> = view.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            ["accountId", "active", "blocked", "canEnable", "enabled", "hasToken", "threshold"]
        );
        assert!(
            !view.as_object().unwrap().contains_key("token"),
            "トークンの値はワイヤに出さない"
        );

        let probe = serde_json::to_value(JevProbeView {
            model: "jev-1.13.0".into(),
            elapsed_ms: 812,
            relevant: Some(0.9),
            boilerplate: Some(0.02),
        })
        .unwrap();
        let mut probe_keys: Vec<&String> = probe.as_object().unwrap().keys().collect();
        probe_keys.sort();
        assert_eq!(probe_keys, ["boilerplate", "elapsedMs", "model", "relevant"]);
    }

    /// **起動経路は差し込みだけを呼び、接続の確認へは出ない。**
    ///
    /// `apply` は HTTP クライアントを組むだけで送信しないが、`probe` は実際に
    /// 外へ出る。起動時に呼ぶ実装へ変わったら、押していないのに送信が起きる。
    #[test]
    fn startup_installs_the_scorer_but_never_probes() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/state.rs");
        let src = std::fs::read_to_string(path).expect("state.rs を読めること");
        assert!(
            src.contains("jev_settings::apply"),
            "起動時に採点器を差し込んでいない"
        );
        assert!(
            !src.contains(".probe("),
            "起動経路が接続の確認を呼んでいる（押していないのに外へ出る）"
        );
    }
}
