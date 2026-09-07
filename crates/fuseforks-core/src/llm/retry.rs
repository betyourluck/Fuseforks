//! LLM 再試行の純関数（Spec 52）。
//!
//! **I/O も時計も乱数も読まない** — 現在時刻と jitter の乱数は引数で受ける
//! （`schedule.rs` と同じ規律。内部で `SystemTime::now()` を読むと、テストが壁時計に
//! 依存して特定の時刻でだけ落ちる）。
//!
//! 構造は busbar（github.com/GetBusbar/busbar・Apache-2.0）の
//! `crates/busbar/src/breaker.rs`（`StatusClass` / `classify` / `parse_retry_after`）と
//! `store/in_memory/breaker.rs`（`compute_cooldown_with_retry_after`）を写した。
//! コードは書き直している。**あちらと違う点は 2 つ** — 天井を超えた明示値は
//! クランプせず止める（[`WaitPlan::StopHintTooLong`]）/ jitter は `max` の後に上向きだけ
//! （明示値を下回らない）。理由は Spec 52 D3。
//!
//! 使う側は `client.rs` の `chat_with_backoff` ただ 1 箇所。

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
    /// 本文（Gemini の `google.rpc.RetryInfo`）だけ。
    Body,
    /// 両方（大きいほうを採る）。
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

/// adapter がエラー本文から取り出す材料。**client はこれを読むだけで JSON を解釈しない。**
///
/// 各 adapter の `error_signal(body)` が返す。表に無いプロバイダ（本文を読まないワイヤ）は
/// [`ErrorSignal::default`] を返し、分類は status 既定へ落ちる。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ErrorSignal {
    /// プロバイダの機械可読コード（OpenAI 系の `error.code` / `error.type`、Anthropic の
    /// `error.type`、Gemini の `error.status`）。`classify` の表と突き合わせる。
    pub code: Option<String>,
    /// 本文に書かれた待ち時間（Gemini の `google.rpc.RetryInfo.retryDelay`）。
    pub retry_after: Option<Duration>,
}

/// 再試行の分類。**閉じた 9 値**で、`verdict` は網羅 match（`_ =>` を書かない）。
///
/// 増やすときはこの enum と [`verdict`] と `as_str` の 3 箇所が同時に落ちる。
/// 408 は `ClientError` に据え置く（2026-09-07 裁定。実機 0 件・`Timeout` は reqwest の
/// `is_timeout` で既に覆われている）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryClass {
    /// 429。
    RateLimit,
    /// 529（Anthropic 固有。IANA 未登録だが文書化されている）。
    Overloaded,
    /// 5xx（529 を除く）。
    ServerError,
    /// reqwest の `is_timeout`。
    Timeout,
    /// reqwest の `is_connect` / `is_request`。
    Network,
    /// 401 / 403。
    Auth,
    /// プロバイダ code（OpenAI 系の `insufficient_quota`）。status に関わらず止める。
    Billing,
    /// 上記以外の 4xx（408 を含む）と、2xx / 3xx がエラー経路へ来た形。
    ClientError,
    /// 400 / 413 かつプロバイダ code が `context_length_exceeded`。
    ContextLength,
}

impl RetryClass {
    /// ログの `class=` に出す語（snake_case）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RateLimit => "rate_limit",
            Self::Overloaded => "overloaded",
            Self::ServerError => "server_error",
            Self::Timeout => "timeout",
            Self::Network => "network",
            Self::Auth => "auth",
            Self::Billing => "billing",
            Self::ClientError => "client_error",
            Self::ContextLength => "context_length",
        }
    }
}

/// 再試行するか、止めるか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 待って再送する。
    Retry,
    /// 同じ入力を再送しても回復しない。即返す。
    Stop,
}

/// Anthropic の過負荷。IANA 未登録。
const HTTP_OVERLOADED: u16 = 529;

/// OpenAI 系が課金切れに付ける code。429 で返る。
const CODE_INSUFFICIENT_QUOTA: &str = "insufficient_quota";

/// OpenAI 系が文脈超過に付ける code。400 で返る。
const CODE_CONTEXT_LENGTH: &str = "context_length_exceeded";

/// HTTP status とプロバイダ code から分類する。
///
/// **code が表にあれば status より優先**（busbar の `error_map` と同じ向き）。
/// `context_length_exceeded` だけは **400 / 413 のときに限る** — 5xx の本文が同じ code を
/// 運んできても、それは上流の障害であって文脈超過ではない。表に無い code は status 既定へ。
pub fn classify(status: u16, code: Option<&str>) -> RetryClass {
    if let Some(code) = code {
        if code == CODE_INSUFFICIENT_QUOTA {
            return RetryClass::Billing;
        }
        if code == CODE_CONTEXT_LENGTH && (status == 400 || status == 413) {
            return RetryClass::ContextLength;
        }
    }
    match status {
        401 | 403 => RetryClass::Auth,
        429 => RetryClass::RateLimit,
        HTTP_OVERLOADED => RetryClass::Overloaded,
        500..=599 => RetryClass::ServerError,
        // 408 を含む。2xx / 3xx がここへ来るのは base_url の設定違い（リダイレクトを
        // 追わなかった等）で、レーンの罪ではないが再送しても同じなので止める。
        _ => RetryClass::ClientError,
    }
}

/// 分類から判定へ。網羅 match — 値を足したらここが落ちる。
pub fn verdict(class: RetryClass) -> Verdict {
    match class {
        RetryClass::RateLimit
        | RetryClass::Overloaded
        | RetryClass::ServerError
        | RetryClass::Timeout
        | RetryClass::Network => Verdict::Retry,
        RetryClass::Auth
        | RetryClass::Billing
        | RetryClass::ClientError
        | RetryClass::ContextLength => Verdict::Stop,
    }
}

/// 明示された待ち時間の天井。これを超える要求には従わず、再試行せず止める（D3）。
///
/// **code constant**（`budget.rs` の重みと同じ扱い。設定にするなら頻度を見てから）。
/// 60 秒の根拠は実機の 2 件が 56 / 51 秒（分単位の quota 窓）で、これを通す最小の切り。
pub const MAX_HONORED_RETRY_AFTER: Duration = Duration::from_secs(60);

/// 指数部の基底（0 回目の失敗の後）。
const BACKOFF_BASE: Duration = Duration::from_millis(200);
/// 指数部の天井。明示値には掛からない。
const BACKOFF_CAP: Duration = Duration::from_secs(5);
/// jitter の幅（上向きだけ。`base × (1 + JITTER × u)`）。
const JITTER: f64 = 0.1;

/// `plan_wait` の答え。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitPlan {
    /// この時間だけ待って再送する。
    Wait(Duration),
    /// 明示値が天井を超えた。再送せず、本文へ秒数を書いて返す。
    StopHintTooLong(Duration),
}

/// 待ち時間を決める（D3。順序が本体）。
///
/// 1. `exp = min(200ms × 2^attempt, 5s)` — `attempt` は **0 始まり**（0 回目の失敗の後が 200 ms）
/// 2. `hint > 60s` なら [`WaitPlan::StopHintTooLong`] — **jitter を掛ける前の値で判定する**
///    （逆にすると 56 秒 × 1.1 = 61.6 秒が天井に当たって止まる）
/// 3. `base = max(exp, hint)` — 明示値は下限
/// 4. `wait = base × (1 + 0.1 × u)`、`u ∈ [0, 1]` — **jitter は `max` の後に上向きだけ。**
///    `max` の前に掛けると明示値が勝った瞬間に jitter が消え、波で同時に落ちた個体が
///    同じ ms で揃う。上向きだけなのは明示値を下回らないため。jitter 後の最大は 66 秒
pub fn plan_wait(attempt: u32, hint: Option<Duration>, u: f64) -> WaitPlan {
    let shift = attempt.min(31);
    let exp = BACKOFF_BASE
        .checked_mul(1u32 << shift)
        .unwrap_or(BACKOFF_CAP)
        .min(BACKOFF_CAP);
    if let Some(hint) = hint {
        if hint > MAX_HONORED_RETRY_AFTER {
            return WaitPlan::StopHintTooLong(hint);
        }
    }
    let base = hint.map_or(exp, |h| exp.max(h));
    let u = if u.is_finite() { u.clamp(0.0, 1.0) } else { 0.0 };
    WaitPlan::Wait(base.mul_f64(1.0 + JITTER * u))
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

/// protobuf `Duration` の JSON 形（`"56s"` / `"0.5s"` / `"3.000000001s"`）を読む。
///
/// Gemini の `google.rpc.RetryInfo.retryDelay` がこの形。負・NaN・単位違いは `None`。
pub fn parse_proto_duration(value: &str) -> Option<Duration> {
    let s = value.trim().strip_suffix('s')?;
    let secs: f64 = s.parse().ok()?;
    if !secs.is_finite() || secs < 0.0 {
        return None;
    }
    Some(Duration::from_secs_f64(secs))
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

    #[test]
    fn proto_duration_reads_gemini_retry_delay() {
        assert_eq!(parse_proto_duration("56s"), Some(Duration::from_secs(56)));
        assert_eq!(parse_proto_duration("0.5s"), Some(Duration::from_millis(500)));
        assert_eq!(
            parse_proto_duration("3.000000001s"),
            Some(Duration::from_secs_f64(3.000000001))
        );
        assert_eq!(parse_proto_duration("56"), None, "単位が無い");
        assert_eq!(parse_proto_duration("-1s"), None, "負");
        assert_eq!(parse_proto_duration("soon"), None);
    }

    /// status 既定の表。busbar の `normalize_raw_error` の Step 2 と同じ並び。
    #[test]
    fn status_defaults_follow_the_table() {
        assert_eq!(classify(429, None), RetryClass::RateLimit);
        assert_eq!(classify(529, None), RetryClass::Overloaded);
        assert_eq!(classify(500, None), RetryClass::ServerError);
        assert_eq!(classify(503, None), RetryClass::ServerError);
        assert_eq!(classify(401, None), RetryClass::Auth);
        assert_eq!(classify(403, None), RetryClass::Auth);
        assert_eq!(classify(400, None), RetryClass::ClientError);
        assert_eq!(classify(404, None), RetryClass::ClientError);
        assert_eq!(classify(408, None), RetryClass::ClientError, "408 は据え置き（裁定）");
        assert_eq!(classify(302, None), RetryClass::ClientError, "2xx/3xx が来ても止める");
    }

    /// code は status より優先。ただし文脈超過は 400 / 413 のときだけ。
    #[test]
    fn provider_codes_override_the_status() {
        assert_eq!(
            classify(429, Some("insufficient_quota")),
            RetryClass::Billing,
            "OpenAI は課金切れを 429 で返す"
        );
        assert_eq!(classify(400, Some("context_length_exceeded")), RetryClass::ContextLength);
        assert_eq!(classify(413, Some("context_length_exceeded")), RetryClass::ContextLength);
        assert_eq!(
            classify(500, Some("context_length_exceeded")),
            RetryClass::ServerError,
            "5xx の本文が同じ code を運んでも上流の障害"
        );
        assert_eq!(
            classify(429, Some("rate_limit_exceeded")),
            RetryClass::RateLimit,
            "表に無い code は status 既定へ"
        );
    }

    /// 9 値すべての判定。表を変えたらここが落ちる。
    #[test]
    fn verdict_covers_all_nine_classes() {
        use RetryClass::*;
        for class in [RateLimit, Overloaded, ServerError, Timeout, Network] {
            assert_eq!(verdict(class), Verdict::Retry, "{class:?}");
        }
        for class in [Auth, Billing, ClientError, ContextLength] {
            assert_eq!(verdict(class), Verdict::Stop, "{class:?}");
        }
    }

    #[test]
    fn exponential_part_is_unchanged_from_before() {
        assert_eq!(plan_wait(0, None, 0.0), WaitPlan::Wait(Duration::from_millis(200)));
        assert_eq!(plan_wait(1, None, 0.0), WaitPlan::Wait(Duration::from_millis(400)));
        assert_eq!(plan_wait(4, None, 0.0), WaitPlan::Wait(Duration::from_millis(3200)));
        assert_eq!(plan_wait(5, None, 0.0), WaitPlan::Wait(Duration::from_secs(5)), "5 秒で頭打ち");
        assert_eq!(plan_wait(40, None, 0.0), WaitPlan::Wait(Duration::from_secs(5)), "溢れない");
    }

    /// 明示値は下限。指数部より小さければ指数部が残る。
    #[test]
    fn the_hint_is_a_floor_not_a_replacement() {
        assert_eq!(
            plan_wait(0, Some(Duration::from_secs(56)), 0.0),
            WaitPlan::Wait(Duration::from_secs(56))
        );
        assert_eq!(
            plan_wait(1, Some(Duration::from_millis(100)), 0.0),
            WaitPlan::Wait(Duration::from_millis(400)),
            "指数部が勝つ"
        );
        assert_eq!(
            plan_wait(0, Some(Duration::ZERO), 0.0),
            WaitPlan::Wait(Duration::from_millis(200)),
            "過去の日付（0）は下限として効かない"
        );
    }

    /// jitter は `max` の後に、上向きだけ。明示値が勝っても散る（S5 の根拠）。
    #[test]
    fn jitter_is_applied_after_the_floor_and_only_upward() {
        let hint = Some(Duration::from_secs(56));
        assert_eq!(plan_wait(0, hint, 1.0), WaitPlan::Wait(Duration::from_millis(61_600)));
        assert_eq!(plan_wait(0, hint, 0.5), WaitPlan::Wait(Duration::from_millis(58_800)));
        assert_ne!(plan_wait(0, hint, 0.5), plan_wait(0, hint, 0.0), "同じ hint でも u が違えば違う");
        assert_eq!(plan_wait(0, hint, -3.0), plan_wait(0, hint, 0.0), "u < 0 は 0 へ（下回らない）");
        assert_eq!(plan_wait(0, hint, 7.0), plan_wait(0, hint, 1.0), "u > 1 は 1 へ");
        assert_eq!(plan_wait(0, hint, f64::NAN), plan_wait(0, hint, 0.0));
    }

    /// 天井の判定は jitter の前。60 秒ちょうどは通り（jitter 後 66 秒）、61 秒は止まる。
    #[test]
    fn the_ceiling_is_judged_before_jitter() {
        assert_eq!(
            plan_wait(0, Some(Duration::from_secs(60)), 1.0),
            WaitPlan::Wait(Duration::from_secs(66))
        );
        assert_eq!(
            plan_wait(0, Some(Duration::from_secs(61)), 0.0),
            WaitPlan::StopHintTooLong(Duration::from_secs(61))
        );
        assert_eq!(
            plan_wait(2, Some(Duration::from_secs(3600)), 0.0),
            WaitPlan::StopHintTooLong(Duration::from_secs(3600)),
            "attempt に関わらず止める"
        );
    }
}
