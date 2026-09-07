//! LLM 再試行の純関数（Spec 52）。
//!
//! **I/O も時計も乱数も読まない** — 現在時刻は引数で受ける（`schedule.rs` と同じ規律。
//! 内部で `SystemTime::now()` を読むと、テストが壁時計に依存して特定の時刻でだけ落ちる）。
//!
//! 構造は busbar（github.com/GetBusbar/busbar・Apache-2.0）の
//! `crates/busbar/src/breaker.rs` の `parse_retry_after` を写した。コードは書き直している。
//!
//! **P0（計器）の範囲はこのファイルの `parse_retry_after` と [`HintSource`] だけ。**
//! 分類（`RetryClass` / `Verdict`）と待ちの式（`plan_wait`）は P1 で足す。

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 明示された待ち時間がどこから来たか。計器 `llm retry:` の `src=` に出す。
///
/// `None` と「ヘッダは付いていたが過去の日付」（`Some(0)`）を読み分けるための欄で、
/// 待ちの計算には使わない（計算は [`crate::llm::LlmError::Api`] の `retry_after` が担う）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintSource {
    /// ヘッダにも本文にも無かった。
    None,
    /// `Retry-After` ヘッダだけ。
    Header,
    /// 本文（Gemini の `google.rpc.RetryInfo`）だけ。P1 から。
    Body,
    /// 両方（大きいほうを採る）。P1 から。
    Both,
}

impl HintSource {
    /// ヘッダと本文それぞれの有無から決める。
    pub fn from_presence(header: bool, body: bool) -> Self {
        match (header, body) {
            (false, false) => Self::None,
            (true, false) => Self::Header,
            (false, true) => Self::Body,
            (true, true) => Self::Both,
        }
    }

    /// ログの `src=` に出す語。無ければ `-`（他の計器の欄と同じ作法）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "-",
            Self::Header => "header",
            Self::Body => "body",
            Self::Both => "both",
        }
    }
}

/// `Retry-After` ヘッダの値を待ち時間へ。
///
/// RFC 9110 §10.2.3 は `delay-seconds / HTTP-date` の**両方**を正としており、
/// プロバイダは両方を送る。整数だけ読むと、日付形の応答ではサーバーが明示した
/// 下限を黙って捨てることになる。
///
/// **過去の日付は `Some(0)`**（「今すぐ」であって「とても長い」ではない）。`None` は
/// 「読めなかった」— ヘッダが無いときも呼び出し側が `None` を作るので、計器では
/// [`HintSource`] で区別する。HTTP-date は IMF-fixdate（`Sun, 06 Nov 1994 08:49:37 GMT`）を
/// 読む。RFC 850 と asctime の旧形式は読まない（現行のプロバイダが送る形ではない）。
pub fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let s = value.trim();
    if let Ok(secs) = s.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let at = chrono::DateTime::parse_from_rfc2822(s).ok()?;
    let now_unix = i64::try_from(now.duration_since(UNIX_EPOCH).ok()?.as_secs()).ok()?;
    let remaining = at.timestamp().saturating_sub(now_unix).max(0);
    Some(Duration::from_secs(remaining as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(unix: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(unix)
    }

    #[test]
    fn delay_seconds_is_read_as_is_and_trimmed() {
        let now = at(1_000_000);
        assert_eq!(parse_retry_after("7", now), Some(Duration::from_secs(7)));
        assert_eq!(parse_retry_after("  56 ", now), Some(Duration::from_secs(56)));
        assert_eq!(parse_retry_after("0", now), Some(Duration::ZERO));
    }

    /// HTTP-date は「今から何秒後か」へ落とす。基準時刻は引数で受けるので固定できる。
    #[test]
    fn http_date_in_the_future_is_the_remaining_seconds() {
        // 1994-11-06 08:49:37 GMT = 784_111_777（RFC 9110 の例文そのもの）。
        let now = at(784_111_777 - 90);
        assert_eq!(
            parse_retry_after("Sun, 06 Nov 1994 08:49:37 GMT", now),
            Some(Duration::from_secs(90))
        );
    }

    /// 過去の日付は 0 = 今すぐ。`None`（読めない）と区別する。
    #[test]
    fn http_date_in_the_past_is_zero_not_none() {
        let now = at(784_111_777 + 3600);
        assert_eq!(
            parse_retry_after("Sun, 06 Nov 1994 08:49:37 GMT", now),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn unreadable_values_are_none() {
        let now = at(1_000_000);
        assert_eq!(parse_retry_after("", now), None);
        assert_eq!(parse_retry_after("soon", now), None);
        assert_eq!(parse_retry_after("-5", now), None);
        assert_eq!(parse_retry_after("1.5", now), None);
    }

    #[test]
    fn hint_source_reads_both_flags() {
        assert_eq!(HintSource::from_presence(false, false).as_str(), "-");
        assert_eq!(HintSource::from_presence(true, false).as_str(), "header");
        assert_eq!(HintSource::from_presence(false, true).as_str(), "body");
        assert_eq!(HintSource::from_presence(true, true).as_str(), "both");
    }
}
