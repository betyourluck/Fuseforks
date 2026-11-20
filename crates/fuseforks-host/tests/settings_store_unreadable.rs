//! ホストの棚 5 つは、「ファイルが無い」だけを未作成として扱い、それ以外の読み取りエラーでは
//! 書き込みを拒む（Spec 77 #5）。
//!
//! 旧実装は `std::fs::read_to_string` の `Err(_)` を NotFound か確かめずに「未作成」と読んでいた。
//! 中身が UTF-8 でない（メモ帳で UTF-16 で保存した・ディスクの破損）・権限が無い・他のプロセスが
//! 共有なしで開いている、のどれでも既定値で読み、次の保存で人が入れた設定・合鍵・承認の一覧を
//! 上書きしていた。JSON として壊れたファイルは元から守られていたので、ここでは UTF-8 でない
//! ファイルで確かめる（`read_to_string` が `InvalidData` を返す）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use fuseforks_host::image_gen_settings::{self, ImageGenSettingsConfig, ImageGenSettingsStore};
use fuseforks_host::jev_settings::{self, JevSettingsConfig, JevSettingsStore};
use fuseforks_host::mcp_server::{self, McpServerConfig, McpServerStore};
use fuseforks_host::pricing_source::{self, PricingSourceConfig, PricingSourceStore};
use fuseforks_host::probe_approvals::{self, ApprovalStore};

/// UTF-16 LE の BOM で始まる中身（メモ帳で「UTF-16」を選んで保存した形）。
const NOT_UTF8: &[u8] = &[0xFF, 0xFE, b'{', 0x00, b'}', 0x00];

fn scratch(label: &str) -> PathBuf {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "fuseforks-unreadable-{label}-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn plant(dir: &Path, file: &str) -> PathBuf {
    let path = dir.join(file);
    std::fs::write(&path, NOT_UTF8).unwrap();
    path
}

fn assert_untouched(path: &Path) {
    assert_eq!(
        std::fs::read(path).unwrap(),
        NOT_UTF8,
        "**1 バイトも上書きしない** — 人が直せば戻る"
    );
}

#[test]
fn mcp_server_store_blocks_writes_when_the_file_is_not_utf8() {
    let dir = scratch("mcp");
    let path = plant(&dir, mcp_server::CONFIG_FILE);
    let mut store = McpServerStore::load(&dir);
    assert!(store.blocked().is_some(), "読めなかったことを覚えている");
    assert!(store.save(McpServerConfig::default()).is_err(), "保存は拒まれる");
    assert_untouched(&path);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn jev_settings_store_blocks_writes_when_the_file_is_not_utf8() {
    let dir = scratch("jev");
    let path = plant(&dir, jev_settings::CONFIG_FILE);
    let mut store = JevSettingsStore::load(&dir);
    assert!(store.blocked().is_some(), "黙って OFF にしない（Spec 59 D9）");
    assert!(store.save(JevSettingsConfig::default()).is_err(), "保存は拒まれる");
    assert_untouched(&path);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn image_gen_settings_store_blocks_writes_when_the_file_is_not_utf8() {
    let dir = scratch("image");
    let path = plant(&dir, image_gen_settings::CONFIG_FILE);
    let mut store = ImageGenSettingsStore::load(&dir);
    assert!(store.blocked().is_some(), "読めなかったことを覚えている");
    assert!(store.save(ImageGenSettingsConfig::default()).is_err(), "保存は拒まれる");
    assert_untouched(&path);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn pricing_source_store_blocks_writes_when_the_file_is_not_utf8() {
    let dir = scratch("pricing");
    let path = plant(&dir, pricing_source::CONFIG_FILE);
    let mut store = PricingSourceStore::load(&dir);
    assert!(store.blocked().is_some(), "読めなかったことを覚えている");
    assert!(store.save(PricingSourceConfig::default()).is_err(), "保存は拒まれる");
    assert_untouched(&path);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn approval_store_blocks_writes_when_the_file_is_not_utf8() {
    let dir = scratch("approvals");
    let path = plant(&dir, probe_approvals::APPROVALS_FILE);
    let store = ApprovalStore::load(&dir);
    assert!(
        store.approve("0".repeat(64)).is_err(),
        "読めないあいだに承認を足すと、それまでの承認が全部消える — 拒む"
    );
    assert_untouched(&path);
    std::fs::remove_dir_all(&dir).ok();
}

/// 読めない `pricing.json` のあいだは、単価表を取りに行かない（Spec 77 N2）。
///
/// 読めないときの設定は既定（開発者の配信元）なので、URL を空にして外へ出ない設定にしていた人でも
/// 「取得」を押すと既定の配信元へ出ていた。
#[test]
fn an_unreadable_pricing_source_does_not_hand_out_the_default_url() {
    let dir = scratch("pricing-url");
    plant(&dir, pricing_source::CONFIG_FILE);
    let store = PricingSourceStore::load(&dir);
    let err = store.fetch_url().expect_err("読めないあいだは取得の URL を渡さない");
    assert!(err.contains(pricing_source::CONFIG_FILE), "実際: {err}");
    std::fs::remove_dir_all(&dir).ok();
}

/// 対照: ファイルが無いのは失敗ではない（初回はこれが正常）。
#[test]
fn a_missing_file_is_not_a_failure() {
    let dir = scratch("missing");
    assert!(McpServerStore::load(&dir).blocked().is_none());
    assert!(JevSettingsStore::load(&dir).blocked().is_none());
    assert!(ImageGenSettingsStore::load(&dir).blocked().is_none());
    assert!(PricingSourceStore::load(&dir).blocked().is_none());
    assert!(PricingSourceStore::load(&dir).fetch_url().is_ok(), "未作成なら既定の配信元");
    assert!(ApprovalStore::load(&dir).approve("0".repeat(64)).is_ok());
    std::fs::remove_dir_all(&dir).ok();
}
