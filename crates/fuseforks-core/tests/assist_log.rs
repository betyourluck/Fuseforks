//! AI 下書き補助の計器 `assist:`（Spec 63 D10）。**ログは処理全体で 1 本なので別のテストバイナリにする。**
//!
//! 留めるのは 2 点 — 1 回の呼び出しごとの `attempt=` / `outcome=` / `forced=`（判断役の輪が
//! `invalid` → `draft` と読めること）と、**会話・編集中の本文・下書きを 1 字も出さないこと**（#71）。

mod assist_support;

use assist_support::{BAD_RULES, GOOD_RULES, JUDGE, TempDir, draft, village_in};
use fuseforks_core::assist::{AssistKind, AssistRequest, AssistTarget};

#[tokio::test]
async fn the_assist_line_is_one_per_call_and_never_carries_text() {
    let dir = TempDir::new("log");
    let log_path = dir.0.join("fuseforks.log");
    fuseforks_core::open_log(&log_path).expect("ログを開けること");

    let v = village_in(dir, vec![draft("c1", BAD_RULES), draft("c2", GOOD_RULES)], |_| {}).await;
    v.orchestrator
        .assist_draft(AssistRequest {
            target: AssistTarget { kind: AssistKind::Judge, id: JUDGE.into() },
            template_id: "gen".into(),
            history: vec![],
            input: Some("SECRET-INPUT を振り分けたい".into()),
            force_draft: true,
            current: "SECRET-CURRENT".into(),
        })
        .await
        .unwrap();

    let log = std::fs::read_to_string(&log_path).unwrap();
    let lines: Vec<&str> = log.lines().filter(|l| l.contains("assist: kind=judge")).collect();
    assert_eq!(lines.len(), 2, "呼び出し 1 回に 1 行:\n{log}");
    assert!(lines[0].contains("model=gen-model attempt=1 forced=yes outcome=invalid"), "{}", lines[0]);
    assert!(lines[1].contains("attempt=2 forced=yes outcome=draft"), "{}", lines[1]);
    assert!(lines[1].contains("prompt=1000 cached=200 total=1100 reasoning=10"), "{}", lines[1]);
    assert!(lines[1].contains("current_chars=14"), "{}", lines[1]);
    for secret in ["SECRET-INPUT", "SECRET-CURRENT", "[questions.kind]", "依頼の種類"] {
        assert!(!log.contains(secret), "本文がログに出ている: {secret}");
    }
}
