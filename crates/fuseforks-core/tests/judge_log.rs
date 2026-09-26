//! 判断役の計器 `judge:`（Spec 62 D12）。**ログは処理全体で 1 本なので別のテストバイナリにする。**
//!
//! 留めるのは 2 点 — 行の形（撒いたときの `outcome=fanned` と `bundle_chars=`・判定できなかったときの
//! `reason=`）と、**依頼の本文を 1 字も出さないこと**（`failures.md` #71）。

mod judge_support;

use judge_support::{TempDir, run, village_in};

#[tokio::test]
async fn the_judge_line_carries_outcome_and_values_but_never_the_message() {
    let dir = TempDir::new("log");
    let log_path = dir.0.join("fuseforks.log");
    fuseforks_core::open_log(&log_path).expect("ログを開けること");

    let v = village_in(dir, Some(("implement", 0.8)), true, false).await;
    run(&v).await;

    let log = std::fs::read_to_string(&log_path).unwrap();
    let line = log
        .lines()
        .find(|l| l.contains("judge: caller=agent_01 judge=router"))
        .unwrap_or_else(|| panic!("judge: 行が無い:\n{log}"));
    assert!(line.contains("rule=2 outcome=fanned"), "{line}");
    assert!(line.contains("answers=kind:implement/0.80/0.80"), "{line}");
    assert!(line.contains("to=agent_02,agent_03"), "{line}");
    assert!(line.contains("bundle_chars="), "{line}");
    assert!(line.contains("input_tokens=10 output_tokens=1"), "{line}");
    // 撒いても波の行は出ない（execute_wave を通らない）。
    assert!(!log.contains("plan wave:") && !log.contains("plan bundle:"), "波の行が出ている");
    // 依頼の本文はどの行にも出ない。
    assert!(!log.contains("LangGraph と AionUi"), "依頼の本文がログに出ている");
}
