//! 起動と終了の配線を留める（Spec 69 D0・D2・D3。`launch_exit_contract` 1〜5）。
//!
//! 起動の順序は `lib.rs` の `setup` と、組み立ての結果を待つタスクの中にあり、`AppHandle` が要るので
//! 単体では組めない。**順序が崩れると、隠して起動したまま組み立てに失敗したプロセスが窓もトレイも
//! 無いまま多重起動の枠だけを握る**（rev2 の査読 1-1）— 型にも lint にも掛からないので、
//! `probe_approval_pruning.rs` と同じく**ソースを走査して配線を見る**。

use std::path::Path;

fn read(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

/// コメントと doc の行を落とす（説明文に名前を書くのは自由）。
fn code_of(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `haystack` の中で `needles` がこの順に現れることを確かめる。
fn assert_in_order(haystack: &str, needles: &[&str], what: &str) {
    let mut from = 0;
    for needle in needles {
        let Some(found) = haystack[from..].find(needle) else {
            panic!("{what}: `{needle}` が順序どおりに見つからない（直前までの並び: {needles:?}）");
        };
        from += found + needle.len();
    }
}

/// 本文の切り出し（`marker` から始まる関数の波括弧の対応で終端を取る）。
fn body_of<'a>(source: &'a str, marker: &str) -> &'a str {
    let start = source.find(marker).unwrap_or_else(|| panic!("`{marker}` が無い"));
    let open = start + source[start..].find('{').expect("開き波括弧");
    let mut depth = 0usize;
    for (index, byte) in source.bytes().enumerate().skip(open) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[start..=index];
                }
            }
            _ => {}
        }
    }
    panic!("`{marker}` の本文が閉じていない");
}

#[test]
fn the_window_is_created_by_setup_from_the_config() {
    let config = read("tauri.conf.json");
    assert!(
        config.contains("\"create\": false"),
        "tauri.conf.json の窓は create: false（setup で見える / 見えないを決めて作る。D5 の案 A）"
    );
    let lib = code_of(&read("src/lib.rs"));
    assert!(lib.contains("WebviewWindowBuilder::from_config"), "窓は設定から作る");
    assert!(lib.contains(".visible(!hidden)"), "見えない状態で作るのは hidden のときだけ");
}

#[test]
fn a_failed_boot_shows_a_hidden_window_and_a_successful_one_manages_before_the_tray_and_the_batch() {
    let lib = code_of(&read("src/lib.rs"));
    // 成功: 中継 → manage → after_boot（トレイ → 一括起動）。
    assert_in_order(
        &lib,
        &["Ok(app_state) =>", "spawn_event_bridge", "handle.manage(app_state)", "after_boot("],
        "組み立てが成功したときの順序",
    );
    // 失敗: 理由を残してから、隠して作っていたら窓を出す（トレイは作らない）。
    let failure = &lib[lib.find("Err(failure) =>").expect("失敗の腕")..];
    let failure = &failure[..failure.find("Ok(())").unwrap_or(failure.len())];
    assert_in_order(
        failure,
        &["*guard = Some(failure)", "if hidden", "tray::show_main(&handle)"],
        "組み立てに失敗したときの順序",
    );
    assert!(!failure.contains("tray::install"), "失敗したプロセスにトレイを作らない");

    let after_boot = body_of(&lib, "async fn after_boot(");
    assert_in_order(
        after_boot,
        &[
            "tray::install(",
            "if hidden && !tray_ready",
            "tray::show_main(handle)",
            "tauri::async_runtime::spawn(launch::run(",
        ],
        "after_boot はトレイを作り、隠しているのにトレイが無ければ窓を出し、一括起動を別のタスクで回す",
    );
}

#[test]
fn the_rust_side_never_hides_the_window() {
    // **窓を隠すのは画面の onCloseRequested の 1 箇所だけ**（launch_exit_contract 2）。Rust 側
    // （RunEvent・トレイ・多重起動の受け口）に hide を置くと、Cmd+Q や終了の要求まで隠しかねない。
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let code = code_of(&std::fs::read_to_string(&path).unwrap());
        assert!(!code.contains(".hide("), "{} が窓を隠している", path.display());
    }
}

#[test]
fn changing_the_language_relabels_the_tray_after_it_is_saved() {
    let commands = code_of(&read("src/commands.rs"));
    let set_language = body_of(&commands, "pub async fn set_language(");
    assert_in_order(
        set_language,
        &[".set_language(language).await?", "tray::relabel(&app, language)"],
        "言語は保存が成功してからトレイの文言を差し替える",
    );
}
