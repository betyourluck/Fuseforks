//! `grep include:` の計器が実際にログへ 1 行残ることを見る結合テスト。
//!
//! 診断ログの宛先はプロセスで 1 つ（`OnceLock`）なので、`tests/diag.rs` と同じく
//! **このファイルは 1 テストだけ**にする。同じバイナリに 2 つ入れると、
//! 先に開いた宛先が勝って後続が何も観測できない。
//!
//! # なぜ計器が要るか
//!
//! `tool:` 行は `args_chars` しか書かないので、**引数の中身はどの計器にも
//! 載っていない**。Spec 16 D1 は「モデルは `*.rs` と書くが、名指しの拒否を読んで
//! `\.rs$` へ直せる」という賭けをしており、**その賭けが当たったかを実機のログから
//! 読めなかった**（2026-08-05 に `concordia.log` を調べて判明）。
//! `run decision:` を足したのと同じ判断で、機構が動いているかどうかは
//! 動いた記録が無ければ確かめられない（`failures.md` #58 の同型）。

use std::path::{Path, PathBuf};

use agent_core::tool::{AgentTool, ToolContext};
use agent_core::{AgentId, GrepTool};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-grep-include-log-{}",
            std::process::id()
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

fn ctx(work_dir: &Path) -> ToolContext {
    ToolContext {
        agent_id: AgentId::from("agent_05"),
        work_dir: Some(work_dir.to_path_buf()),
        cancel: None,
        rag_roots: Vec::new(),
    }
}

/// `include` を使った呼び出しは、**成否に関わらず 1 行残す**。
///
/// glob 形（`*.rs`）と正しい正規表現（`\.rs$`）で `outcome` が分かれることまで見る。
/// ここが分かれていないと、「モデルが間違えた回数」と「次のターンで直した回数」を
/// 後から数えられない。
#[tokio::test]
async fn an_include_call_always_leaves_one_line_with_its_outcome() {
    let dir = TempDir::new();
    std::fs::write(dir.0.join("a.rs"), "needle\n").unwrap();
    let log = dir.0.join("concordia.log");
    agent_core::open_log(&log).expect("開けること");

    // 1. glob 形 — 拒否され、`outcome=glob` が残る。
    let rejected = GrepTool
        .call(
            &ctx(&dir.0),
            &serde_json::json!({ "pattern": "needle", "include": "*.rs" }),
        )
        .await
        .unwrap();
    assert!(rejected.contains("glob ではなく正規表現"), "{rejected}");

    // 2. 直したところ — 通り、`outcome=ok` が残る。
    let accepted = GrepTool
        .call(
            &ctx(&dir.0),
            &serde_json::json!({ "pattern": "needle", "include": "\\.rs$" }),
        )
        .await
        .unwrap();
    assert!(accepted.contains("a.rs"), "{accepted}");

    // 3. `include` を省いた呼び出しでは 1 行も増えない。
    let plain = GrepTool
        .call(
            &ctx(&dir.0),
            &serde_json::json!({ "pattern": "needle" }),
        )
        .await
        .unwrap();
    assert!(plain.contains("a.rs"), "{plain}");

    let body = std::fs::read_to_string(&log).expect("読めること");
    let lines: Vec<&str> = body
        .lines()
        .filter(|line| line.contains("grep include:"))
        .collect();

    assert_eq!(
        lines.len(),
        2,
        "include を使った 2 本だけが残ること（省いた呼び出しでは増えない）: {body}"
    );
    assert!(
        lines[0].contains("agent=agent_05")
            && lines[0].contains("pattern=*.rs")
            && lines[0].contains("outcome=glob"),
        "glob 形が名指しで残ること: {}",
        lines[0]
    );
    assert!(
        lines[1].contains("outcome=ok"),
        "直した呼び出しが ok として残ること: {}",
        lines[1]
    );
}
