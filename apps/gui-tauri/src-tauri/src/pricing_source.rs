//! 単価表の取得元（Spec 41 P2）。**この村で 1 本目の非 LLM 外向き通信。**
//!
//! **設定は `{app_data_dir}/pricing.json`** — `mcp_server.json` と同じ第 3 の棚。
//! **`world.json` に置かない**: 村を配ったとき、**受け取った人の村が、その人の
//! 知らない URL へ取りに行ける状態を作らない**（Spec 25 の「村を配っても扉は
//! 開かない」と同じ線引き）。**単価そのものは `ModelTemplate` に住み、村と
//! 一緒に配られる**（公開情報でテンプレートの属性）。
//!
//! **凍結事項**（`data_contract` の `pricing_fetch_freeze`）:
//! - `interval` / `tokio::spawn` / background task を持たない
//! - 起動時・画面遷移時・フォーカス時の自動 GET を持たない
//! - **URL の到達確認のための自動 GET も持たない**（疎通確認もボタン押下時のみ）
//! - **リモートと比較して「古いか」を判定しない** — 古さは
//!   `ModelTemplate::pricing_as_of` からの**ローカルの経過日数**だけで表示する
//!
//! 最後の 1 行が要。**「古い」を手元の引き算だけで定義すれば、比較のために
//! 外へ出る理由が消える** — これが無いと、実装者は古さを知るために
//! 起動時 `HEAD` を発明する。

use std::path::{Path, PathBuf};
use std::time::Duration;

use fuseforks_core::pricing::ParsedTable;
use serde::{Deserialize, Serialize};

/// 設定ファイルの名前（`{app_data_dir}/pricing.json`）。
pub const CONFIG_FILE: &str = "pricing.json";

/// 取得の待ち時間。**短く固定** — 押して待つ操作なので、長い待ちは失敗より悪い。
const TIMEOUT: Duration = Duration::from_secs(15);

/// 受け取る本文の上限。桁外れの応答でメモリを食わないための衛生。
const MAX_BYTES: usize = 8 * 1024 * 1024;

/// 既定の取得先。
///
/// **開発者が用意した静的ファイル**（GitHub Pages）。`PRIVACY.md` に「開発者が
/// 運営する送信先」の唯一の例外として明記してある。**画面から変更・空にできる**。
///
/// **空にすれば、この村は単価表のために 1 度も外へ出ない**（S4）。
/// **既定が入っていても、押すまでは 1 度も通信しない**（凍結事項）。
const DEFAULT_URL: &str = "https://betyourluck.github.io/prices.json";

/// 取得元の設定。`{app_data_dir}/pricing.json` の中身そのもの。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PricingSourceConfig {
    /// 単価表の URL。**空なら取得しない。**
    pub url: String,
}

impl Default for PricingSourceConfig {
    fn default() -> Self {
        Self {
            url: DEFAULT_URL.to_owned(),
        }
    }
}

/// 設定の読み書き。**`McpServerStore` と同じ作法** — 読めないあいだは書き込みを拒む
/// （既定値を書き戻すと利用者が入れた URL を消す。`failures.md` #70）。
#[derive(Debug)]
pub struct PricingSourceStore {
    path: PathBuf,
    blocked: Option<String>,
    config: PricingSourceConfig,
}

impl PricingSourceStore {
    /// 設定を読み込む。**ファイルが無いのは失敗ではない**（既定で空の URL）。
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(CONFIG_FILE);
        match std::fs::read_to_string(&path) {
            Ok(raw) => match serde_json::from_str::<PricingSourceConfig>(&raw) {
                Ok(config) => Self {
                    path,
                    blocked: None,
                    config,
                },
                Err(err) => {
                    fuseforks_core::note!(
                        "pricing: {CONFIG_FILE} を読めませんでした。取得はできず、設定の保存も拒みます（{err}）"
                    );
                    Self {
                        path,
                        blocked: Some(err.to_string()),
                        config: PricingSourceConfig::default(),
                    }
                }
            },
            Err(_) => Self {
                path,
                blocked: None,
                config: PricingSourceConfig::default(),
            },
        }
    }

    /// 現在の設定。
    pub fn config(&self) -> &PricingSourceConfig {
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
    pub fn save(&mut self, config: PricingSourceConfig) -> Result<(), String> {
        if let Some(reason) = &self.blocked {
            return Err(format!(
                "{CONFIG_FILE} が読めないため保存できません。ファイルを直すか削除してください（{reason}）"
            ));
        }
        let raw = serde_json::to_string_pretty(&config).map_err(|err| err.to_string())?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        std::fs::write(&self.path, raw).map_err(|err| err.to_string())?;
        self.config = config;
        Ok(())
    }
}

/// URL が取得先として使えるかを見る。**`https` のみ。**
///
/// 平文で取る理由が無く、**取ってきた数値がそのまま金額になる**ので、
/// 途中で書き換えられる経路を最初から作らない。
///
/// # Errors
/// 空、または `https://` で始まらないとき。
pub fn validate_url(url: &str) -> Result<(), String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("取得先が設定されていません".to_owned());
    }
    if !url.starts_with("https://") {
        return Err("取得先は https:// で始まる必要があります".to_owned());
    }
    Ok(())
}

/// 単価表を取りに行く。**呼ぶのは利用者がボタンを押したときだけ。**
///
/// **この関数を起動経路・画面遷移・タイマーから呼んではならない**（上の凍結）。
/// 呼び出し元は IPC ハンドラ 1 箇所に限る。
///
/// # Errors
/// URL が不正、通信に失敗、応答が大きすぎる、JSON として読めないとき。
pub async fn fetch_table(url: &str) -> Result<ParsedTable, String> {
    validate_url(url)?;
    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;
    let res = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!("取得先が {} を返しました", res.status()));
    }
    let body = res.bytes().await.map_err(|e| e.to_string())?;
    if body.len() > MAX_BYTES {
        return Err(format!(
            "応答が大きすぎます（{} バイト。上限 {MAX_BYTES}）",
            body.len()
        ));
    }
    let raw = String::from_utf8(body.to_vec()).map_err(|e| e.to_string())?;
    let table = fuseforks_core::pricing::parse_table(&raw)?;
    // `context=` は窓が入った要素の数（Spec 50）。**`as_of` は単価の時点で、窓を足しても
    // 動かない** — `as_of=2026-08-20 context=1493` が同じ行に並ぶのは矛盾ではない。
    fuseforks_core::note!(
        "pricing fetch: entries={} dropped={} context={} as_of={}",
        table.entries.len(),
        table.dropped,
        table.windows(),
        table.as_of.as_deref().unwrap_or("-")
    );
    Ok(table)
}

/// 画面へ返す取得元の状態。**URL はそのまま返す** — 秘密ではなく、
/// **利用者が見て変えられることが「自分で設定した接続先」の根拠**になる。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PricingSourceView {
    /// 取得先。空なら取得しない。
    pub url: String,
    /// 設定ファイルが読めない理由（`Some` の間は保存できない）。
    pub blocked: Option<String>,
}

/// 取得の結果。**画面の欄へ入れるだけで、保存はしない**（Spec 41 D3）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchedPrices {
    /// 表が名乗る時点。`pricingAsOf` へ入る。
    pub as_of: Option<String>,
    /// モデル名 → 単価。
    pub models: Vec<FetchedPrice>,
    /// 値が不正で落とした件数。**落としたことを画面に出す。**
    pub dropped: u32,
}

/// 1 モデルぶんの単価。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchedPrice {
    /// 突き合わせる鍵（`ModelTemplate::model`）。
    pub key: String,
    /// 入力。
    pub input_per_mtok: Option<f64>,
    /// 出力。
    pub output_per_mtok: Option<f64>,
    /// キャッシュ読み。
    pub cache_read_per_mtok: Option<f64>,
    /// キャッシュ書き込み。
    pub cache_write_per_mtok: Option<f64>,
    /// うち 1 時間 TTL。
    pub cache_write_1h_per_mtok: Option<f64>,
    /// 入力の窓（Spec 50）。`contextLength` へ入る。表に無いか不正なら `None` で、
    /// 画面は**欄を触らず**通知の 2 文目で「表にありません」と言う。
    pub max_input_tokens: Option<u32>,
}

impl From<ParsedTable> for FetchedPrices {
    fn from(t: ParsedTable) -> Self {
        Self {
            as_of: t.as_of,
            dropped: t.dropped,
            models: t
                .entries
                .into_iter()
                .map(|(key, r, window)| FetchedPrice {
                    key,
                    input_per_mtok: r.input,
                    output_per_mtok: r.output,
                    cache_read_per_mtok: r.cache_read,
                    cache_write_per_mtok: r.cache_write,
                    cache_write_1h_per_mtok: r.cache_write_1h,
                    max_input_tokens: window,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **https 以外は取りに行かない**（Spec 41 D2）。
    #[test]
    fn only_https_is_accepted() {
        assert!(validate_url("https://example.test/p.json").is_ok());
        assert!(validate_url("http://example.test/p.json").is_err());
        assert!(validate_url("file:///etc/passwd").is_err());
        assert!(validate_url("  ").is_err());
    }

    /// **空にすれば取りに行かない**（S4）。
    ///
    /// **「既定が空であること」ではなく「空にできること」が要件**。
    /// rev1 の実装途中は既定が空で、テストもそれを凍結していたが、
    /// **それは状態であって設計ではなかった** — URL が入ったら赤くなり、
    /// 要件は 1 つも壊れていないのにテストだけが止まる形だった。
    #[test]
    fn clearing_the_source_disables_the_fetch() {
        assert!(validate_url("").is_err());
        assert!(validate_url("   ").is_err());
    }

    /// **既定は入っているが、それ自体は通信を起こさない。**
    ///
    /// 通信が起きるのは `fetch_table` を呼んだときだけで、**呼ぶのは
    /// IPC ハンドラ 1 箇所**（起動経路が呼ばないことは下のテストが留める）。
    #[test]
    fn the_default_source_is_a_usable_https_url() {
        let url = PricingSourceConfig::default().url;
        assert!(validate_url(&url).is_ok(), "既定が使えない: {url}");
        assert!(url.starts_with("https://"));
    }

    /// **自動取得の機構を持たない**（`data_contract` の `pricing_fetch_freeze`）。
    ///
    /// 文章だけだと、実機の不満（「古い単価に気づけない」）から
    /// タイマーや到達確認が忍び込む。**ソースを走査して構造で留める。**
    #[test]
    fn the_fetch_has_no_automatic_trigger() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/pricing_source.rs");
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
        let code: String = src
            .split("#[cfg(test)]")
            .next()
            .expect("テストより前の本体")
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for banned in ["spawn", "interval", "sleep", "HEAD", "Instant::now"] {
            assert!(
                !code.contains(banned),
                "pricing_source.rs の本体が `{banned}` を持っている（自動取得の凍結が破れた）"
            );
        }
    }

    /// **起動経路が取得を呼ばない。** `state.rs` は棚を読むだけ。
    #[test]
    fn startup_only_loads_the_shelf_and_never_fetches() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/state.rs");
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
        assert!(
            src.contains("PricingSourceStore::load"),
            "起動時に棚を読んでいない"
        );
        assert!(
            !src.contains("fetch_table"),
            "起動経路が取得を呼んでいる（凍結違反）"
        );
    }
}
