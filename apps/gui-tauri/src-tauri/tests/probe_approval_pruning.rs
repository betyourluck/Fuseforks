//! 予定の集合を変える IPC が、前判定の承認の掃除を必ず通ることを留める（Spec 28 D10）。
//!
//! **`retain_for` の単体テストは、呼ばれていることを 1 ミリも保証しない。**
//! 初版は `create_schedule` からしか呼んでおらず、`delete_schedule` が素通りだった
//! （`failures.md` #88）。関数は正しく動き、テストも緑で、**doc の
//! 「予定を消せば承認も消える」だけが嘘**という形になっていた。
//!
//! `AppState` を組む結合テストは Tauri の `State` が要って重いので、
//! **ソースを走査して配線を見る**（`defaultEnabledTools.test.ts` /
//! `toolLabel.test.ts` と同じ形 — 名前の並びのずれはコンパイラにも lint にも
//! 引っかからない、という同じ理由でこの村が既に採っている手）。

use std::path::Path;

/// `pub async fn` を名前と本文の対に切り出す（波括弧の対応で終端を取る）。
fn split_pub_async_fns(source: &str) -> Vec<(String, String)> {
    const MARKER: &str = "pub async fn ";
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut cursor = 0;

    while let Some(found) = source[cursor..].find(MARKER) {
        let name_start = cursor + found + MARKER.len();
        let name: String = source[name_start..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();

        // 引数の括弧をまたいで、本文の開き波括弧を探す。
        let Some(open_rel) = source[name_start..].find('{') else {
            break;
        };
        let open = name_start + open_rel;

        let mut depth = 0usize;
        let mut end = open;
        for (index, byte) in bytes.iter().enumerate().skip(open) {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = index;
                        break;
                    }
                }
                _ => {}
            }
        }

        out.push((name, source[open..=end].to_owned()));
        cursor = end.max(name_start);
    }

    out
}

#[test]
fn commands_that_change_the_schedule_set_also_prune_approvals() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands.rs");
    let source = std::fs::read_to_string(&path).expect("src/commands.rs を読めること");

    let functions = split_pub_async_fns(&source);
    assert!(
        functions.len() > 10,
        "走査が壊れている（`pub async fn` を {} 本しか拾えていない）。\
         切り出しが空振りすると、この検査は「全部通った」ように見える",
        functions.len()
    );

    // **予定の集合を変える**ものだけが対象。`set_schedule_enabled` は
    // `schedules.json` を書くが集合は変えないので対象外 —
    // 一時停止は「いまは動かさない」であって取り消しではなく、
    // 再開のたびに承認を押し直させると D10 の目的を超える。
    let mut guarded = Vec::new();
    for (name, body) in &functions {
        if !body.contains(".create_schedule(") && !body.contains(".delete_schedule(") {
            continue;
        }
        guarded.push(name.clone());
        assert!(
            body.contains("retain_for"),
            "`{name}` は予定の集合を変えるのに承認を掃除していない。\
             予定だけ消えて承認が端末に残ると、GUI を通らない経路で同じコマンド行が\
             入ってきたときに人が押していないのに走る（Spec 28 D10）"
        );
    }

    // 見つからなくなったこと自体が退行なので、名前で固定する。
    // **通る側と対で見ないと、1 本も拾えていない実装でも緑になる。**
    guarded.sort();
    assert_eq!(
        guarded,
        vec!["create_schedule".to_owned(), "delete_schedule".to_owned()],
        "対象の IPC が増減した。増えたなら掃除を通したうえでここへ足す"
    );
}
