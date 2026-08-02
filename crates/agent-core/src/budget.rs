//! トークン予算（Spec 11）の純機構 — 計量の純関数と予算プール。
//!
//! 契約は `data_contract.yaml` の `token_budget` ブロック（P0 凍結）が正。
//! ここには I/O も配線も無い — 因果の根での生成・封筒への相乗り・周回境界の
//! 検査は P2（orchestrator）の仕事で、本モジュールは数えと原子性だけを持つ。
//!
//! # 内部表現は milli 実効トークン
//!
//! 重み 0.1 を扱うのに浮動小数は使えない（`AtomicF64` は存在しない）。
//! すべて ×1000 の整数（milli）で持ち、境界で切り上げて実効トークンへ戻す。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::llm::Usage;

/// 未キャッシュ入力の重み（milli。= ×1.0）。
pub const WEIGHT_UNCACHED_MILLI: u64 = 1000;
/// キャッシュ済み入力の重み（milli。= ×0.1 — Anthropic の cache read 価格比）。
pub const WEIGHT_CACHED_MILLI: u64 = 100;
/// 出力の重み（milli。= ×4.0）。
pub const WEIGHT_OUTPUT_MILLI: u64 = 4000;

/// 新規 world.json に書き込む既定の天井（実効トークン）。
///
/// 根拠実測（2026-08-02）: 健全な 6 体編成の依頼 1 件 = 実効 ≈ 250K。
/// その約 4 倍で、#41 型の暴走（素 2M）は確実に止まる。
/// **既存の村へは書かない**（契約 ceiling の後方互換規程）。
pub const DEFAULT_CEILING: u64 = 1_000_000;

/// usage を milli 実効トークンへ換算する。
///
/// `未キャッシュ ×1000 + キャッシュ済み ×100 + 出力 ×4000`。
/// `cache_read > prompt` という壊れた usage が来ても引き算で wrap しない
/// （未キャッシュを 0 に飽和）。
pub fn effective_milli(usage: &Usage) -> u64 {
    let uncached = usage.prompt.saturating_sub(usage.cache_read);
    uncached
        .saturating_mul(WEIGHT_UNCACHED_MILLI)
        .saturating_add(usage.cache_read.saturating_mul(WEIGHT_CACHED_MILLI))
        .saturating_add(usage.completion.saturating_mul(WEIGHT_OUTPUT_MILLI))
}

/// usage を実効トークンへ換算する（milli を切り上げ）。
///
/// 切り上げは保守側 — キャッシュ済み 1 トークン（100 milli）も 0 ではなく
/// 1 実効トークンとして数える。
pub fn effective_tokens(usage: &Usage) -> u64 {
    effective_milli(usage).div_ceil(1000)
}

/// usage が欠けた応答のトークン見積もり（切り上げ）。
///
/// 分母は **UTF-8 バイト数**（`s.len()`）であって文字数ではない —
/// `chars().count() ÷ 4` は日本語（≈1〜2 字/トークン）で約 4 倍の過小になり、
/// 「楽観の 0 を作らない」規程に反する。バイトは日本語 3 バイト/字で
/// 過大側に倒れ、O(1) で取れる。適用の三規程（prompt 欠落 = 送信バイト /
/// completion 欠落 = 受信バイト / cached 区分なし = 全量未キャッシュ）は
/// 呼び出し側（P2）が担う。
pub fn estimate_tokens(utf8_len: usize) -> u64 {
    (utf8_len as u64).div_ceil(4)
}

/// usage が報告されなかった応答を、見積もりで埋めた usage へ正規化する。
///
/// 「欠落」の判定は `total() == 0`（canonical の [`Usage`] は adapter が値を
/// 埋めなかったとき全欄 0 の `Default` になり、欄ごとの「無い」と「0」は
/// 区別できない）。実プロバイダは usage を返すので、ここへ落ちるのは
/// 主にテストバックエンドと異常応答。
///
/// - prompt 欠落 → 送信 UTF-8 バイト数 ÷ 4（切り上げ）
/// - completion 欠落 → 受信 UTF-8 バイト数 ÷ 4（切り上げ。0 にしない —
///   ツール結果が支配的なターンで 0 見積もりになる穴）
/// - cached 区分なし → 全量を未キャッシュ扱い（`cache_read = 0` のまま）
pub fn normalized_usage(reported: &Usage, sent_utf8_len: usize, received_utf8_len: usize) -> Usage {
    if reported.total() > 0 {
        return *reported;
    }
    Usage {
        prompt: estimate_tokens(sent_utf8_len),
        completion: estimate_tokens(received_utf8_len),
        cache_read: 0,
    }
}

/// 依頼 1 つの因果が共有する予算プール。
///
/// 根（ユーザー発話の宛先 1 体ごと / 予定の発火 1 回）で 1 つ生まれ、
/// `Arc` で封筒に相乗りして因果の全ターンが同じプールから引く。
/// **転送・波の配送で新しいプールを作ってはならない**（天井が蒸発する —
/// delegation-fanout race。契約 pool の節）。
///
/// # 検査と減算は分離されている（TOCTOU は仕様）
///
/// `try_reserve()`（atomic な残高検査）→ LLM 呼び出し → `debit()`（atomic な
/// 減算）。間に飛行時間が挟まるため一体の atomic にはできず、飛行中
/// 1 呼び出し分のオーバーシュートを**許容して数える** — 残高は 0 に飽和し、
/// 次の `try_reserve()` が確実に止める。
#[derive(Debug)]
pub struct BudgetPool {
    /// 天井（milli）。`spent_effective` の計算にも使う不変値。
    ceiling_milli: u64,
    /// 残額（milli）。0 に飽和し、wrap しない。
    remaining_milli: AtomicU64,
    /// 尽きの初回観測フラグ（`note_exhausted` の CAS 対象）。
    exhausted: AtomicBool,
}

impl BudgetPool {
    /// 天井（実効トークン建て）からプールを作る。
    ///
    /// 0 は契約上ここへ来ない（`Some(0)` は world.json 読み込みで `None` へ
    /// 正規化される）が、来ても安全 — 最初の `try_reserve` が偽を返すだけ。
    pub fn new(ceiling_effective: u64) -> Self {
        let ceiling_milli = ceiling_effective.saturating_mul(1000);
        Self {
            ceiling_milli,
            remaining_milli: AtomicU64::new(ceiling_milli),
            exhausted: AtomicBool::new(false),
        }
    }

    /// 周回境界の検査。残額があれば真（LLM を呼んでよい）。
    pub fn try_reserve(&self) -> bool {
        self.remaining_milli.load(Ordering::Acquire) > 0
    }

    /// usage 実測ぶんを減算する（オーバーシュート込み・0 に飽和）。
    pub fn debit(&self, usage: &Usage) {
        self.debit_milli(effective_milli(usage));
    }

    /// 見積もり値など、milli 建ての任意額を減算する。
    pub fn debit_milli(&self, milli: u64) {
        // fetch_sub は wrap するので使わない。飽和引き算を CAS ループで行う。
        let _ = self
            .remaining_milli
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
                Some(v.saturating_sub(milli))
            });
    }

    /// 尽きの初回観測を主張する。**真を受け取るのは因果全体で 1 呼び出しだけ**。
    ///
    /// 並列ワーカーが同時に尽きを観測しても、因果レベルの記録（System 行など、
    /// P2 が決める）を 1 系統に保つための CAS。各ターン自身の分類
    /// （`budget_exhausted`）はこれとは別に、観測した全ターンが刻んでよい。
    pub fn note_exhausted(&self) -> bool {
        self.exhausted
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// 天井（実効トークン建て）。打ち切りの System 行「実効 N トークン」に使う。
    pub fn ceiling_effective(&self) -> u64 {
        self.ceiling_milli.div_ceil(1000)
    }

    /// 使用済み（実効トークン建て・切り上げ）。飽和により天井を超えない —
    /// オーバーシュート分の超過額は残高に現れない（契約どおり数えは
    /// 減算側で行われ、報告は天井で頭打ち）。
    pub fn spent_effective(&self) -> u64 {
        self.ceiling_milli
            .saturating_sub(self.remaining_milli.load(Ordering::Acquire))
            .div_ceil(1000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn usage(prompt: u64, cache_read: u64, completion: u64) -> Usage {
        Usage {
            prompt,
            completion,
            cache_read,
        }
    }

    #[test]
    fn weights_match_the_frozen_constants() {
        // 未キャッシュ 10 → 10 実効。
        assert_eq!(effective_tokens(&usage(10, 0, 0)), 10);
        // 出力 1 → 4 実効。
        assert_eq!(effective_tokens(&usage(0, 0, 1)), 4);
        // 全量キャッシュ済み 10 → 1000 milli → 1 実効。
        assert_eq!(effective_tokens(&usage(10, 10, 0)), 1);
        // 実測形: prompt 116,765 / cached 83,953 / 出力 7,038。
        // uncached 32,812 + 8,395.3 + 28,152 → 69,359.3 → 切り上げ 69,360。
        assert_eq!(effective_tokens(&usage(116_765, 83_953, 7_038)), 69_360);
    }

    #[test]
    fn rounding_is_upward_so_tiny_usage_still_counts() {
        // キャッシュ済み 1 トークン = 100 milli。切り捨てなら 0 になるが、
        // 保守側の規程は 1 を数える。
        assert_eq!(effective_tokens(&usage(1, 1, 0)), 1);
    }

    #[test]
    fn a_broken_usage_with_cache_exceeding_prompt_does_not_wrap() {
        // cache_read > prompt（壊れた usage）でも引き算が wrap せず、
        // キャッシュ分だけが数えられる。
        assert_eq!(effective_milli(&usage(5, 9, 0)), 900);
    }

    #[test]
    fn missing_usage_is_estimated_from_bytes_but_reported_usage_is_trusted() {
        // 欠落（全欄 0）: 送信 8 バイト → 2、受信 5 バイト → 2。
        let estimated = normalized_usage(&Usage::default(), 8, 5);
        assert_eq!(estimated, usage(2, 0, 2));
        // 報告あり: バイト数は無視してそのまま信じる。
        let reported = usage(100, 40, 7);
        assert_eq!(normalized_usage(&reported, 8, 5), reported);
        // 欠落かつ受信も空: completion は 0 のまま（見積もる材料が無い）。
        assert_eq!(normalized_usage(&Usage::default(), 8, 0).completion, 0);
    }

    #[test]
    fn byte_estimates_round_up_and_use_utf8_length() {
        assert_eq!(estimate_tokens(0), 0);
        assert_eq!(estimate_tokens(1), 1);
        assert_eq!(estimate_tokens(4), 1);
        assert_eq!(estimate_tokens(5), 2);
        // 「あ」= UTF-8 で 3 バイト。文字数 ÷4 なら 0 に落ちるが、バイトなら 1。
        assert_eq!(estimate_tokens("あ".len()), 1);
    }

    #[test]
    fn the_pool_saturates_at_zero_and_refuses_further_reservations() {
        let pool = BudgetPool::new(10);
        assert!(pool.try_reserve());

        // 4 実効ぶん引く → 残 6。
        pool.debit(&usage(4, 0, 0));
        assert!(pool.try_reserve());
        assert_eq!(pool.spent_effective(), 4);

        // オーバーシュート: 残 6 のところへ 40 実効（出力 10）。飽和して 0。
        pool.debit(&usage(0, 0, 10));
        assert!(!pool.try_reserve(), "残 0 で予約は拒否");
        assert_eq!(pool.spent_effective(), 10, "報告は天井で頭打ち");
    }

    #[test]
    fn a_zero_ceiling_pool_is_immediately_exhausted_but_safe() {
        // 契約上 Some(0) は読み込みで None へ正規化されるが、来ても安全。
        let pool = BudgetPool::new(0);
        assert!(!pool.try_reserve());
    }

    #[test]
    fn exhaustion_is_observed_exactly_once_across_threads() {
        let pool = Arc::new(BudgetPool::new(1));
        pool.debit(&usage(1, 0, 0));
        assert!(!pool.try_reserve());

        let mut handles = Vec::new();
        for _ in 0..8 {
            let pool = Arc::clone(&pool);
            handles.push(std::thread::spawn(move || pool.note_exhausted()));
        }
        let firsts: usize = handles
            .into_iter()
            .map(|h| h.join().unwrap() as usize)
            .sum();
        assert_eq!(firsts, 1, "初回観測は因果全体で 1 呼び出しだけ");
    }

    #[test]
    fn parallel_debits_never_wrap_and_sum_correctly() {
        // 8 スレッド × 100 回 × 2 実効 = 1,600 実効。天井 1,000 なので飽和。
        let pool = Arc::new(BudgetPool::new(1_000));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let pool = Arc::clone(&pool);
            handles.push(std::thread::spawn(move || {
                for _ in 0..100 {
                    pool.debit(&usage(2, 0, 0));
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert!(!pool.try_reserve(), "合計が天井を超えるので必ず尽きる");
        assert_eq!(pool.spent_effective(), 1_000, "wrap せず天井で飽和");
    }

    #[test]
    fn pools_are_independent_between_causalities() {
        let a = BudgetPool::new(5);
        let b = BudgetPool::new(5);
        a.debit(&usage(5, 0, 0));
        assert!(!a.try_reserve());
        assert!(b.try_reserve(), "別の因果の予算は減らない");
    }
}
