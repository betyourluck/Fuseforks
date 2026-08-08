//! 診断ログの出口の結合テスト。
//!
//! 出口はプロセスで 1 つ（`OnceLock`）なので、**このファイルは 1 テストだけ**に
//! する。他のテストと同じバイナリに入れると、先に開いた宛先が勝って後続が
//! 何も観測できない。

use std::path::PathBuf;

/// `open_log` の後は、`note!` の行がファイルにも残ること。
///
/// stderr だけに出していた間、この行は端末を持っている人にしか読めなかった。
#[test]
fn notes_reach_the_log_file_after_open() {
    let dir: PathBuf = std::env::temp_dir().join(format!("fuseforks-diag-it-{}", std::process::id()));
    let path = dir.join("concordia.log");
    let _guard = scopeguard(dir.clone());

    agent_core::open_log(&path).expect("開けること");
    agent_core::note!("turn: agent={} rounds={}", "agent_01", 19);

    let body = std::fs::read_to_string(&path).expect("読めること");
    assert!(body.contains("起動しました"), "起動の区切りが残ること: {body}");
    assert!(
        body.contains("[concordia] turn: agent=agent_01 rounds=19"),
        "行が接頭辞つきで残ること: {body}"
    );
    // 時刻はファイル側にだけ付ける（stderr の見え方は変えない）。
    // 端末の行と突き合わせるとき、時刻が無いと「どの起動のどの周か」が解けない。
    let line = body
        .lines()
        .find(|l| l.contains("turn: agent=agent_01"))
        .unwrap();
    assert!(
        line.starts_with("20") && line.contains(':'),
        "行頭に時刻が付くこと: {line}"
    );

    // 2 回目の open_log は宛先を奪わない（最初に決まった出口が勝つ）。
    let other = dir.join("別の宛先.log");
    agent_core::open_log(&other).expect("2 回目も Err にはしない");
    agent_core::note!("2 回目の open の後");
    assert!(
        std::fs::read_to_string(&path).unwrap().contains("2 回目の open の後"),
        "最初の宛先へ出続けること"
    );
}

/// 後始末（テスト用の一時フォルダを消す）。
fn scopeguard(dir: PathBuf) -> impl Drop {
    struct Guard(PathBuf);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    Guard(dir)
}
