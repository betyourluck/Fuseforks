//! 予定の**前判定**（Spec 28）の型と純機構。
//!
//! ## 何を解いているか
//!
//! 定期発火（Spec 07）は時刻が来ると無条件にチャットを配送する。監視の用途では
//! 「変化が無い」が大半なのに、確かめる仕事そのものを LLM にやらせるとトークンが
//! 毎回出る。5 分ごとなら 1 日 288 回で、変化が 1 回なら 287 回ぶんが無駄になる。
//!
//! そこで発火と配送の間に**コマンド 1 本の判定**を挟む。判定はプロセス実行だけで
//! LLM を通さないので、一致しなかった回は 1 トークンも払わない。
//!
//! ## このモジュールが純関数だけで出来ている理由
//!
//! プロセス実行・現在時刻・承認の有無は**すべて引数で受け取る**。
//! `schedule.rs` と同じ規律で、判定の正しさを I/O 無しで確かめられるようにする。
//! 実際に走らせる側（`tools::run` から切り出した実行の核）と、走った結果を
//! どう読むか（ここ）を分けてある。
//!
//! ## exit code を判定に使わない
//!
//! **監視の慣習は異常を非 0 で表す**（`echo CHANGED; exit 1`）。exit code を
//! 判定に混ぜると、その最も普通の書き方が永遠に発火しない。「壊れたスクリプトの
//! 偶然の出力で課金しない」という懸念は [`ScheduleProbe::expect`] の完全一致が
//! 既に守っている — クラッシュ出力が合図と一字一句同じ行になる壊れ方は実在しない。
//! exit code は計器にだけ載せる。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// stdout の取り込み上限（文字）。
///
/// これを超えた分は捨てる。**打ち切りが 1 行目の中で起きたら判定を不成立にする**
/// （[`Judgement::SignalTruncated`]）— 切り詰めた 1 行目で照合すると、
/// 本来一致しない合図が一致したように見えることがある。
pub const PROBE_STDOUT_CHARS: usize = 65_536;

/// 依頼文へ付記する本文の上限（文字）。`tools::run` の stderr 枠と同じ桁。
pub const PROBE_APPENDIX_CHARS: usize = 4_000;

/// `timeoutSecs` の既定値（秒）。
pub const PROBE_TIMEOUT_DEFAULT: u64 = 60;

/// `timeoutSecs` の上限（秒）。`run.json` と同じ値域。
pub const PROBE_TIMEOUT_MAX: u64 = 3_600;

/// 予定の前判定 1 件。
///
/// **シェルを介さない。** `command` + `args` の配列で起動するので、`&&` も `|` も
/// `$(...)` も構造的に存在しない（Spec 15 が 3 回の査読で守った境界を、
/// 別の入口から破らないため）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleProbe {
    /// 実行ファイル名またはパス。**空・空白のみは読み込みで弾く**。
    pub command: String,
    /// 引数の配列。**順序は意味を持つ**（承認鍵にもそのまま入る）。
    #[serde(default)]
    pub args: Vec<String>,
    /// stdout の 1 行目（トリム後）と完全一致させる合図。**空は読み込みで弾く**。
    pub expect: String,
    /// 打ち切りまでの秒数。
    #[serde(default = "timeout_default")]
    pub timeout_secs: u64,
    /// 作業フォルダ（絶対パス）。`None` なら `{workspace}`。
    ///
    /// **検査は実行時のみ。** 読み込みで弾くと、村を配った先で存在しないパスを
    /// 指しているだけでスケジュール全体が読めなくなる。
    #[serde(default)]
    pub cwd: Option<String>,
}

/// `timeoutSecs` の既定値。
fn timeout_default() -> u64 {
    PROBE_TIMEOUT_DEFAULT
}

/// 配送をどの会話へ積むか。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionMode {
    /// いまの会話に続ける（既定）。
    ///
    /// **意味は「切り替えを起こさない」であって「旧セッションを指名する」では
    /// ない。** 同じ tick に [`SessionMode::Fresh`] の予定が混ざっていれば、
    /// この予定の配送も新しい会話に積まれる。
    #[default]
    Continue,
    /// 新しい会話を起こしてから積む。
    Fresh,
}

/// 前判定として受け付けられない値。
///
/// UI で防ぐだけでは `schedules.json` を手で編集された時に入るので、
/// **読み込みでも弾く**（[`crate::schedule::InvalidRecurrence`] と同じ棚）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidProbe {
    /// `command` が空、または空白のみ。
    ///
    /// 弾かないと「起動できないまま毎回不一致で消化される予定」になる。
    /// 会話ログにもダイアログにも出ない**静かな死**なので、入口で止める。
    #[error("command は空にできません")]
    EmptyCommand,
    /// `expect` が空、または空白のみ。
    ///
    /// 空を許すと「何も出力しないスクリプト」と一致してしまい、
    /// 無出力＝異常という最も普通の失敗形を検知できなくなる。
    #[error("expect は空にできません")]
    EmptyExpect,
    /// `timeoutSecs` が 0、または上限超過。
    #[error("timeoutSecs は 1〜{PROBE_TIMEOUT_MAX} である必要があります（受け取った値: {0}）")]
    TimeoutOutOfRange(u64),
}

impl ScheduleProbe {
    /// 読み込み時の検証。
    ///
    /// # Errors
    /// `command` / `expect` が空、または `timeoutSecs` が値域外の場合。
    pub fn validate(&self) -> Result<(), InvalidProbe> {
        if self.command.trim().is_empty() {
            return Err(InvalidProbe::EmptyCommand);
        }
        if self.expect.trim().is_empty() {
            return Err(InvalidProbe::EmptyExpect);
        }
        if self.timeout_secs == 0 || self.timeout_secs > PROBE_TIMEOUT_MAX {
            return Err(InvalidProbe::TimeoutOutOfRange(self.timeout_secs));
        }
        Ok(())
    }

    /// 承認鍵（`SHA-256` の 16 進小文字 64 桁）。
    ///
    /// **束縛の実体は村の識別子であってパスではない。** この村の workspace は
    /// `{app_data_dir}/workspace` に固定されており、村の入れ替えは「別のパスを
    /// 開く」ではなく**同じパスの中身の差し替え**として起きる。だから絶対パスを
    /// salt にしても同一機では常に同じ値になり、差し替え攻撃を 1 件も止められない。
    ///
    /// `cwd` は**書かれた文字列のまま**入れる（`None` は JSON の `null`）。
    /// 実行時の解決値を入れると、同じ設定が環境ごとに別の鍵になる。
    pub fn approval_key(&self, village_id: &str) -> String {
        // canonical JSON: キーは辞書順・空白なし。serde_json の Map は
        // 既定で挿入順を保つので、**並べる順序をここで固定する**
        // （BTreeMap に頼ると、欄が増えたとき辞書順の再確認が要る）。
        let canonical = serde_json::json!({
            "args": self.args,
            "command": self.command,
            "cwd": self.cwd,
            "villageId": village_id,
        });
        // to_string は compact（空白なし）。
        let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
        let digest = Sha256::digest(&bytes);
        digest.iter().fold(String::with_capacity(64), |mut acc, b| {
            use std::fmt::Write as _;
            let _ = write!(acc, "{b:02x}");
            acc
        })
    }
}

/// stdout を読んだ結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Judgement {
    /// 1 行目が合図と一致した。付記に回す本文つき。
    Match {
        /// 2 行目以降（原文保持・上限で切り詰め済み・注記つき）。空なら付記しない。
        appendix: String,
    },
    /// 1 行目が合図と一致しなかった。
    NoMatch,
    /// **1 行目が取り込み上限の中で終わらなかった。**
    ///
    /// 切り詰めた 1 行目で照合すると誤判定するので、判定そのものを不成立にする。
    SignalTruncated,
}

/// stdout を読んで判定する。
///
/// **`exit_code` を受け取らないのは意図** — 判定に使わないことを、引数に無い形で
/// 示している（受け取ってから無視する実装は、次に読む人が「使い忘れ」と読む）。
///
/// 行の扱い:
/// - 行区切りは `\n`。行末直前の `\r` は落とす（`\r\n` を同じに扱う）
/// - 1 行目 = 最初の改行まで。**先頭が空行なら 1 行目は空文字**
/// - **トリムは 1 行目だけ。2 行目以降は原文保持**（JSON のインデントを壊さない）
pub fn judge(stdout: &str, expect: &str) -> Judgement {
    // 取り込み上限。超えた分は捨てるが、**捨てたことを覚えておく**。
    let (taken, truncated) = take_chars(stdout, PROBE_STDOUT_CHARS);

    let (first, rest) = match taken.find('\n') {
        Some(at) => (&taken[..at], Some(&taken[at + 1..])),
        None => (taken.as_str(), None),
    };

    // 改行が 1 つも無いまま上限に達した = 合図が最後まで読めていない。
    if rest.is_none() && truncated {
        return Judgement::SignalTruncated;
    }

    if first.trim_end_matches('\r').trim() != expect.trim() {
        return Judgement::NoMatch;
    }

    let appendix = rest.map_or_else(String::new, |rest| appendix_of(rest, truncated));
    Judgement::Match { appendix }
}

/// 2 行目以降を付記の形へ整える。
///
/// 上限を超えたら切って母数を添える。**`N` は切る前の元の字数**で、注記そのものは
/// 上限の外に置く（含めると本文の上限が注記の桁数で揺れる）。
///
/// `intake_truncated` が真のとき、真の総数は**分からない** — 取り込みの時点で
/// 既に捨てているため。だから確定値を書かず「以上」と書く。
/// **知らない数を知っているように書かない。**
fn appendix_of(rest: &str, intake_truncated: bool) -> String {
    let trimmed = rest.trim_end();
    if trimmed.is_empty() {
        return String::new();
    }
    let total = trimmed.chars().count();
    if total <= PROBE_APPENDIX_CHARS && !intake_truncated {
        return trimmed.to_owned();
    }
    let head: String = trimmed.chars().take(PROBE_APPENDIX_CHARS).collect();
    let measure = if intake_truncated {
        format!("全 {PROBE_STDOUT_CHARS} 字以上")
    } else {
        format!("全 {total} 字")
    };
    format!("{head}\n（{measure}のうち先頭 {PROBE_APPENDIX_CHARS} 字。打ち切りました）")
}

/// 先頭から `limit` 文字を取り、打ち切ったかどうかを返す。
fn take_chars(text: &str, limit: usize) -> (String, bool) {
    let mut taken = String::with_capacity(text.len().min(limit * 4));
    for (index, ch) in text.chars().enumerate() {
        if index == limit {
            return (taken, true);
        }
        taken.push(ch);
    }
    (taken, false)
}

/// 判定 1 回の結末。**計器に出す語彙をそのまま型にしてある。**
///
/// 画面にもログにも同じ語が出るので、文字列を 2 箇所で組み立てない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// 一致した（配送する）。
    Match,
    /// 一致しなかった。
    NoMatch,
    /// 実行そのものが成立しなかった。
    Error(ProbeError),
    /// 打ち切り時間に達した。
    ///
    /// **`Error` の一種にしない** — 1 つの事実を outcome と reason の 2 欄で
    /// 言うことになる。timeout は outcome 側で完結させる。
    Timeout,
    /// 端末の承認が無いので実行していない。
    Unapproved,
}

/// 実行が成立しなかった理由。**閉じた列挙**。
///
/// `outcome=error` だけでは「何を直せばよいか」がログから読めない。
/// 直せる沈黙を作らないための欄で、**モデルの書いた文字列は 1 文字も入らない**
/// （`failures.md` #71 — 計器は秘密の転送経路になる）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeError {
    /// `command` が PATH にも実パスにも見つからない。
    NotFound,
    /// 起動そのものに失敗した（権限・実行形式など）。
    SpawnFailed,
    /// `cwd` が存在しない、またはフォルダでない。
    CwdMissing,
    /// 1 行目が取り込み上限の中で終わらなかった（[`Judgement::SignalTruncated`]）。
    SignalTruncated,
}

impl ProbeOutcome {
    /// 計器に出す語。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Match => "match",
            Self::NoMatch => "no_match",
            Self::Error(_) => "error",
            Self::Timeout => "timeout",
            Self::Unapproved => "unapproved",
        }
    }

    /// 計器に出す失敗理由。**値を持つのは [`Self::Error`] のときだけ。**
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Error(err) => err.as_str(),
            _ => "-",
        }
    }

    /// 配送してよいか。
    pub fn delivers(&self) -> bool {
        matches!(self, Self::Match)
    }
}

impl ProbeError {
    /// 計器に出す語。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::SpawnFailed => "spawn_failed",
            Self::CwdMissing => "cwd_missing",
            Self::SignalTruncated => "signal_truncated",
        }
    }
}

/// 依頼文へ前判定の出力を添える。
///
/// **付記しないとサーヴァントが同じ情報を取りに行く周回が発生し、
/// トークン節約という目的と逆行する**（Spec 28 D3）。
pub fn compose_body(base: &str, appendix: &str) -> String {
    if appendix.is_empty() {
        return base.to_owned();
    }
    format!("{base}\n\n【前判定の出力】\n{appendix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(expect: &str) -> ScheduleProbe {
        ScheduleProbe {
            command: "python".to_owned(),
            args: vec!["watch.py".to_owned()],
            expect: expect.to_owned(),
            timeout_secs: PROBE_TIMEOUT_DEFAULT,
            cwd: None,
        }
    }

    #[test]
    fn the_first_line_decides_and_the_rest_becomes_the_appendix() {
        let Judgement::Match { appendix } = judge("CHANGED\n3 件増えました\n", "CHANGED") else {
            panic!("一致するはず");
        };
        assert_eq!(appendix, "3 件増えました");
    }

    #[test]
    fn a_lone_signal_matches_with_an_empty_appendix() {
        // 合図しか出さないスクリプトが最も普通の形。
        assert_eq!(judge("TRUE\n", "TRUE"), Judgement::Match {
            appendix: String::new()
        });
        assert_eq!(judge("TRUE", "TRUE"), Judgement::Match {
            appendix: String::new()
        });
    }

    #[test]
    fn crlf_is_read_the_same_as_lf() {
        // Windows のスクリプトが主戦場。\r を残すと一致が原理的に成立しない。
        let Judgement::Match { appendix } = judge("CHANGED\r\n本文\r\n", "CHANGED") else {
            panic!("CRLF でも一致するはず");
        };
        assert_eq!(appendix, "本文");
    }

    #[test]
    fn only_the_first_line_is_trimmed() {
        // 2 行目以降の先頭空白を落とすと、付記した JSON のインデントが壊れる。
        let Judgement::Match { appendix } = judge("  CHANGED  \n  {\n    \"a\": 1\n  }\n", "CHANGED")
        else {
            panic!("前後の空白は落として一致するはず");
        };
        assert_eq!(appendix, "  {\n    \"a\": 1\n  }");
    }

    #[test]
    fn a_leading_blank_line_means_an_empty_first_line() {
        // 「合図は最初の行に書く」を守らせる。2 行目を繰り上げると、
        // どの行が合図かがスクリプト側から読めなくなる。
        assert_eq!(judge("\nCHANGED\n", "CHANGED"), Judgement::NoMatch);
    }

    #[test]
    fn a_different_signal_does_not_match() {
        assert_eq!(judge("UNCHANGED\n", "CHANGED"), Judgement::NoMatch);
        assert_eq!(judge("", "CHANGED"), Judgement::NoMatch);
    }

    #[test]
    fn a_signal_that_never_ends_is_not_a_judgement() {
        // 改行が来ないまま上限に達したら、切り詰めた 1 行目で照合しない。
        let flood = "x".repeat(PROBE_STDOUT_CHARS + 10);
        assert_eq!(judge(&flood, "x"), Judgement::SignalTruncated);
    }

    #[test]
    fn truncation_after_the_first_line_still_judges() {
        // 打ち切りが 2 行目以降なら判定は成立し、付記だけが切られる。
        let mut out = String::from("CHANGED\n");
        out.push_str(&"あ".repeat(PROBE_STDOUT_CHARS));
        let Judgement::Match { appendix } = judge(&out, "CHANGED") else {
            panic!("1 行目は読み切れているので判定は成立するはず");
        };
        assert!(appendix.contains("以上"), "真の総数は分からないので「以上」と書く: {appendix}");
        assert!(!appendix.contains(&format!("全 {PROBE_STDOUT_CHARS} 字のうち")));
    }

    #[test]
    fn the_appendix_reports_the_original_length_before_cutting() {
        // N は切る前の元の字数。切り詰め後の字数を書くと母数の意味が消える。
        let mut out = String::from("CHANGED\n");
        let body_chars = PROBE_APPENDIX_CHARS + 500;
        out.push_str(&"あ".repeat(body_chars));
        let Judgement::Match { appendix } = judge(&out, "CHANGED") else {
            panic!("一致するはず");
        };
        assert!(appendix.contains(&format!("全 {body_chars} 字")), "{appendix}");
    }

    #[test]
    fn japanese_is_counted_in_characters_not_bytes() {
        // len() で実装すると日本語は 1/3 の位置で切れる。
        let mut out = String::from("CHANGED\n");
        out.push_str(&"あ".repeat(PROBE_APPENDIX_CHARS));
        let Judgement::Match { appendix } = judge(&out, "CHANGED") else {
            panic!("一致するはず");
        };
        // ちょうど上限なので注記は付かず、全文が残る。
        assert_eq!(appendix.chars().count(), PROBE_APPENDIX_CHARS);
        assert!(!appendix.contains("打ち切りました"));
    }

    #[test]
    fn an_empty_command_or_expect_is_rejected_at_load() {
        let mut p = probe("CHANGED");
        p.command = "  ".to_owned();
        assert_eq!(p.validate(), Err(InvalidProbe::EmptyCommand));

        let p = probe("   ");
        assert_eq!(p.validate(), Err(InvalidProbe::EmptyExpect));

        let mut p = probe("CHANGED");
        p.timeout_secs = 0;
        assert_eq!(p.validate(), Err(InvalidProbe::TimeoutOutOfRange(0)));
        p.timeout_secs = PROBE_TIMEOUT_MAX + 1;
        assert!(matches!(p.validate(), Err(InvalidProbe::TimeoutOutOfRange(_))));

        assert_eq!(probe("CHANGED").validate(), Ok(()));
    }

    #[test]
    fn the_approval_key_is_stable_for_the_same_line_in_the_same_village() {
        let p = probe("CHANGED");
        let key = p.approval_key("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
        assert_eq!(key, p.approval_key("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"));
        assert_eq!(key.len(), 64, "SHA-256 の 16 進 64 桁");
        assert!(key.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));
    }

    #[test]
    fn the_village_changes_the_key() {
        // **これが差し替え攻撃を止めている 1 本。** 同じコマンド行でも、
        // 攻撃者の村は別の識別子を運ぶので承認が一致しない。
        let p = probe("CHANGED");
        assert_ne!(
            p.approval_key("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
            p.approval_key("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb")
        );
    }

    #[test]
    fn every_part_of_the_command_line_changes_the_key() {
        let village = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
        let base = probe("CHANGED").approval_key(village);

        let mut p = probe("CHANGED");
        p.command = "python3".to_owned();
        assert_ne!(base, p.approval_key(village), "command");

        let mut p = probe("CHANGED");
        p.args.push("--verbose".to_owned());
        assert_ne!(base, p.approval_key(village), "args の要素");

        let mut p = probe("CHANGED");
        p.args = vec!["b.py".to_owned(), "a.py".to_owned()];
        let mut q = probe("CHANGED");
        q.args = vec!["a.py".to_owned(), "b.py".to_owned()];
        assert_ne!(
            p.approval_key(village),
            q.approval_key(village),
            "args の順序は意味を持つ"
        );

        let mut p = probe("CHANGED");
        p.cwd = Some("D:/work".to_owned());
        assert_ne!(base, p.approval_key(village), "cwd");
    }

    #[test]
    fn an_unset_cwd_is_not_an_empty_cwd() {
        // null と "" を同じ鍵に潰すと、「未指定」という意味が消える。
        let village = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
        let mut empty = probe("CHANGED");
        empty.cwd = Some(String::new());
        assert_ne!(probe("CHANGED").approval_key(village), empty.approval_key(village));
    }

    #[test]
    fn expect_and_timeout_do_not_change_the_key() {
        // **承認したのは「何が走るか」であって「何を合図と見るか」ではない。**
        // 合図の変更や待ち時間の調整で承認が外れると、利用者は理由の分からない
        // unapproved を踏む。走る中身（command / args / cwd / 村）だけが鍵。
        let village = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
        let base = probe("CHANGED").approval_key(village);

        assert_eq!(base, probe("UPDATED").approval_key(village));

        let mut p = probe("CHANGED");
        p.timeout_secs = 120;
        assert_eq!(base, p.approval_key(village));
    }

    #[test]
    fn the_outcome_reason_is_only_set_for_errors() {
        // timeout は outcome 側で完結する。1 つの事実を 2 欄で言わない。
        assert_eq!(ProbeOutcome::Timeout.as_str(), "timeout");
        assert_eq!(ProbeOutcome::Timeout.reason(), "-");
        assert_eq!(ProbeOutcome::Match.reason(), "-");
        assert_eq!(ProbeOutcome::NoMatch.reason(), "-");
        assert_eq!(ProbeOutcome::Unapproved.reason(), "-");
        assert_eq!(
            ProbeOutcome::Error(ProbeError::CwdMissing).reason(),
            "cwd_missing"
        );
        assert_eq!(ProbeOutcome::Error(ProbeError::NotFound).as_str(), "error");
    }

    #[test]
    fn only_a_match_delivers() {
        assert!(ProbeOutcome::Match.delivers());
        for outcome in [
            ProbeOutcome::NoMatch,
            ProbeOutcome::Timeout,
            ProbeOutcome::Unapproved,
            ProbeOutcome::Error(ProbeError::SpawnFailed),
        ] {
            assert!(!outcome.delivers(), "{outcome:?} は配送しない");
        }
    }

    #[test]
    fn the_body_carries_the_appendix_only_when_there_is_one() {
        assert_eq!(compose_body("【定期実行: 5 分ごと】\n見て", ""), "【定期実行: 5 分ごと】\n見て");
        assert_eq!(
            compose_body("依頼", "3 件"),
            "依頼\n\n【前判定の出力】\n3 件"
        );
    }

    #[test]
    fn the_default_session_mode_is_continue() {
        assert_eq!(SessionMode::default(), SessionMode::Continue);
    }
}
