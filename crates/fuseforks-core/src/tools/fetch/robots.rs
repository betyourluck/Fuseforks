//! robots.txt の規則と、origin ごとに覚えておく棚（Spec 78 D7 / `web_fetch_contract` 13）。
//!
//! **取りに行くのは `mod.rs`**（HTTP はそこにだけ置く）。ここは状態コードの振り分けと、解析した規則を
//! 1 時間覚える棚だけ。解析は robotxt 0.7 の `Robots::parse` だけを使う — robotxt の `AccessResult` は
//! RFC 9309 どおり 4xx を全部許すので、401 / 403 を断る上流の main の規則と違う。状態コードは自前で振り分ける。
//!
//! robotstxt 0.3（Google の C++ 判定器の移植）は使わない — 1 行が 16,664 バイトを超えた先に多バイト文字が
//! あると、文字の途中で切って panic する（P0a で再現）。配布版は panic で落ちる設定なので、外のサイトの
//! robots.txt 1 本でアプリが落ちる。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// robots.txt で照合する名前（User-Agent の製品名）。
pub const AGENT: &str = "Fuseforks";

/// 解析した規則を覚えておく長さ（プロセスの寿命。保存しない）。
pub const KEEP_FOR: Duration = Duration::from_secs(60 * 60);

/// 読む長さ（Google と同じ 500 KiB + 1 バイト。robotxt は 500 KiB の中の最後の行末で切る）。
pub const READ_LIMIT: usize = robotxt::BYTE_LIMIT + 1;

/// robots.txt の応答の状態コードの振り分け（上流の main と同じ）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusRule {
    /// 2xx — 本文を解析する。
    Parse,
    /// 401 / 403 以外の 4xx — 規則が無いとして通す。
    AllowAll,
    /// 401 / 403 / 5xx ほか — 断る（`robots_unavailable`）。
    Refuse,
}

/// 状態コードから振り分ける。
pub fn status_rule(status: u16) -> StatusRule {
    match status {
        200..=299 => StatusRule::Parse,
        401 | 403 => StatusRule::Refuse,
        400..=499 => StatusRule::AllowAll,
        _ => StatusRule::Refuse,
    }
}

/// 規則を解析する。照合する名前は [`AGENT`]。
pub fn parse(body: &[u8]) -> robotxt::Robots {
    robotxt::Robots::parse(body, AGENT)
}

/// origin（スキーム・ホスト・ポート）ごとに、解析した規則を 1 時間覚える棚。
///
/// **覚えるのは規則が決まったときだけ**（2xx の解析と、規則なしの 4xx）。断ったとき（401 / 403 / 5xx /
/// 繋がらない）は覚えない — 一時的な失敗で 1 時間そのサイトを読めなくしないため。
#[derive(Default)]
pub struct RobotsShelf {
    entries: Mutex<HashMap<String, (Instant, Arc<robotxt::Robots>)>>,
}

impl RobotsShelf {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, (Instant, Arc<robotxt::Robots>)>> {
        // 毒されても使う（棚は覚え書きで、壊れても取り直せば済む）。
        self.entries.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 覚えている規則（`now` から見て 1 時間以内のものだけ）。
    pub fn get(&self, origin: &str, now: Instant) -> Option<Arc<robotxt::Robots>> {
        let mut entries = self.lock();
        match entries.get(origin) {
            Some((at, rules)) if now.duration_since(*at) < KEEP_FOR => Some(Arc::clone(rules)),
            Some(_) => {
                entries.remove(origin);
                None
            }
            None => None,
        }
    }

    /// 覚える。
    pub fn put(&self, origin: &str, rules: Arc<robotxt::Robots>, now: Instant) {
        self.lock().insert(origin.to_owned(), (now, rules));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_are_sorted_like_upstream() {
        assert_eq!(status_rule(200), StatusRule::Parse);
        assert_eq!(status_rule(404), StatusRule::AllowAll);
        assert_eq!(status_rule(410), StatusRule::AllowAll);
        assert_eq!(status_rule(401), StatusRule::Refuse);
        assert_eq!(status_rule(403), StatusRule::Refuse);
        assert_eq!(status_rule(500), StatusRule::Refuse);
        assert_eq!(status_rule(503), StatusRule::Refuse);
    }

    #[test]
    fn rules_name_fuseforks_case_insensitively() {
        let rules = parse(b"User-agent: fuseforks\nDisallow: /private\n\nUser-agent: *\nDisallow: /\n");
        assert!(rules.is_allowed("/public"), "自分の名前の群が * より優先");
        assert!(!rules.is_allowed("/private/x"));
        let star = parse(b"User-agent: *\nDisallow: /\n");
        assert!(!star.is_allowed("/anything"));
        assert!(star.is_allowed("/robots.txt"));
    }

    /// robotstxt 0.3 が panic した入力（P0a）。robotxt は通る。
    #[test]
    fn a_long_line_with_multibyte_characters_does_not_panic() {
        let body = format!("User-agent: *\nDisallow: /{}あ\nDisallow: /x\n", "a".repeat(16_700));
        let _ = parse(body.as_bytes()).is_allowed("/x");
    }

    #[test]
    fn the_shelf_forgets_after_an_hour() {
        let shelf = RobotsShelf::default();
        let t0 = Instant::now();
        shelf.put("https://a.test", Arc::new(robotxt::Robots::allow_all()), t0);
        assert!(shelf.get("https://a.test", t0 + Duration::from_secs(60 * 59)).is_some());
        assert!(shelf.get("https://a.test", t0 + KEEP_FOR).is_none());
        assert!(shelf.get("https://b.test", t0).is_none());
    }
}
