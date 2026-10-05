//! Fuseforks のホスト層 — 村を開いて組み立て、閉じる（Spec 64）。
//!
//! **Tauri を知らない。** GUI（`apps/gui-tauri`）と実行ファイル `fuseforks-cli` の
//! 両方がここを呼ぶ。GUI 層に残るのは Tauri の Builder・IPC の受け口・イベントの
//! 橋だけで、組み立ての配線（同梱ツールの登録・扉・前判定の承認・Jev・単価表の
//! 取得元）は 1 実装をここに置く。
//!
//! 棚（`mcp_server.json` / `pricing.json` / `probe_approvals.json` / `jev.json`）は
//! **workspace の外**（`data_dir` 直下）に住む — 村を配っても扉は開かず、承認も
//! 単価の取得先も付いてこない、を置き場で成立させている。

pub mod jev_settings;
pub mod pricing_source;
pub mod probe_approvals;
