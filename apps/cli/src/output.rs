//! 出力の形（Spec 64 D7 / D8・`headless_host_contract` 凍結 13）。
//!
//! **標準出力は答え（`ask`）か検査の結果（`check`）だけ。** 指摘・診断・CLI 自身の行は
//! すべて標準エラーへ出す — `ask` の標準出力をそのままパイプへ流せるようにする。
//!
//! `--events jsonl` の間は**標準エラーの全行が JSON**。行の種類は 3 つで、
//! CoreEvent（IPC と同じワイヤ形 = `serde_json::to_string`）/ CLI 自身の行
//! `{"type":"cli","level":…,"code":…,"message":…}` / 診断の行 `{"type":"log",…}`
//! （`diag::set_stderr_json`。コアの `note!` が出す）。素の行を 1 行でも混ぜると、
//! 読む側のパーサが壊れる。

use fuseforks_core::event::CoreEvent;
use fuseforks_core::headless::{Finding, FindingLevel};

/// CLI 自身の行の重さ。`--events jsonl` の `level` にそのまま出る。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// 起動しない・続けられない。
    Error,
    /// 続けるが、設定どおりには動かない部分がある。
    Warn,
    /// 確認のための情報。
    Info,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Error => "エラー",
            Self::Warn => "警告",
            Self::Info => "情報",
        }
    }
}

/// 標準エラーへの出口。`Copy` で、イベントの転送タスクへそのまま渡せる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Output {
    /// `--events jsonl`。
    pub jsonl: bool,
}

impl Output {
    /// CLI 自身の 1 行。`code` は閉じた識別子（機械が読む）、`message` は人が読む文。
    pub fn cli(self, level: Level, code: &str, message: &str) {
        eprintln!("{}", cli_line(self.jsonl, level, code, message));
    }

    /// CoreEvent を 1 行（`--events jsonl` のときだけ。人が読む形では出さない —
    /// 毎秒の統計まで流れて、答えと指摘が埋もれる）。
    pub fn event(self, event: &CoreEvent) {
        if !self.jsonl {
            return;
        }
        match serde_json::to_string(event) {
            Ok(line) => eprintln!("{line}"),
            // シリアライズに失敗しても素の行は出さない（凍結 13）。
            Err(err) => self.cli(Level::Warn, "EVENT_UNSERIALIZABLE", &err.to_string()),
        }
    }

    /// 検査の指摘を標準エラーへ（`ask` / `serve`）。情報は `show_info` のときだけ。
    pub fn findings(self, findings: &[Finding], show_info: bool) {
        for finding in findings {
            if finding.level == FindingLevel::Info && !show_info {
                continue;
            }
            eprintln!("{}", finding_line(self.jsonl, finding));
        }
    }
}

/// CLI 自身の 1 行を組む（純関数 — 単体で書式を留める）。
pub fn cli_line(jsonl: bool, level: Level, code: &str, message: &str) -> String {
    if jsonl {
        serde_json::json!({
            "type": "cli",
            "level": level.as_str(),
            "code": code,
            "message": message,
        })
        .to_string()
    } else {
        format!("[fuseforks-cli] {}: {message}", level.label())
    }
}

/// 指摘 1 件を標準エラーの 1 行（jsonl）か 2 行（人が読む形）に組む。
///
/// jsonl では `type: "cli"` の行に `fix` を足す — 行の種類を増やさない（凍結 13 の 3 種）。
pub fn finding_line(jsonl: bool, finding: &Finding) -> String {
    if jsonl {
        serde_json::json!({
            "type": "cli",
            "level": level_str(finding.level),
            "code": finding.code,
            "message": finding.message,
            "fix": finding.fix,
        })
        .to_string()
    } else {
        format!(
            "[fuseforks-cli] {} {}: {}\n  直し方: {}",
            level_label(finding.level),
            finding.code,
            finding.message,
            finding.fix
        )
    }
}

/// `check` の結果（標準出力）。`--json` は `{"findings":[…]}` の 1 行、人が読む形は
/// 指摘ごとの 2 行と、末尾に件数の 1 行。指摘が 0 件なら「問題ありません」。
pub fn check_report(json: bool, findings: &[Finding], start: &[String]) -> String {
    if json {
        return serde_json::json!({ "findings": findings, "start": start }).to_string();
    }
    let mut out = String::new();
    out.push_str(&format!(
        "起動する集合: {}\n",
        if start.is_empty() {
            "（なし）".to_owned()
        } else {
            start.join(", ")
        }
    ));
    for finding in findings {
        out.push_str(&format!(
            "{} {}: {}\n  直し方: {}\n",
            level_label(finding.level),
            finding.code,
            finding.message,
            finding.fix
        ));
    }
    let count = |level| findings.iter().filter(|f| f.level == level).count();
    if findings.is_empty() {
        out.push_str("問題ありません\n");
    } else {
        out.push_str(&format!(
            "拒否 {} / 警告 {} / 情報 {}\n",
            count(FindingLevel::Reject),
            count(FindingLevel::Warn),
            count(FindingLevel::Info)
        ));
    }
    out
}

fn level_str(level: FindingLevel) -> &'static str {
    match level {
        FindingLevel::Reject => "reject",
        FindingLevel::Warn => "warn",
        FindingLevel::Info => "info",
    }
}

fn level_label(level: FindingLevel) -> &'static str {
    match level {
        FindingLevel::Reject => "拒否",
        FindingLevel::Warn => "警告",
        FindingLevel::Info => "情報",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(level: FindingLevel) -> Finding {
        Finding {
            level,
            code: "SECRET_MISSING",
            message: "秘密がありません\n2 行目".into(),
            fix: "置いてください".into(),
        }
    }

    /// jsonl の行は 1 行に収まり、種類は `cli` だけ（凍結 13 の 3 種のうち CLI の分）。
    #[test]
    fn jsonl_lines_are_single_line_json_of_type_cli() {
        for line in [
            cli_line(true, Level::Warn, "X", "a\nb \"q\""),
            finding_line(true, &finding(FindingLevel::Reject)),
        ] {
            assert!(!line.contains('\n'), "{line}");
            let value: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(value["type"], "cli");
        }
        let value: serde_json::Value =
            serde_json::from_str(&finding_line(true, &finding(FindingLevel::Reject))).unwrap();
        assert_eq!(value["level"], "reject");
        assert_eq!(value["code"], "SECRET_MISSING");
        assert_eq!(value["fix"], "置いてください");
    }

    /// `--json` の検査結果は `findings` を FindingLevel の serde 名のまま運ぶ。
    #[test]
    fn check_json_carries_findings_as_serialized() {
        let line = check_report(true, &[finding(FindingLevel::Warn)], &["agent_1".into()]);
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["findings"][0]["level"], "warn");
        assert_eq!(value["findings"][0]["code"], "SECRET_MISSING");
        assert_eq!(value["start"][0], "agent_1");
    }

    #[test]
    fn check_human_counts_by_level_and_says_ok_when_empty() {
        assert!(check_report(false, &[], &[]).ends_with("問題ありません\n"));
        let report = check_report(
            false,
            &[finding(FindingLevel::Reject), finding(FindingLevel::Info)],
            &[],
        );
        assert!(report.ends_with("拒否 1 / 警告 0 / 情報 1\n"), "{report}");
    }
}
