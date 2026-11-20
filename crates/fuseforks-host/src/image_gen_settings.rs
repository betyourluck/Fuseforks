//! 画像生成の設定（Spec 76 D1 / `image_gen_contract` 不変条件 1・2）。**`jev.json` と同じ第 3 の棚。**
//!
//! **`{app_data_dir}/image_gen.json` に置く理由は `jev.json` と同じ** — 村を配ったとき、
//! **受け取った人の村が、その人の知らない送信先へプロンプトと画像を送る状態を作らない**。
//! 鍵だけは資格情報ストア（接続先ごとに 1 つ）で、値を返す IPC は作らない。
//!
//! **この層が持つのは設定の読み書きと、差し込み口（`ImageGenSlot`）を埋めるかの判定だけ。**
//! 生成そのものはコア（`fuseforks_core::image_gen` / `fuseforks_core::tools::image`）が持つ。
//!
//! # 「いま効いているか」は 1 実装
//!
//! [`status`] の 1 本に閉じ、偽の理由は閉じた 3 つ（[`InactiveReason`]）。ツールの提示（差し込み口が
//! 空か）・設定画面・起動時のログが同じ述語を読む。**接続確認の結果は入れない**（確認は押したとき
//! だけ外へ出る情報で、門ではない）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use fuseforks_core::image_gen::{ImageAspect, ImageGenConfig, ImageProvider, ImageResolution};
use fuseforks_core::secret::SecretStore;
use fuseforks_core::tools::image::{ActiveImageGen, ImageGenSlot};

/// 設定ファイルの名前（`{app_data_dir}/image_gen.json`）。
pub const CONFIG_FILE: &str = "image_gen.json";

/// 接続先ごとの鍵の名前（OS の資格情報ストア）。**モデルのテンプレート id と衝突しない名前。**
pub const KEY_NAMES: [&str; 4] = [
    "image_gen_key_openai",
    "image_gen_key_gemini",
    "image_gen_key_meta",
    "image_gen_key_xai",
];

/// 接続先の鍵の名前。
#[must_use]
pub fn key_name(provider: ImageProvider) -> &'static str {
    match provider {
        ImageProvider::Openai => KEY_NAMES[0],
        ImageProvider::Gemini => KEY_NAMES[1],
        ImageProvider::Meta => KEY_NAMES[2],
        ImageProvider::Xai => KEY_NAMES[3],
    }
}

/// 接続先ごとの枠。**切り替えても他の枠の値は残る**（Lorekeel の `perProvider` と同じ）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSlot {
    /// モデル。空なら既定。
    #[serde(default)]
    pub model: String,
    /// 接続先。空なら既定。
    #[serde(default)]
    pub base_url: String,
    /// タイムアウト（秒）。`None` なら既定。
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

/// 画像生成の設定。`{app_data_dir}/image_gen.json` の中身そのもの。**鍵はここに入らない。**
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageGenSettingsConfig {
    /// 選んでいる接続先。`None` = 未選択（画像生成は効かない）。**既定は未選択。**
    #[serde(default)]
    pub provider: Option<ImageProvider>,
    /// 接続先ごとの枠。
    #[serde(default)]
    pub per_provider: BTreeMap<ImageProvider, ProviderSlot>,
    /// 縦横比の既定（ツールの引数が省かれたとき）。
    #[serde(default)]
    pub aspect: ImageAspect,
    /// 解像度の段（**モデルに選ばせない**。Meta には効かない）。
    #[serde(default)]
    pub resolution: ImageResolution,
}

impl ImageGenSettingsConfig {
    /// 接続先の枠から 1 回の生成に使う設定を組む。
    #[must_use]
    pub fn config_for(&self, provider: ImageProvider) -> ImageGenConfig {
        let slot = self.per_provider.get(&provider).cloned().unwrap_or_default();
        ImageGenConfig {
            provider,
            model: slot.model.trim().to_owned(),
            base_url: slot.base_url.trim().to_owned(),
            timeout_secs: slot.timeout_secs,
            resolution: self.resolution,
        }
    }
}

/// 設定の読み書き。**`JevSettingsStore` と同じ作法** — 読めないあいだは書き込みを拒む
/// （既定値を書き戻すと選んだ接続先と枠を消す。`failures.md` #70）。
#[derive(Debug)]
pub struct ImageGenSettingsStore {
    path: PathBuf,
    /// 読み込みに失敗した理由。`Some` の間は書き込みを拒み、画像生成も効かない。
    blocked: Option<String>,
    config: ImageGenSettingsConfig,
}

impl ImageGenSettingsStore {
    /// 設定を読み込む。**ファイルが無いのは失敗ではない**（既定で未選択）。それ以外の読み取りエラーは
    /// 解析の失敗と同じく「読めない」（Spec 77 #5。`settings_file::load`）。
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(CONFIG_FILE);
        match crate::settings_file::load::<ImageGenSettingsConfig>(&path) {
            Ok(config) => Self {
                path,
                blocked: None,
                config: config.unwrap_or_default(),
            },
            Err(err) => {
                // 黙って未選択にしない — 失われたことは画面とログの両方に出る。
                fuseforks_core::note!(
                    "image gen: {CONFIG_FILE} を読めませんでした。画像生成は効かず、設定の保存も拒みます（{err}）"
                );
                Self {
                    path,
                    blocked: Some(err),
                    config: ImageGenSettingsConfig::default(),
                }
            }
        }
    }

    /// 現在の設定。
    pub fn config(&self) -> &ImageGenSettingsConfig {
        &self.config
    }

    /// 読み込みに失敗した理由（`Some` の間は保存できない）。
    pub fn blocked(&self) -> Option<&str> {
        self.blocked.as_deref()
    }

    /// 設定を差し替えて保存する。
    ///
    /// # Errors
    /// 読み込みに失敗している間、または書き込みに失敗した場合。
    pub fn save(&mut self, config: ImageGenSettingsConfig) -> Result<(), String> {
        if let Some(reason) = &self.blocked {
            return Err(format!(
                "{CONFIG_FILE} が読めないため保存できません。ファイルを直すか削除してください（{reason}）"
            ));
        }
        let raw = serde_json::to_string_pretty(&config).map_err(|err| err.to_string())?;
        crate::settings_file::write_atomic(&self.path, raw.as_bytes()).map_err(|err| err.to_string())?;
        self.config = config;
        Ok(())
    }
}

/// 画像生成が効いていない理由（閉じた 3 つ。契約の不変条件 2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InactiveReason {
    /// 設定ファイルが読めない。
    ConfigUnreadable,
    /// 接続先が選ばれていない。
    NoProvider,
    /// 選んだ接続先の鍵が登録されていない。
    NoKey,
}

impl InactiveReason {
    /// ログの語。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ConfigUnreadable => "config_unreadable",
            Self::NoProvider => "no_provider",
            Self::NoKey => "no_key",
        }
    }
}

/// 登録済みの鍵。**空白だけは未登録として扱う**（貼り付け事故の吸収 — Jev と同じ）。
#[must_use]
pub fn stored_key(secrets: &dyn SecretStore, provider: ImageProvider) -> Option<String> {
    secrets
        .get(key_name(provider))
        .ok()
        .flatten()
        .map(|k| k.trim().to_owned())
        .filter(|k| !k.is_empty())
}

/// **いま効いているか。** 起動時・保存後・画面が読む 1 実装。真なら選んでいる接続先。
///
/// # Errors
/// 効いていない理由（閉じた 3 つ）。
pub fn status(
    config: &ImageGenSettingsConfig,
    blocked: bool,
    has_key: impl Fn(ImageProvider) -> bool,
) -> Result<ImageProvider, InactiveReason> {
    if blocked {
        return Err(InactiveReason::ConfigUnreadable);
    }
    let provider = config.provider.ok_or(InactiveReason::NoProvider)?;
    if !has_key(provider) {
        return Err(InactiveReason::NoKey);
    }
    Ok(provider)
}

/// 設定と鍵から差し込み口を埋める（または空ける）。**ここで外へは 1 バイトも出ない。**
///
/// 呼ぶのは起動時と、設定・鍵を変えた直後だけ。
///
/// # Errors
/// 効いていない理由（差し込み口は空けてある）。
pub fn apply(
    slot: &ImageGenSlot,
    config: &ImageGenSettingsConfig,
    blocked: bool,
    secrets: &dyn SecretStore,
) -> Result<ImageProvider, InactiveReason> {
    let decided = status(config, blocked, |p| stored_key(secrets, p).is_some());
    let active = decided.ok().and_then(|provider| {
        stored_key(secrets, provider).map(|key| ActiveImageGen {
            config: config.config_for(provider),
            key,
            default_aspect: config.aspect,
        })
    });
    slot.set(active);
    decided
}

/// [`apply`] して、結果を起動時と同じ 1 行でログへ書く。**ログの書式はここだけ**（起動・設定の保存・
/// 鍵の登録と削除が同じ行を出す — P4 で「押した後に効いたか」をログで読むため）。
pub fn apply_and_note(slot: &ImageGenSlot, store: &ImageGenSettingsStore, secrets: &dyn SecretStore) {
    let decided = apply(slot, store.config(), store.blocked().is_some(), secrets);
    fuseforks_core::note!(
        "image gen: active={} provider={} reason={}",
        decided.is_ok(),
        store.config().provider.map_or("-", |p| p.as_str()),
        decided.err().map_or("-", |r| r.as_str())
    );
}

/// 画面へ返す接続先 1 つぶん。**既定値はここから引く**（画面に同じ表を書かない）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    /// 接続先。
    pub id: ImageProvider,
    /// 保存してある枠（空の欄は既定）。
    pub slot: ProviderSlot,
    /// 鍵が登録済みか。**値は返さない。**
    pub has_key: bool,
    /// 既定のモデル（欄が空のときの placeholder）。
    pub default_model: &'static str,
    /// 既定の接続先。
    pub default_base_url: &'static str,
    /// 既定のタイムアウト（秒）。
    pub default_timeout_secs: u64,
    /// 解像度の段が効くか（Meta は偽 — 画面は解像度の欄を押せなくする）。
    pub honors_resolution: bool,
}

/// 画面へ返す状態。**鍵の値は返さない**（`hasKey` だけ）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageGenSettingsView {
    /// 選んでいる接続先（`null` = 使わない）。
    pub provider: Option<ImageProvider>,
    /// 縦横比の既定。
    pub aspect: ImageAspect,
    /// 解像度の段。
    pub resolution: ImageResolution,
    /// 4 社の行（`ImageProvider::ALL` の順）。
    pub providers: Vec<ProviderView>,
    /// **いま効いているか**（[`status`] の 1 実装から）。
    pub active: bool,
    /// 効いていない理由（閉じた 3 つ）。
    pub inactive_reason: Option<InactiveReason>,
    /// 設定ファイルが読めない理由（`null` 以外の間は保存できない）。
    pub blocked: Option<String>,
}

/// 画面へ返す状態を、棚と資格情報ストアから組む。**判定は [`status`] から引く。**
#[must_use]
pub fn view(store: &ImageGenSettingsStore, secrets: &dyn SecretStore) -> ImageGenSettingsView {
    let config = store.config();
    let has_key = |provider| stored_key(secrets, provider).is_some();
    let decided = status(config, store.blocked().is_some(), has_key);
    ImageGenSettingsView {
        provider: config.provider,
        aspect: config.aspect,
        resolution: config.resolution,
        providers: ImageProvider::ALL
            .iter()
            .map(|&id| ProviderView {
                id,
                slot: config.per_provider.get(&id).cloned().unwrap_or_default(),
                has_key: has_key(id),
                default_model: id.default_model(),
                default_base_url: id.default_base_url(),
                default_timeout_secs: id.default_timeout_secs(),
                honors_resolution: id.honors_resolution(),
            })
            .collect(),
        active: decided.is_ok(),
        inactive_reason: decided.err(),
        blocked: store.blocked().map(str::to_owned),
    }
}

/// 接続を確かめた結果。**`outcome` は閉じた集合**（`ok` / 8 区分の語 / `no_provider` / `no_key`）で、
/// 画面は語から辞書の鍵を組む。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageGenProbeView {
    /// 確かめた接続先（未選択なら `null`）。
    pub provider: Option<ImageProvider>,
    /// 結末の語。
    pub outcome: &'static str,
    /// 往復の実測（ms。送らなかったときは `null`）。
    pub elapsed_ms: Option<u64>,
    /// 状態コード（`api` のときだけ）。
    pub status: Option<u16>,
    /// 接続先が返した理由の要点（本文の先頭）。**`auth` には付かない** — OpenAI は 401 の本文に
    /// 鍵の末尾を返す（Spec 68 P0）。
    pub detail: Option<String>,
}

/// 接続を確かめる（画像を作らない GET を 1 本。**押したときだけ**呼ぶ）。確かめるのは**保存済みの**
/// 選んでいる接続先と、登録済みの鍵（Spec 68 と同じ — 下書きは確かめない）。
///
/// ログに `image gen probe:` を 1 行（本文も鍵も書かない）。
pub async fn probe(config: &ImageGenSettingsConfig, secrets: &dyn SecretStore) -> ImageGenProbeView {
    let view = probe_inner(config, secrets).await;
    fuseforks_core::note!(
        "image gen probe: provider={} outcome={} elapsed_ms={} status={}",
        view.provider.map_or("-", |p| p.as_str()),
        view.outcome,
        view.elapsed_ms.map_or_else(|| "-".to_owned(), |ms| ms.to_string()),
        view.status.map_or_else(|| "-".to_owned(), |s| s.to_string()),
    );
    view
}

async fn probe_inner(config: &ImageGenSettingsConfig, secrets: &dyn SecretStore) -> ImageGenProbeView {
    let refused = |provider, reason: InactiveReason| ImageGenProbeView {
        provider,
        outcome: reason.as_str(),
        elapsed_ms: None,
        status: None,
        detail: None,
    };
    let Some(provider) = config.provider else {
        return refused(None, InactiveReason::NoProvider);
    };
    let Some(key) = stored_key(secrets, provider) else {
        return refused(Some(provider), InactiveReason::NoKey);
    };
    let http = reqwest::Client::new();
    let started = std::time::Instant::now();
    let result = fuseforks_core::image_gen::probe(&http, &config.config_for(provider), &key).await;
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    match result {
        Ok(rtt) => ImageGenProbeView {
            provider: Some(provider),
            outcome: "ok",
            elapsed_ms: Some(u64::try_from(rtt.as_millis()).unwrap_or(u64::MAX)),
            status: None,
            detail: None,
        },
        Err(failure) => probe_failure_view(provider, &failure, elapsed_ms),
    }
}

/// 失敗を画面の形へ。**状態コードと本文の先頭を分けて運ぶ**（画面は語で文を選び、本文は添えるだけ）。
fn probe_failure_view(
    provider: ImageProvider,
    failure: &fuseforks_core::image_gen::ImageGenFailure,
    elapsed_ms: u64,
) -> ImageGenProbeView {
    use fuseforks_core::image_gen::ImageGenFailure as F;
    let (status, detail) = match failure {
        F::Auth | F::Timeout => (None, None),
        F::Api { status, detail } => (Some(*status), Some(detail.clone())),
        F::RateLimited { detail }
        | F::Network { detail }
        | F::Shape { detail }
        | F::Config { detail } => (None, Some(detail.clone())),
        F::Blocked { reason } => (None, Some(reason.clone())),
    };
    ImageGenProbeView {
        provider: Some(provider),
        outcome: failure.as_str(),
        elapsed_ms: Some(elapsed_ms),
        status,
        detail: detail.filter(|d| !d.is_empty()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuseforks_core::secret::InMemorySecretStore;

    fn dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!("ff-image-gen-{}", uuid_like()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn uuid_like() -> String {
        format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
    }

    #[test]
    fn status_has_three_closed_reasons() {
        let mut config = ImageGenSettingsConfig::default();
        assert_eq!(status(&config, true, |_| true), Err(InactiveReason::ConfigUnreadable));
        assert_eq!(status(&config, false, |_| true), Err(InactiveReason::NoProvider));
        config.provider = Some(ImageProvider::Xai);
        assert_eq!(status(&config, false, |_| false), Err(InactiveReason::NoKey));
        assert_eq!(status(&config, false, |p| p == ImageProvider::Xai), Ok(ImageProvider::Xai));
        // 鍵は選んでいる接続先のものだけを見る。
        assert_eq!(status(&config, false, |p| p == ImageProvider::Openai), Err(InactiveReason::NoKey));
    }

    #[test]
    fn apply_fills_and_clears_the_slot() {
        let slot = ImageGenSlot::new();
        let secrets = InMemorySecretStore::default();
        let mut config = ImageGenSettingsConfig {
            provider: Some(ImageProvider::Gemini),
            ..ImageGenSettingsConfig::default()
        };
        config.per_provider.insert(
            ImageProvider::Gemini,
            ProviderSlot {
                model: " gemini-x ".to_owned(),
                ..ProviderSlot::default()
            },
        );
        assert_eq!(apply(&slot, &config, false, &secrets), Err(InactiveReason::NoKey));
        assert!(slot.current().is_none());
        secrets.set(key_name(ImageProvider::Gemini), " k ").unwrap();
        assert_eq!(apply(&slot, &config, false, &secrets), Ok(ImageProvider::Gemini));
        let active = slot.current().unwrap();
        assert_eq!(active.key, "k");
        assert_eq!(active.config.model_or_default(), "gemini-x");
        assert_eq!(apply(&slot, &config, true, &secrets), Err(InactiveReason::ConfigUnreadable));
        assert!(slot.current().is_none(), "読めない設定では空ける");
    }

    #[test]
    fn an_unreadable_file_blocks_saving() {
        let dir = dir();
        std::fs::write(dir.join(CONFIG_FILE), "{not json").unwrap();
        let mut store = ImageGenSettingsStore::load(&dir);
        assert!(store.blocked().is_some());
        assert!(store.save(ImageGenSettingsConfig::default()).is_err());
        assert_eq!(std::fs::read_to_string(dir.join(CONFIG_FILE)).unwrap(), "{not json", "書き戻さない");
    }

    #[test]
    fn slots_survive_switching_providers() {
        let dir = dir();
        let mut store = ImageGenSettingsStore::load(&dir);
        assert!(store.blocked().is_none());
        let mut config = ImageGenSettingsConfig {
            provider: Some(ImageProvider::Openai),
            ..ImageGenSettingsConfig::default()
        };
        config.per_provider.insert(
            ImageProvider::Openai,
            ProviderSlot {
                model: "gpt-image-1".to_owned(),
                ..ProviderSlot::default()
            },
        );
        store.save(config.clone()).unwrap();
        config.provider = Some(ImageProvider::Meta);
        store.save(config).unwrap();
        let reloaded = ImageGenSettingsStore::load(&dir);
        assert_eq!(reloaded.config().provider, Some(ImageProvider::Meta));
        assert_eq!(
            reloaded.config().per_provider[&ImageProvider::Openai].model,
            "gpt-image-1"
        );
        let raw = std::fs::read_to_string(dir.join(CONFIG_FILE)).unwrap();
        assert!(raw.contains("\"perProvider\""), "{raw}");
        assert!(raw.contains("\"openai\""), "{raw}");
    }

    #[test]
    fn the_view_carries_four_rows_and_never_the_key() {
        let dir = dir();
        let secrets = InMemorySecretStore::default();
        secrets.set(key_name(ImageProvider::Meta), "sk-SECRET-VALUE").unwrap();
        let store = ImageGenSettingsStore::load(&dir);
        let view = view(&store, &secrets);
        assert!(!view.active);
        assert_eq!(view.inactive_reason, Some(InactiveReason::NoProvider));
        let ids: Vec<_> = view.providers.iter().map(|p| p.id).collect();
        assert_eq!(ids, ImageProvider::ALL.to_vec(), "並びは ALL のまま");
        let meta = &view.providers[2];
        assert!(meta.has_key);
        assert!(!meta.honors_resolution, "Meta は解像度が効かない");
        assert_eq!(meta.default_timeout_secs, 120);
        assert!(view.providers.iter().filter(|p| p.id != ImageProvider::Meta).all(|p| !p.has_key));

        let raw = serde_json::to_string(&view).unwrap();
        assert!(!raw.contains("sk-SECRET-VALUE"), "鍵の値は画面へ出さない: {raw}");
        for key in ["\"inactiveReason\":\"no_provider\"", "\"hasKey\":true", "\"honorsResolution\":false", "\"defaultModel\":\"muse-image-1.0\""] {
            assert!(raw.contains(key), "{key} が無い: {raw}");
        }
    }

    #[test]
    fn the_config_the_screen_sends_round_trips() {
        // 画面（`types.ts` の `ImageGenSettingsConfig`）が送る形そのもの。
        let raw = r#"{"provider":"xai","perProvider":{"xai":{"model":"grok-x","baseUrl":"","timeoutSecs":null},"meta":{"model":"","baseUrl":"","timeoutSecs":30}},"aspect":"landscape","resolution":"high"}"#;
        let config: ImageGenSettingsConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(config.provider, Some(ImageProvider::Xai));
        assert_eq!(config.aspect, ImageAspect::Landscape);
        assert_eq!(config.resolution, ImageResolution::High);
        assert_eq!(config.per_provider[&ImageProvider::Meta].timeout_secs, Some(30));
        assert_eq!(config.config_for(ImageProvider::Xai).model_or_default(), "grok-x");
        // `null` = 使わない。
        let none: ImageGenSettingsConfig = serde_json::from_str(r#"{"provider":null}"#).unwrap();
        assert_eq!(none.provider, None);
    }

    #[tokio::test]
    async fn the_probe_refuses_without_sending() {
        let secrets = InMemorySecretStore::default();
        let mut config = ImageGenSettingsConfig::default();
        let none = probe(&config, &secrets).await;
        assert_eq!((none.provider, none.outcome, none.elapsed_ms), (None, "no_provider", None));
        // 接続先を選んでいても鍵が無ければ送らない（`elapsed_ms` が無い = 往復していない）。
        config.provider = Some(ImageProvider::Gemini);
        let no_key = probe(&config, &secrets).await;
        assert_eq!(
            (no_key.provider, no_key.outcome, no_key.elapsed_ms),
            (Some(ImageProvider::Gemini), "no_key", None)
        );
    }

    #[test]
    fn auth_failures_carry_no_body() {
        use fuseforks_core::image_gen::ImageGenFailure as F;
        let auth = probe_failure_view(ImageProvider::Openai, &F::Auth, 12);
        assert_eq!((auth.outcome, auth.status, auth.detail), ("auth", None, None));
        let api = probe_failure_view(
            ImageProvider::Xai,
            &F::Api { status: 500, detail: "boom".to_owned() },
            9,
        );
        assert_eq!((api.outcome, api.status, api.detail.as_deref()), ("api", Some(500), Some("boom")));
        let config = probe_failure_view(
            ImageProvider::Gemini,
            &F::Config { detail: String::new() },
            9,
        );
        assert_eq!(config.detail, None, "空の本文は運ばない");
    }

    #[test]
    fn key_names_do_not_look_like_template_ids() {
        for name in KEY_NAMES {
            assert!(name.starts_with("image_gen_key_"));
        }
        assert_eq!(key_name(ImageProvider::Xai), "image_gen_key_xai");
    }
}
