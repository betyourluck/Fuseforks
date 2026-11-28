//! 判断役の「非表示に連動する」（`JudgeSpec::hide_with_group`。2026-11-29 利用者要望）。
//!
//! 留めるのは 3 つ — 連動先が `world.json` を往復して残り、一覧の姿（`JudgeView`）に写る /
//! グループを消しても連動先には触らない（`group_contract` 凍結 3 と同じ。画面が「連動しない」で描く）/
//! **コアの評価・配送はこの欄を読まない**（表示だけ。走査で留める — 読んでよい場所を名指しする）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use fuseforks_core::model::{AgentGroupId, JudgeSpec, UnknownFields};
use fuseforks_core::{ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig};

/// 一時フォルダの通し番号（ナノ秒だけだと並列のテストで名前が衝突しうる — Spec 65 P2 で踏んだ）。
static SEQ: AtomicU32 = AtomicU32::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-judge-hide-{tag}-{}-{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
            SEQ.fetch_add(1, Ordering::Relaxed),
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

async fn setup(dir: &TempDir) -> Orchestrator {
    let orchestrator = Orchestrator::bootstrap(
        ConfigStore::new(&dir.0),
        Arc::new(FixedBackendFactory::echo("[echo]")),
        Arc::new(InMemorySecretStore::new()),
        OrchestratorConfig::default(),
    )
    .await
    .expect("bootstrap できること");
    orchestrator.set_language(fuseforks_core::world::Language::Ja).await.unwrap();
    orchestrator
}

fn judge(hide_with_group: Option<AgentGroupId>) -> JudgeSpec {
    JudgeSpec {
        id: "madoguchi".into(),
        name: "窓口".into(),
        order: 0,
        enabled: true,
        hide_with_group,
        unknown: UnknownFields::default(),
    }
}

/// 連動先は `world.json` を往復して残り、一覧の姿に写る（画面はこれを読んで地図を隠す）。
#[tokio::test]
async fn the_link_survives_a_restart_and_shows_in_the_view() {
    let dir = TempDir::new("persist");
    let group_id = {
        let orchestrator = setup(&dir).await;
        let group = orchestrator.create_group("投資").await.unwrap();
        orchestrator.create_judge(judge(Some(group.id.clone()))).await.unwrap();
        let views = orchestrator.judges().await;
        assert_eq!(views[0].hide_with_group.as_ref(), Some(&group.id), "一覧の姿に写る");
        group.id
    };

    let reopened = setup(&dir).await;
    let views = reopened.judges().await;
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].hide_with_group, Some(group_id), "再起動の後も残る");
}

/// 更新は欄ごと差し替える（「なし」を選ぶと外れる）。有効/無効だけを直す操作でも、画面が写して
/// 送れば連動先は残る（`JudgeList` のトグル）。
#[tokio::test]
async fn updating_replaces_the_link() {
    let dir = TempDir::new("update");
    let orchestrator = setup(&dir).await;
    let group = orchestrator.create_group("投資").await.unwrap();
    orchestrator.create_judge(judge(None)).await.unwrap();
    assert_eq!(orchestrator.judges().await[0].hide_with_group, None, "既定は連動しない");

    orchestrator.update_judge(judge(Some(group.id.clone()))).await.unwrap();
    assert_eq!(orchestrator.judges().await[0].hide_with_group.as_ref(), Some(&group.id));

    let toggled = JudgeSpec { enabled: false, ..judge(Some(group.id.clone())) };
    orchestrator.update_judge(toggled).await.unwrap();
    assert_eq!(orchestrator.judges().await[0].hide_with_group.as_ref(), Some(&group.id), "写して送れば残る");

    orchestrator.update_judge(judge(None)).await.unwrap();
    assert_eq!(orchestrator.judges().await[0].hide_with_group, None, "「なし」で外れる");
}

/// グループを消しても連動先には触らない（`group_contract` 凍結 3 と同じ）。画面は引けない id を
/// 「連動しない」で描き、次に判断役を保存する操作で `None` へ正規化する。
#[tokio::test]
async fn deleting_the_group_leaves_the_link_untouched() {
    let dir = TempDir::new("delete");
    let orchestrator = setup(&dir).await;
    let group = orchestrator.create_group("投資").await.unwrap();
    orchestrator.create_judge(judge(Some(group.id.clone()))).await.unwrap();
    orchestrator.remove_group(&group.id).await.unwrap();

    assert_eq!(orchestrator.judges().await[0].hide_with_group, Some(group.id), "引けない id が残る");
}

/// 本体のコードだけを返す（最初の `#[cfg(test)]` より前で、行コメントと doc を落とす）。
/// 説明文に欄の名前を書くのは自由 — 見たいのは「読んでいるか」。
fn code_lines(path: &Path) -> Vec<String> {
    let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
    src.lines()
        .take_while(|l| l.trim() != "#[cfg(test)]")
        .map(|l| if l.trim_start().starts_with("//") { String::new() } else { l.to_owned() })
        .collect()
}

/// `start` を含む行から、`end` と一致する最初の行までの範囲（両端を含む）。
fn block(lines: &[String], start: &str, end: &str) -> std::ops::RangeInclusive<usize> {
    let from = lines.iter().position(|l| l.contains(start)).unwrap_or_else(|| panic!("`{start}` が見つからない"));
    let to = lines[from..].iter().position(|l| l == end).map(|i| from + i).unwrap_or_else(|| panic!("`{start}` の終わりが見つからない"));
    from..=to
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// **コアの評価・配送はこの欄を読まない**（表示だけ — 利用者「あくまで表示だけです」）。
///
/// 本体（テストを除く）で欄の名前が現れてよいのは 3 箇所だけ — 定義（`model.rs`）/ 一覧の姿の欄
/// （`judge.rs` の `JudgeView`）/ 一覧の姿への写し（`judging.rs` の `judges()`）。評価（`run_judge`・
/// 規則・Jev）・配送（`delegation.rs` / `turn.rs`）・ツールの提示・`plan` のどこかで読み始めると赤くなる。
/// グループの所属が実行経路に入らない網（`agent_groups.rs`）と同じ形で、こちらは**読んでよい場所の
/// 許容**で書く（除外の一覧だと、新しいファイルで読み始めたときに素通りする）。
#[test]
fn the_execution_path_never_reads_the_link() {
    const FIELD: &str = "hide_with_group";
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&root, &mut files);

    let mut seen = Vec::new();
    for path in files {
        let rel = path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        let lines = code_lines(&path);
        let hits: Vec<usize> = lines.iter().enumerate().filter(|(_, l)| l.contains(FIELD)).map(|(i, _)| i).collect();
        if hits.is_empty() {
            continue;
        }
        let allowed = match rel.as_str() {
            "model.rs" => block(&lines, "pub struct JudgeSpec {", "}"),
            "judge.rs" => block(&lines, "pub struct JudgeView {", "}"),
            "orchestrator/judging.rs" => block(&lines, "pub async fn judges(", "    }"),
            other => panic!("{other} の本体が判断役の連動先（{FIELD}）を読んでいる — 表示だけの欄で、コアの評価・配送は読まない"),
        };
        for i in hits {
            assert!(
                allowed.contains(&i),
                "{rel}:{} で {FIELD} を読んでいる — 読んでよいのは定義・一覧の姿・その写しだけ",
                i + 1
            );
        }
        seen.push(rel);
    }
    seen.sort();
    // 網が空振りしていないこと（3 箇所とも実際に見つかっている = 走査が欄の名前を拾えている）。
    assert_eq!(seen, ["judge.rs", "model.rs", "orchestrator/judging.rs"]);
}
