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
    /// 仕事の状態 = `blackboard/` 直下のフォルダ名そのもの（Spec 54）。
    /// `None` は直下の平置き（「状態なし」）。**閉じた 5 値かどうかはコアで見ない** —
    /// 5 値の外のフォルダは画面が「その他: <名>」として名指しで出す（閉じた列挙から
    /// 外れた名前を黙って混ぜない）。ワイヤでは無いときに欄ごと省く（既存の形を保つ）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// ファイル名（フォルダを含めない。`state` と合わせて場所が決まる）。
    pub name: String,
    /// 本文。UTF-8 として読めないバイトは置換文字になる。
    pub content: String,
    /// 最終更新時刻（epoch ms・壁時計）。取得できない環境では 0。
    pub modified_ms: u64,
}

/// 付箋のファイル名として受け付けてよいか（パスとして安全か）。
///
/// **GUI から来る値なので、ここが唯一の関門。** `blackboard/` 直下の平置きの
/// ファイル名だけを通す — 区切り文字も `..` も入れさせない。
/// **`read_blackboard_dir` が返した `name` をそのまま返してくる**のが正常系だが、
/// **正常系だけを想定した検査は検査ではない**。
///
/// **区切り文字は自分で数える。`Path` に訊かない** — `\` を区切りとして扱うのは
/// Windows の `Path` だけで、Unix では `sub\note.md` が合法な平置きの名前になる。
/// 判定を `file_name()` に委ねると、**同じ入力の可否が開発機の OS で変わる**
/// （実際に v0.1.3 の CI で macOS と Ubuntu だけが赤くなった）。
/// `file_name()` の比較は残すが、これはドライブ接頭辞のような
/// **Windows 固有の形**を拾う保険であって、区切りの保証はその上の行が持つ。
fn is_safe_note_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains(['/', '\\'])
        && Path::new(name).file_name().and_then(|n| n.to_str()) == Some(name)
}

/// 付箋 1 枚を**ごみ箱へ移す**（2026-08-12 の UI 追加）。
///
/// **完全削除はしない。** `file` ツールの remove と同じ規律で、
/// ごみ箱が使えない環境では**消さずに失敗を返す** — 取り消せない操作へ
/// 勝手に格上げしない。**取り消せるからこそ、個別削除に確認を付けていない。**
///
/// **これは「書き込み」ではない。** 契約の凍結「GUI からの書き込み経路は
/// 作らない」が守っているのは**条例の「書いてよいのは自分の付箋だけ」を
/// GUI が迂回しないこと**で、削除は誰かの名前で内容を書く操作ではない。
/// むしろ**人にしかできない後始末**で、work_dir を移した個体の付箋は
/// 本人が消せない（`resolve_in_work_dir` が届かない）。
///
/// `state` は状態フォルダ（Spec 54）。`None` と空文字は直下。**`state` と `name` は
/// 別々に [`is_safe_note_name`] を通す** — 関門を 2 段にすることで、`name` に
/// 区切りを入れて `blackboard/` の外や 2 段目より深くへ届く経路を開けない。
pub async fn delete_note(work_dir: &Path, state: Option<&str>, name: &str) -> CoreResult<()> {
    let state = state.filter(|s| !s.is_empty());
    if !is_safe_note_name(name) {
        return Err(CoreError::BlackboardDeleteFailed {
            name: name.to_owned(),
            reason: "付箋のファイル名として受け付けられません".to_owned(),
        });
    }
    if let Some(state) = state
        && !is_safe_note_name(state)
    {
        return Err(CoreError::BlackboardDeleteFailed {
            name: name.to_owned(),
            reason: "状態フォルダの名前として受け付けられません".to_owned(),
        });
    }
    let mut path = work_dir.join(BLACKBOARD_DIR);
    if let Some(state) = state {
        path.push(state);
    }
    path.push(name);
    // 既に無いものを消せと言われたら成功として扱う（同じ結末なので、
    // 2 人が同時に消したときに片方だけ赤くする理由が無い）。
    if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
        return Ok(());
    }
    let owned = name.to_owned();
    // trash はブロッキング。ワーカーを塞ぐと他のサーヴァントのターンが止まる。
    tokio::task::spawn_blocking(move || trash::delete(&path))
        .await
        .map_err(|err| CoreError::BlackboardDeleteFailed {
            name: owned.clone(),
            reason: err.to_string(),
        })?
        .map_err(|err| CoreError::BlackboardDeleteFailed {
            name: owned,
            reason: err.to_string(),
        })
}

/// `{work_dir}/blackboard/` の付箋を読む。フォルダが無ければ空。
///
/// - 読むのは**直下のファイルと、直下のフォルダ 1 段の中のファイル**まで（Spec 54）。
///   フォルダの中のフォルダは無視する（2 段目より深い付箋は画面に出ない。`fd` には
///   出るので、条例が「状態フォルダの中にフォルダを作らない」と書いて受ける）
/// - `.` で始まるフォルダは読まない。[`is_safe_note_name`] が先頭 `.` を拒むので、
///   読んでも画面から消せない付箋になる（読めるのに消せない形を作らない）
/// - 読めない 1 枚は黙って飛ばす（1 枚のロック・権限で黒板全体を人質にしない）
/// - 並びは `まとめ.md`（直下）→ 直下の平置き（名前順）→ `state` の文字列順 →
///   その中で名前順。**安定な並びのためで、意味は持たせない** — 画面の列の並び
///   （`doing → on-hold → done`）はフロントが持つ。コアに列順を持たせると 3 値の
///   順序がコアと辞書の 2 箇所に住む
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
        if meta.is_file() {
            if let Some(note) = read_note(work_dir, None, &entry.path(), &meta).await {
                notes.push(note);
            }
            continue;
        }
        if !meta.is_dir() {
            continue;
        }
        let state = entry.file_name().to_string_lossy().into_owned();
        if state.starts_with('.') {
            continue;
        }
        // 1 段目のフォルダの中。読めないフォルダは黙って飛ばす（1 枚の規律と同じ）。
        let Ok(mut inner) = tokio::fs::read_dir(entry.path()).await else { continue };
        while let Ok(Some(child)) = inner.next_entry().await {
            let Ok(child_meta) = child.metadata().await else { continue };
            if !child_meta.is_file() {
                continue;
            }
            if let Some(note) = read_note(work_dir, Some(&state), &child.path(), &child_meta).await
            {
                notes.push(note);
            }
        }
    }

    notes.sort_by(|a, b| {
        let a_is_summary = a.state.is_none() && a.name == SUMMARY_FILE;
        let b_is_summary = b.state.is_none() && b.name == SUMMARY_FILE;
        b_is_summary
            .cmp(&a_is_summary)
            .then_with(|| a.state.cmp(&b.state))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(notes)
}

/// 付箋 1 枚を読む。読めなければ `None`（呼び手が飛ばす）。
async fn read_note(
    work_dir: &Path,
    state: Option<&str>,
    path: &Path,
    meta: &std::fs::Metadata,
) -> Option<BlackboardNote> {
    let bytes = tokio::fs::read(path).await.ok()?;

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

    Some(BlackboardNote {
        dir: work_dir.display().to_string(),
        state: state.map(str::to_owned),
        name: path.file_name()?.to_string_lossy().into_owned(),
        content,
        modified_ms,
    })
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

    /// **GUI から来る名前は `blackboard/` 直下の平置きだけ通す。**
    /// ここが唯一の関門で、通ると `work_dir.join(BLACKBOARD_DIR).join(name)` が
    /// 黒板の外を指しうる。**正常系（一覧が返した name）だけを想定した検査は
    /// 検査ではない。**
    #[test]
    fn only_flat_note_names_are_accepted() {
        assert!(is_safe_note_name("ザリ.md"));
        assert!(is_safe_note_name("まとめ.md"));

        for bad in [
            "",
            "..",
            "../world.json",
            "sub/note.md",
            // Rust の raw 文字列。**普通の文字列だと \n が改行になり、
            // Windows の区切りとして検査されないまま通ってしまう**（実際に踏んだ）。
            r"sub\note.md",
            "/etc/passwd",
            ".hidden",
            ".",
        ] {
            assert!(!is_safe_note_name(bad), "通してはいけない名前が通った: {bad}");
        }
    }

    /// 無い付箋を消せと言われたら成功。**同じ結末なので、2 人が同時に消した
    /// ときに片方だけ赤くする理由が無い**（10 秒ごとの自動再読と削除は競合する）。
    #[tokio::test]
    async fn deleting_a_missing_note_is_not_an_error() {
        let dir = TempDir::new("bb-del-missing");
        delete_note(&dir.0, None, "居ない.md").await.unwrap();
        delete_note(&dir.0, Some("done"), "居ない.md").await.unwrap();
    }

    /// 危ない名前は**ファイルへ触る前に**落ちる。
    #[tokio::test]
    async fn unsafe_names_are_rejected_before_touching_the_disk() {
        let dir = TempDir::new("bb-del-unsafe");
        let err = delete_note(&dir.0, None, "../world.json").await.unwrap_err();
        assert_eq!(err.code(), "BLACKBOARD_DELETE_FAILED");
    }

    /// **`state` は `name` とは別に関門を通る**（Spec 54 凍結 4）。`name` が安全でも
    /// `state` に区切りや `..` が入れば拒否 — 2 段にしないと、状態フォルダの欄が
    /// `blackboard/` の外へ届く経路になる。
    #[tokio::test]
    async fn unsafe_state_names_are_rejected_before_touching_the_disk() {
        let dir = TempDir::new("bb-del-unsafe-state");
        let board = dir.0.join(BLACKBOARD_DIR).join("done");
        std::fs::create_dir_all(&board).unwrap();
        std::fs::write(board.join("ザリ - 調査.md"), "x").unwrap();

        for bad in ["..", "../..", "done/sub", r"done\sub", "/", ".hidden"] {
            let err = delete_note(&dir.0, Some(bad), "ザリ - 調査.md")
                .await
                .unwrap_err();
            assert_eq!(err.code(), "BLACKBOARD_DELETE_FAILED", "state={bad}");
        }
        assert!(
            board.join("ザリ - 調査.md").exists(),
            "拒否された呼び出しはディスクに触らない"
        );
    }

    /// `state` を渡すとそのフォルダの 1 枚だけが消え、同名の直下の付箋は残る。
    /// 空文字は `None` と同じ（フロントが `undefined` を空文字で送る形を拒否にしない）。
    #[tokio::test]
    async fn state_selects_the_folder_and_empty_state_means_the_root() {
        let dir = TempDir::new("bb-del-state");
        let root = dir.0.join(BLACKBOARD_DIR);
        let done = root.join("done");
        std::fs::create_dir_all(&done).unwrap();
        std::fs::write(root.join("ザリ.md"), "root").unwrap();
        std::fs::write(done.join("ザリ.md"), "done").unwrap();

        delete_note(&dir.0, Some("done"), "ザリ.md").await.unwrap();
        assert!(!done.join("ザリ.md").exists(), "done/ の 1 枚が消える");
        assert!(root.join("ザリ.md").exists(), "直下の同名は残る");

        delete_note(&dir.0, Some(""), "ザリ.md").await.unwrap();
        assert!(!root.join("ザリ.md").exists(), "空文字の state は直下");
    }

    /// **1 段目のフォルダの中まで読み、2 段目より深くは読まない**（Spec 54 凍結 1）。
    /// 3 値の外のフォルダ（`foo` / `bar`）もコアはそのまま `state` に載せる — 閉じた列挙を
    /// 見るのは画面で、コアは名指しの材料を落とさない。`.` で始まるフォルダは読まない。
    #[tokio::test]
    async fn notes_one_folder_deep_are_read_with_their_state() {
        let dir = TempDir::new("bb-state");
        let root = dir.0.join(BLACKBOARD_DIR);
        for state in ["doing", "on-hold", "done", "foo", "bar", ".git"] {
            std::fs::create_dir_all(root.join(state)).unwrap();
            std::fs::write(root.join(state).join("ザリ - 調査.md"), state).unwrap();
        }
        std::fs::create_dir_all(root.join("done").join("2026")).unwrap();
        std::fs::write(root.join("done").join("2026").join("深い.md"), "deep").unwrap();
        std::fs::write(root.join("ルナ.md"), "root").unwrap();

        let notes = read_blackboard_dir(&dir.0).await.unwrap();
        let places: Vec<(Option<&str>, &str)> = notes
            .iter()
            .map(|n| (n.state.as_deref(), n.name.as_str()))
            .collect();
        assert_eq!(
            places,
            vec![
                (None, "ルナ.md"),
                (Some("bar"), "ザリ - 調査.md"),
                (Some("doing"), "ザリ - 調査.md"),
                (Some("done"), "ザリ - 調査.md"),
                (Some("foo"), "ザリ - 調査.md"),
                (Some("on-hold"), "ザリ - 調査.md"),
            ],
            "直下 → state の文字列順（列順ではない）。2 段目と `.git` は出ない"
        );
        let done = notes.iter().find(|n| n.state.as_deref() == Some("done")).unwrap();
        assert_eq!(done.content, "done");
    }

    /// `まとめ.md` が先頭に固定されるのは**直下のものだけ**（凍結 9）。
    /// `state` の中の `まとめ.md` は普通の付箋として state の並びに入る。
    #[tokio::test]
    async fn only_the_root_summary_is_pinned_first() {
        let dir = TempDir::new("bb-summary-state");
        let root = dir.0.join(BLACKBOARD_DIR);
        std::fs::create_dir_all(root.join("doing")).unwrap();
        std::fs::write(root.join("doing").join("まとめ.md"), "x").unwrap();
        std::fs::write(root.join("doing").join("あ.md"), "x").unwrap();
        std::fs::write(root.join("ん.md"), "x").unwrap();
        std::fs::write(root.join("まとめ.md"), "x").unwrap();

        let notes = read_blackboard_dir(&dir.0).await.unwrap();
        let places: Vec<(Option<&str>, &str)> = notes
            .iter()
            .map(|n| (n.state.as_deref(), n.name.as_str()))
            .collect();
        assert_eq!(
            places,
            vec![
                (None, "まとめ.md"),
                (None, "ん.md"),
                (Some("doing"), "あ.md"),
                (Some("doing"), "まとめ.md"),
            ]
        );
    }

    /// ワイヤ形: `state` が無いときは欄ごと省く（既存の形を保つ）。
    #[test]
    fn state_is_omitted_from_the_wire_when_absent() {
        let root = BlackboardNote {
            dir: "d".into(),
            state: None,
            name: "ザリ.md".into(),
            content: String::new(),
            modified_ms: 0,
        };
        let json = serde_json::to_value(&root).unwrap();
        assert!(json.get("state").is_none());
        let filed = BlackboardNote {
            state: Some("done".into()),
            ..root
        };
        let json = serde_json::to_value(&filed).unwrap();
        assert_eq!(json["state"], "done");
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
        // 空のサブフォルダは何も足さない（中身が無い状態フォルダ）。
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
