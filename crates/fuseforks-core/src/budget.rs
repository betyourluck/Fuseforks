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

/// 予約の見積もりの床（milli = 1,000 実効トークン。Spec 38 D1(b)）。
///
/// 直前の呼び出しの実測を見積もりに使うが、**初回は実測が無い**ので
/// ここから始める。床を上げると単発の小さい依頼まで早期に止まる側へ倒れる。
pub const RESERVE_FLOOR_MILLI: u64 = 1_000_000;

/// 直前の実測から次の予約額を決める（Spec 38 D1(b)）。
///
/// `max(直前の実測, 床)` を**天井で頭打ち**にする。頭打ちが要るのは、
/// 直前の 1 呼び出しが天井を超えていた場合に `try_reserve` が
/// 「`estimate > ceiling` は即 `None`」で**永久に通らなくなる**ため —
/// 天井 1,000 実効の村で 1 回でも 1,000 超の呼び出しをした個体が、
/// 以後どの因果でも 1 度も走れなくなる形を塞ぐ。
pub fn reserve_estimate_milli(last_call_milli: u64, ceiling_milli: u64) -> u64 {
    last_call_milli.max(RESERVE_FLOOR_MILLI).min(ceiling_milli)
}

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
        // 思考ぶんは**見積もらない**。バイト数から出せるのは受け取った本文の量で、
        // 思考は本文に現れないので、推定する材料がそもそも無い。ここで
        // 保守側へ倒す（大きめに入れる）と、天井には効かないのに計器だけが
        // 嘘の桁を運ぶ（Spec 31 D8 で tick を換算しなかったのと同じ規律）。
        reasoning: 0,
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
/// 検査 → LLM 呼び出し → 清算の間に飛行時間が挟まるため一体の atomic には
/// できず、オーバーシュートを**許容して数える** — 残高は 0 に飽和し、次の
/// 検査が確実に止める。検査は 2 形ある（Spec 38 P1）:
/// `has_remaining`（load 観測のみ。配送前検査と、P2 までの周回境界）/
/// `try_reserve(estimate)`（CAS 予約。P2 で周回境界がこちらへ移る）。
///
/// 超過の上限は **1 呼び出し分ではない**（2026-08-14 に TLC で反証。
/// `specs/tla/BudgetOvershootBound.tla`）。load 観測だけで通すと、
/// 波が N 体へ撒くと残額 1 でも N 体が同時に通る —
/// 上限は「同時に飛ぶ本数 × その時いちばん大きい 1 呼び出しの実費」。
/// 因果が 1 本のときだけ「1 呼び出し分」に縮む。
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
    /// 正規化される）が、来ても安全 — 最初の `has_remaining` が偽を返すだけ。
    pub fn new(ceiling_effective: u64) -> Self {
        let ceiling_milli = ceiling_effective.saturating_mul(1000);
        Self {
            ceiling_milli,
            remaining_milli: AtomicU64::new(ceiling_milli),
            exhausted: AtomicBool::new(false),
        }
    }

    /// 残額の観測（load のみ・予約しない）。
    ///
    /// 旧名 `try_reserve` — **予約しないのに予約を名乗っていたのが #105 の
    /// 嘘の起点**なので、Spec 38 P1 で実態の名前へ改めた。使い所は 2 つ:
    /// `deliver_and_wait` 冒頭の配送前検査（LLM を呼ばない門なので観測で
    /// 足りる — ここを予約に変えないことは契約とテストで凍結）と、
    /// P2 で予約へ置き換わるまでの `run_turn` の周回境界。
    pub fn has_remaining(&self) -> bool {
        self.remaining_milli.load(Ordering::Acquire) > 0
    }

    /// 見積もりぶんを CAS で先に引いて予約する（Spec 38 P1）。
    ///
    /// - 残額が `estimate_milli` に満たなければ `None`（引かない）。
    /// - **`estimate_milli > ceiling` は残額に関わらず `None`** — 天井より
    ///   大きい予約の部分適用は残高を負方向へ持っていけるので書かない。
    /// - 残額 0 のときは `estimate_milli == 0` でも `None`（尽きた因果で
    ///   新しい呼び出しを始めない、という門の意味を見積もり 0 が迂回しない）。
    ///
    /// 返る [`ReservationGuard`] が予約の所有者。`commit(actual)` で実測と
    /// 清算し、commit されなかったパスでは Drop が全額を返す。
    pub fn try_reserve(&self, estimate_milli: u64) -> Option<ReservationGuard<'_>> {
        if estimate_milli > self.ceiling_milli {
            return None;
        }
        self.remaining_milli
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
                if v == 0 || v < estimate_milli {
                    None
                } else {
                    Some(v - estimate_milli)
                }
            })
            .ok()
            .map(|_| ReservationGuard {
                pool: self,
                reserved_milli: estimate_milli,
            })
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

    /// 天井（milli 建て）。予約額の頭打ちに使う（[`reserve_estimate_milli`]）。
    pub fn ceiling_milli(&self) -> u64 {
        self.ceiling_milli
    }

    /// 使用済み（実効トークン建て・切り上げ）。飽和により天井を超えない —
    /// オーバーシュート分の超過額は残高に現れない（契約どおり数えは
    /// 減算側で行われ、報告は天井で頭打ち）。
    ///
    /// **飛行中の予約も「使用済み」に見える**（予約は残額から引かれている）。
    /// commit の清算で実測へ収束するので、統計の読みでは一時的な揺れとして
    /// 扱う（`reserved=` を計器へ出すかは Spec 38 P4 の判断）。
    pub fn spent_effective(&self) -> u64 {
        self.ceiling_milli
            .saturating_sub(self.remaining_milli.load(Ordering::Acquire))
            .div_ceil(1000)
    }

    /// 返金（milli 建て・**天井で飽和**）。
    ///
    /// 会計が正しければ数学的には天井を超えないが、並列返金の一時的な
    /// 天井超えが後続 debit の超過を隠す形を防御的に塞ぐ（Spec 38 P0）。
    fn credit_milli(&self, milli: u64) {
        let ceiling = self.ceiling_milli;
        let _ = self
            .remaining_milli
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
                Some(v.saturating_add(milli).min(ceiling))
            });
    }
}

/// 予約の所有者（Spec 38 P0 で API を凍結）。
///
/// [`BudgetPool::try_reserve`] が残額から引いた見積もりぶんを 1 つだけ持ち、
/// 清算の経路を型で 2 つに限る:
///
/// - [`commit`](Self::commit) — 実測との差額を清算する。**self を move で
///   消費する**ので、commit 後に Drop は呼ばれない。
/// - Drop — commit されなかったパス（`?` / return / future の drop）で
///   予約の**全額**を返す。
///
/// `committed` フラグは持たない — フラグを足すと「commit したのに Drop も
/// 返す」二重返金が再び書ける形に戻る（Spec 38 承認査読 a）。
///
/// **`JoinSet::abort` で Drop が走るかは tokio 文書に明言が無い** — このモジュールの
/// `join_set_abort_runs_drop_and_refunds` が実測で留める（Spec 38 P1）。
/// プロセス死（exit / SIGKILL）で Drop が走らないのは実害の経路が無い —
/// プールはプロセス内メモリで再起動で復元しない（Spec 12 の凍結の帰結）。
#[derive(Debug)]
pub struct ReservationGuard<'a> {
    pool: &'a BudgetPool,
    reserved_milli: u64,
}

impl ReservationGuard<'_> {
    /// 実測で清算して予約を閉じる。
    ///
    /// `actual > estimate` なら差額を debit、`actual < estimate` なら差額を
    /// 返金（天井で飽和）。self は move で消費され、以後 Drop は走らない。
    pub fn commit(self, actual_milli: u64) {
        let pool = self.pool;
        let reserved = self.reserved_milli;
        // Drop（全額返金）を無効化する。フィールドは Copy なので取り出し済み。
        std::mem::forget(self);
        if actual_milli >= reserved {
            pool.debit_milli(actual_milli - reserved);
        } else {
            pool.credit_milli(reserved - actual_milli);
        }
    }

    /// usage 実測で清算する（`commit` の実効 milli 換算版）。
    pub fn commit_usage(self, usage: &Usage) {
        self.commit(effective_milli(usage));
    }
}

impl Drop for ReservationGuard<'_> {
    /// commit されなかったパスでのみ走る。予約の全額を返す。
    fn drop(&mut self) {
        self.pool.credit_milli(self.reserved_milli);
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
            // 予算の計算は思考ぶんを見ない（内数なので completion に入っている）。
            // ここを 0 以外にしても実効トークンが動かないことは
            // `reasoning_does_not_change_the_effective_cost` が留める。
            reasoning: 0,
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

    /// 思考ぶんは `completion` の内数なので、**実効トークンを 1 ミリも動かさない**
    /// （Spec 32 D2 / `data_contract` の `llm_wire.usage`）。
    ///
    /// これが破れると、思考するモデルの村だけ天井が早く尽きる。
    /// `effective_milli` が `reasoning` を読む実装に変えると、この 1 本が落ちる。
    #[test]
    fn reasoning_does_not_change_the_effective_cost() {
        let mut with_thinking = usage(1_000, 0, 500);
        with_thinking.reasoning = 499; // 実測の比率（出力のほぼ全部が思考）

        assert_eq!(
            effective_milli(&with_thinking),
            effective_milli(&usage(1_000, 0, 500)),
            "内数なので、思考ぶんを入れても実効トークンは変わらない"
        );
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
        assert!(pool.has_remaining());

        // 4 実効ぶん引く → 残 6。
        pool.debit(&usage(4, 0, 0));
        assert!(pool.has_remaining());
        assert_eq!(pool.spent_effective(), 4);

        // オーバーシュート: 残 6 のところへ 40 実効（出力 10）。飽和して 0。
        pool.debit(&usage(0, 0, 10));
        assert!(!pool.has_remaining(), "残 0 で検査は拒否");
        assert_eq!(pool.spent_effective(), 10, "報告は天井で頭打ち");
    }

    #[test]
    fn a_zero_ceiling_pool_is_immediately_exhausted_but_safe() {
        // 契約上 Some(0) は読み込みで None へ正規化されるが、来ても安全。
        let pool = BudgetPool::new(0);
        assert!(!pool.has_remaining());
    }

    #[test]
    fn exhaustion_is_observed_exactly_once_across_threads() {
        let pool = Arc::new(BudgetPool::new(1));
        pool.debit(&usage(1, 0, 0));
        assert!(!pool.has_remaining());

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
        assert!(!pool.has_remaining(), "合計が天井を超えるので必ず尽きる");
        assert_eq!(pool.spent_effective(), 1_000, "wrap せず天井で飽和");
    }

    #[test]
    fn pools_are_independent_between_causalities() {
        let a = BudgetPool::new(5);
        let b = BudgetPool::new(5);
        a.debit(&usage(5, 0, 0));
        assert!(!a.has_remaining());
        assert!(b.has_remaining(), "別の因果の予算は減らない");
    }

    // ── Spec 38 P1: 予約（ReservationGuard）─────────────────────────

    #[test]
    fn reserve_commit_settles_to_the_actual_cost() {
        // 実測 > 見積もり: 差額が追加で引かれ、最終残高は「天井 − 実測」。
        let pool = BudgetPool::new(1_000);
        let g = pool.try_reserve(300_000).expect("残額十分");
        assert_eq!(pool.spent_effective(), 300, "予約が残額から引かれている");
        g.commit(500_000);
        assert_eq!(pool.spent_effective(), 500, "実測 500 へ清算（過小予約の差額を debit）");

        // 実測 < 見積もり: 差額が返金され、最終残高は同じく「天井 − 実測」。
        let pool = BudgetPool::new(1_000);
        let g = pool.try_reserve(300_000).expect("残額十分");
        g.commit(100_000);
        assert_eq!(pool.spent_effective(), 100, "実測 100 へ清算（過大予約の差額を返金）");
    }

    #[test]
    fn reserve_rejects_when_estimate_exceeds_ceiling() {
        // 満額の残高があっても、天井より大きい予約は部分適用せず即 None（P0 凍結）。
        let pool = BudgetPool::new(1_000);
        assert!(pool.try_reserve(1_000_001).is_none());
        assert_eq!(pool.spent_effective(), 0, "拒否は残高に触らない");
    }

    #[test]
    fn reserve_rejects_when_remaining_is_short_and_zero_estimate_does_not_bypass() {
        let pool = BudgetPool::new(1_000);
        let _g = pool.try_reserve(900_000).expect("1 本目は通る");
        assert!(pool.try_reserve(200_000).is_none(), "残 100 に 200 の予約は通らない");
        pool.debit_milli(u64::MAX); // 残 0 へ
        assert!(pool.try_reserve(0).is_none(), "残 0 では見積もり 0 でも門を迂回できない");
    }

    #[test]
    fn drop_without_commit_refunds_in_full() {
        // `?` / return / future の drop で guard が落ちるパスの等価物。
        let pool = BudgetPool::new(1_000);
        {
            let _g = pool.try_reserve(400_000).expect("残額十分");
            assert_eq!(pool.spent_effective(), 400);
        }
        assert_eq!(pool.spent_effective(), 0, "commit されなかった予約は全額戻る");
    }

    #[test]
    fn refund_saturates_at_ceiling() {
        // 会計が正しければ超えないが、防御的飽和が効いていることを白箱で留める
        // （credit_milli は private なので同モジュールのテストだけが踏める）。
        let pool = BudgetPool::new(1_000);
        pool.credit_milli(500_000);
        assert_eq!(pool.spent_effective(), 0, "満額への返金は天井で頭打ち");
    }

    #[test]
    fn estimate_starts_at_the_floor_and_then_follows_the_last_call() {
        let ceiling = 1_000_000_000; // 実効 1,000,000（既定の天井）
        // 初回は実測が無い → 床。
        assert_eq!(reserve_estimate_milli(0, ceiling), RESERVE_FLOOR_MILLI);
        // 床より小さい実測は床のまま（小さすぎる予約は上限を縮めない）。
        assert_eq!(reserve_estimate_milli(1_000, ceiling), RESERVE_FLOOR_MILLI);
        // 床より大きい実測に追従する。
        assert_eq!(reserve_estimate_milli(50_000_000, ceiling), 50_000_000);
    }

    #[test]
    fn estimate_is_capped_at_the_ceiling_so_a_huge_last_call_cannot_wedge_the_agent() {
        // 直前の 1 呼び出しが天井を超えていた個体。頭打ちが無いと
        // `try_reserve` の「estimate > ceiling は即 None」に当たり続け、
        // **以後どの因果でも 1 度も走れなくなる**。
        let ceiling = 5_000;
        let estimate = reserve_estimate_milli(9_999_999, ceiling);
        assert_eq!(estimate, ceiling, "天井で頭打ち");
        let pool = BudgetPool::new(5);
        assert!(
            pool.try_reserve(estimate).is_some(),
            "頭打ちした見積もりは満額の残高で必ず通る（詰みを作らない）"
        );
    }

    /// TLC の `SpecReserving`（緑）の実装版 + ミューテーション (i) の的。
    ///
    /// 天井 = 1 呼び出しぶん。3 スレッドが同時に予約を試みると、CAS では
    /// **ちょうど 1 本**しか通らない。`try_reserve` を load 観測に戻す
    /// ミューテーションでは 3 本とも通り、spent が「天井 + 1 呼び出し」を
    /// 超えて赤くなる（TLC の反証経路 ReserveByLoad ×3 → DebitByLoad ×3 の再現）。
    #[test]
    fn parallel_reserve_admits_exactly_one_call_at_the_boundary() {
        let call_milli: u64 = 1_000_000; // 1 呼び出し = 実効 1,000
        let pool = Arc::new(BudgetPool::new(1_000)); // 天井 = ちょうど 1 呼び出し
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let mut handles = Vec::new();
        for _ in 0..3 {
            let pool = Arc::clone(&pool);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                match pool.try_reserve(call_milli) {
                    Some(g) => {
                        g.commit(call_milli);
                        call_milli
                    }
                    None => 0,
                }
            }));
        }
        let spent_milli: u64 = handles.into_iter().map(|h| h.join().unwrap()).sum();
        assert_eq!(spent_milli, call_milli, "境界で通るのはちょうど 1 本");
        assert!(
            spent_milli <= 1_000_000 + call_milli,
            "OvershootAtMostOneCall（TLC と同じ不変条件）"
        );
    }

    /// **Spec 38 P0 が「仮説」として P1 へ送った 1 点を実測で決める** —
    /// `JoinSet::abort` で guard の Drop が走るか（tokio 文書に明言が無く、
    /// 査読 2 系統でも結論が割れた）。判定は残高の復元で読む: Drop が
    /// 走らなければ予約 400 が引かれたまま残り、このテストが赤くなる。
    #[tokio::test(flavor = "multi_thread")]
    async fn join_set_abort_runs_drop_and_refunds() {
        let pool = Arc::new(BudgetPool::new(1_000));
        let mut set = tokio::task::JoinSet::new();
        let inner = Arc::clone(&pool);
        set.spawn(async move {
            let _g = inner.try_reserve(400_000).expect("残額十分");
            std::future::pending::<()>().await; // commit へ到達しない飛行中
        });
        // spawn 直後はまだ予約前かもしれない — 予約の観測を先に取る。
        // **有界ループ**にする: 無限 while だと、予約が起きない実装
        // （ミューテーション (i) = load 観測化）でハングになり、赤と
        // 区別が付かない（#86 — 失敗は「永久に返らない」形にしない）。
        let mut reserved_seen = false;
        for _ in 0..10_000 {
            if pool.spent_effective() == 400 {
                reserved_seen = true;
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(reserved_seen, "予約が残額から引かれる実装であること");
        set.abort_all();
        while set.join_next().await.is_some() {}
        assert_eq!(
            pool.spent_effective(),
            0,
            "abort で Drop が走り、予約が全額返金される（仮説の実測決着）"
        );
    }
}
