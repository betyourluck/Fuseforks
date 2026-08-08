//! 村の黒板（共有作業メモ）の GUI 投影。
//!
//! 黒板の実体はエージェントの共通 work_dir にある `blackboard/` フォルダで、
//! 書き手はエージェント（`file` ツール）と人。**GUI からの書き込み経路は
//! 作らない** — 条例の「書いてよいのは自分の付箋だけ」を GUI が迂回する
//! 口を開けない。読みも pull のみで、コアはファイル変更を監視しない
//! （黒板は push しない、という運用の形をコードでも守る）。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult};

/// 黒板フォルダの名前。**条例の記述と必ず一致させる** — 食い違うと、
/// エージェントは条例の名前へ書き、GUI はここの名前を読むので、
/// **書けているのに画面に出ない**という形で割れる。
///
/// **`.` で始まる名前は使えない**（2026-08-05 に `.concordia` 案を検討して却下）。
/// [`crate::tools::fs`] の走査は隠しフォルダを丸ごと外すので、`fd` と `grep` から
/// 見えなくなる — 条例の「着手前に fd + file read」が成立しなくなり、
/// **`file read` は通るので書けるし GUI にも映るのに、エージェントだけが
/// 自分で見つけられない**という壊れ方をする（しかもテストは落ちない）。
///
/// 日本語の `黒板` から改名した理由は、**言語に依存しない名前にするため**
/// （利用者判断 2026-08-05）。呼び名としての「黒板」は台帳・設計語に残す。
pub const BLACKBOARD_DIR: &str = "blackboard";

/// 進行役が束ねる付箋。一覧の先頭へ固定する（条例で書き手が 1 本と
/// 決まっている唯一のファイルで、読み手が最初に見るべきもの）。
const SUMMARY_FILE: &str = "まとめ.md";

/// 1 枚あたりの読み上限（bytes）。付箋の想定を大きく超えるファイルで
/// IPC ペイロードが膨れるのを防ぐ。超過分は切り詰めて末尾に注記を足す。
const NOTE_MAX_BYTES: usize = 256 * 1024;

/// 黒板の付箋 1 枚の GUI 投影（読み取り専用）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlackboardNote {
    /// 由来の work_dir（実パス）。複数の work_dir が混在するときの区別用。
    pub dir: String,
    /// ファイル名（`blackboard/` 直下）。
    pub name: String,
    /// 本文。UTF-8 として読めないバイトは置換文字になる。
    pub content: String,
    /// 最終更新時刻（epoch ms・壁時計）。取得できない環境では 0。
    pub modified_ms: u64,
}

/// `{work_dir}/blackboard/` 直下のファイルを読む。フォルダが無ければ空。
///
/// - サブフォルダは無視する（付箋は 1 人 1 ファイルの平置きが条例の形）
/// - 読めない 1 枚は黙って飛ばす（1 枚のロック・権限で黒板全体を人質にしない）
/// - 並びは `まとめ.md` を先頭に、残りはファイル名順
pub async fn read_blackboard_dir(work_dir: &Path) -> CoreResult<Vec<BlackboardNote>> {
    let dir = work_dir.join(BLACKBOARD_DIR);
    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(rd) => rd,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(io_err(&dir, err)),
    };

    let mut notes = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(|e| io_err(&dir, e))? {
        let Ok(meta) = entry.metadata().await else { continue };
        if !meta.is_file() {
            continue;
        }
        let Ok(bytes) = tokio::fs::read(entry.path()).await else { continue };

        let truncated = bytes.len() > NOTE_MAX_BYTES;
        let slice = if truncated { &bytes[..NOTE_MAX_BYTES] } else { &bytes[..] };
        let mut content = String::from_utf8_lossy(slice).into_owned();
        if truncated {
            content.push_str("\n\n…（付箋の想定を超える長さのため、ここで切り詰めました）");
        }

        let modified_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0);

        notes.push(BlackboardNote {
            dir: work_dir.display().to_string(),
            name: entry.file_name().to_string_lossy().into_owned(),
            content,
            modified_ms,
        });
    }

    notes.sort_by(|a, b| {
        let a_is_summary = a.name == SUMMARY_FILE;
        let b_is_summary = b.name == SUMMARY_FILE;
        b_is_summary
            .cmp(&a_is_summary)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(notes)
}

/// I/O エラーへパス情報を添える（`ConfigStore` と同じ形）。
fn io_err(path: &Path, source: std::io::Error) -> CoreError {
    CoreError::ConfigIo {
        path: path.display().to_string(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// テスト用の一時ディレクトリ（`config_store` のものと同じ最小実装）。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "fuseforks-test-{tag}-{}",
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

    #[tokio::test]
    async fn a_missing_blackboard_folder_reads_as_empty_not_as_an_error() {
        let dir = TempDir::new("bb-missing");
        let notes = read_blackboard_dir(&dir.0).await.unwrap();
        assert!(notes.is_empty(), "黒板が未作成の村は普通の状態");
    }

    #[tokio::test]
    async fn notes_are_read_with_the_summary_pinned_first() {
        let dir = TempDir::new("bb-order");
        let board = dir.0.join(BLACKBOARD_DIR);
        std::fs::create_dir_all(&board).unwrap();
        std::fs::write(board.join("ザリ.md"), "調査中: specs/04").unwrap();
        std::fs::write(board.join("まとめ.md"), "# 今日の束ね").unwrap();
        std::fs::write(board.join("ジェミー.md"), "検索語: tokio select").unwrap();
        // サブフォルダは無視（付箋は平置き）。
        std::fs::create_dir_all(board.join("古い黒板")).unwrap();

        let notes = read_blackboard_dir(&dir.0).await.unwrap();
        let names: Vec<&str> = notes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, vec!["まとめ.md", "ザリ.md", "ジェミー.md"]);
        assert_eq!(notes[1].content, "調査中: specs/04");
        assert_eq!(notes[0].dir, dir.0.display().to_string());
        assert!(notes[0].modified_ms > 0, "更新時刻が入ること");
    }

    #[tokio::test]
    async fn an_oversized_note_is_truncated_with_a_notice() {
        let dir = TempDir::new("bb-truncate");
        let board = dir.0.join(BLACKBOARD_DIR);
        std::fs::create_dir_all(&board).unwrap();
        std::fs::write(board.join("巨大.md"), "あ".repeat(200_000)).unwrap();

        let notes = read_blackboard_dir(&dir.0).await.unwrap();
        assert_eq!(notes.len(), 1);
        assert!(notes[0].content.chars().count() < 200_000);
        assert!(notes[0].content.ends_with("切り詰めました）"));
    }
}
