//! 表示の言語（`DisplayLanguage`）がコアのどこまで入ってよいかを留める（Spec 74 D1）。
//!
//! 保証したいのは「**モデルへ届く文の分岐（`pick` と `match`、約 200 か所）が `ZhHans` を
//! 受け取らない**」ことで、手段は型の分離 — 分岐が受け取る `Language` に 3 値目が無い。
//! ただし `DisplayLanguage` そのものはコアにも在る（保存と初回確定はコアの仕事）。
//! その字面が分岐のあるファイルへ漏れると、`voice()` を通さずに表示の言語で分岐する形を
//! 誰でも書けてしまうので、現れてよいファイルを 3 つに留める。
//!
//! 走査の対象は `crates/fuseforks-core/src`（コメントも数える — 字面が出ること自体が、
//! そのファイルが表示の言語を知っている印になる）。`tests/` は `Into` 経由で `Language` を
//! 渡すだけなので対象外。

use std::path::{Path, PathBuf};

const ALLOWED: [&str; 3] = [
    "world.rs",
    "orchestrator/settings.rs",
    "orchestrator/bootstrap.rs",
];

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

#[test]
fn display_language_appears_only_where_the_village_stores_it() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);

    let mut found: Vec<String> = files
        .iter()
        .filter(|path| std::fs::read_to_string(path).unwrap().contains("DisplayLanguage"))
        .map(|path| {
            path.strip_prefix(&src)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    found.sort();

    let mut expected: Vec<String> = ALLOWED.iter().map(|s| (*s).to_owned()).collect();
    expected.sort();
    assert_eq!(
        found, expected,
        "DisplayLanguage を読んでよいのは保存と初回確定の 3 ファイルだけ。モデルへの文を組む側は \
         World::language / Orchestrator::language（コアの声）を読む"
    );
}
