/**
 * 統計の「全会話」を切る月の期間（Spec 42）。**純関数だけ** — 時計も設定も読まない。
 *
 * # 期間の定義（利用者の言葉を式にしたもの。Spec 42 D1）
 *
 * 締め日 `d` を 1 つ持ち、締め月 `{year, month}` の期間は
 * **「前の締め日の翌日 00:00」から「締め日の翌日 00:00」までの半開区間 `[since, until)`**。
 *
 * ```text
 * 締め日 25 / 締め月 8 月:  7/26 00:00 ≤ ts < 8/26 00:00   → 「8 月分（7/26〜8/25）」
 * 月末    / 締め月 8 月:  8/01 00:00 ≤ ts < 9/01 00:00   → 「8 月分（8/1〜8/31）」
 * ```
 *
 * - **選べる締め日は 1〜28 と「月末」（`"eom"`）だけ。** 29〜31 を選ばせないのは、2 月に
 *   存在しない日を選ばせると「その月はどうなるか」の規則がもう 1 つ要るため（却下した代案は
 *   1〜31 + `min(d, 末日)` の丸め — 穴も重なりも出ないが、2 月にだけ黙って 28 日締めになる）
 * - **締め日 1 は月末と同義ではない**（1 日締めは 7/2〜8/1、月末は 8/1〜8/31）。取り違えは
 *   説明文ではなく構造で防ぐ — 設定ページがこの同じ関数で「今の期間」をライブで出す
 * - **状態は締め月 `{year, month}` で持ち、境界はそこから都度導く。** `[since, until)` から
 *   締め日を逆算しない（`since = 1 日` は月末締めとも 1 日締めとも読めず、逆算は不定）
 *
 * # なぜ日付をここで組み立て、コアへは epoch ms だけ渡すか
 *
 * 締め日は**請求を読む人の属性**で、境界はその人の端末のローカル時刻で決まる。
 * `Date(y, m, d)` はローカル時刻の 0 時を返し、夏時間の 23 / 25 時間の日も正しくまたぐ。
 * コアはタイムゾーンを持たない（`aggregate` は純関数）ので、渡すのは 2 数だけ。
 *
 * # なぜ現在時刻を引数で受け取るか
 *
 * `clock.ts` / `schedule.rs` と同じ規律 — 内部で `new Date()` を読むと、テストが壁時計に
 * 依存して特定の日にだけ落ちるものになる。
 */

/** 締め日。1〜28 の日か、月末。 */
export type ClosingDay = number | "eom";

/** 選べる締め日の最大。29〜31 は選ばせない（上の理由）。 */
export const CLOSING_DAY_MAX = 28;

/** 既定 = 暦の月。 */
export const DEFAULT_CLOSING_DAY: ClosingDay = "eom";

/** 締め月。`month` は 1〜12。 */
export interface ClosingMonth {
  year: number;
  month: number;
}

/** 期間の境界（epoch ms）。`sinceMs` を含み `untilMs` を含まない。 */
export interface PeriodBounds {
  sinceMs: number;
  untilMs: number;
}

/**
 * 保存値の検証。`1..=28` の整数か `"eom"` だけを通す。
 * 手編集や旧版の値（範囲外・小数・文字列）は呼び出し側が既定へ落とす。
 */
export function isClosingDay(value: unknown): value is ClosingDay {
  if (value === "eom") return true;
  return typeof value === "number" && Number.isInteger(value) && value >= 1 && value <= CLOSING_DAY_MAX;
}

/** ローカル時刻の `y-m-d 00:00:00.000`。`m` は 1 始まり。溢れた日は JS が翌月へ繰り上げる。 */
function localMidnight(year: number, month1: number, day: number): Date {
  return new Date(year, month1 - 1, day, 0, 0, 0, 0);
}

/**
 * 今日を含む期間の締め月。
 *
 * 締め日 `d`: 今日が `d` 日以前ならその月、`d` 日より後なら翌月が締め月
 * （8/19 で締め日 25 → 8 月分。8/26 なら 9 月分）。月末: 今月。
 */
export function closingMonthOf(closingDay: ClosingDay, now: Date): ClosingMonth {
  const ym = { year: now.getFullYear(), month: now.getMonth() + 1 };
  if (closingDay === "eom" || now.getDate() <= closingDay) return ym;
  return shift(ym, 1);
}

/** 締め月を `delta` か月ずらす。年をまたぐ（12 月 +1 → 翌年 1 月）。 */
export function shift(ym: ClosingMonth, delta: number): ClosingMonth {
  // 0 始まりに直して足し、floor で年へ繰り上げ・繰り下げる（負の delta でも正しい）。
  const index = ym.year * 12 + (ym.month - 1) + delta;
  return { year: Math.floor(index / 12), month: (((index % 12) + 12) % 12) + 1 };
}

/**
 * 締め月の期間 `[sinceMs, untilMs)`。
 *
 * - 締め日 `d`: `since` = 前月 `d+1` 日 00:00、`until` = 締め月 `d+1` 日 00:00。
 *   `d ≤ 28` なので `d+1 ≤ 29` で、2 月（28 日）の `29` は JS が 3/1 へ繰り上げる =
 *   「2/28 の翌日 00:00」そのもの。丸めの規則を書かずに済んでいるのは、上限を 28 に
 *   したから
 * - 月末: `since` = 締め月 1 日 00:00、`until` = 翌月 1 日 00:00
 *
 * 隣り合う締め月の `until` と `since` は一致する（穴も重なりも無い）。
 */
export function boundsOf(closingDay: ClosingDay, ym: ClosingMonth): PeriodBounds {
  if (closingDay === "eom") {
    return {
      sinceMs: localMidnight(ym.year, ym.month, 1).getTime(),
      untilMs: localMidnight(ym.year, ym.month + 1, 1).getTime(),
    };
  }
  const prev = shift(ym, -1);
  return {
    sinceMs: localMidnight(prev.year, prev.month, closingDay + 1).getTime(),
    untilMs: localMidnight(ym.year, ym.month, closingDay + 1).getTime(),
  };
}

/**
 * 表示用の範囲。`until` は排他なので**右端は `until − 1 ms` を日付にしたもの**
 * （`9/1 00:00` を `8/31` として出す）。`since` と同じローカル時刻で日付化する。
 */
export function rangeOf(closingDay: ClosingDay, ym: ClosingMonth): { first: Date; last: Date } {
  const b = boundsOf(closingDay, ym);
  return { first: new Date(b.sinceMs), last: new Date(b.untilMs - 1) };
}

/** `M/D`。言語で変えない（`clock.ts` と同じ — 範囲は識別子に近く、月日の順が国で変わると読めない）。 */
export function shortDate(d: Date): string {
  return `${d.getMonth() + 1}/${d.getDate()}`;
}

/**
 * `◀` を押せるか = 1 つ前の締め月に、村で最初の turn（`oldestMs`）以後の時刻が含まれるか。
 * `oldestMs` が無い（記録ゼロの村）なら押せない。
 */
export function canGoBack(closingDay: ClosingDay, ym: ClosingMonth, oldestMs: number | null | undefined): boolean {
  if (oldestMs === null || oldestMs === undefined) return false;
  return boundsOf(closingDay, ym).sinceMs > oldestMs;
}

/** `▶` を押せるか = 今日を含む締め月より前を見ているか（未来の期間は開けない）。 */
export function canGoForward(closingDay: ClosingDay, ym: ClosingMonth, now: Date): boolean {
  const current = closingMonthOf(closingDay, now);
  return ym.year * 12 + ym.month < current.year * 12 + current.month;
}

/**
 * ラベルの材料（辞書の `stats.period.label` へ渡す）。文の枠は言語ごとに辞書が持ち、
 * ここは数字と `M/D` だけを返す — 訳した語句を枠へ埋め込むと語順が言語をまたげない
 * （`toolLabel` の「{name}: {tool}」と同じ判断）。
 */
export function labelParamsOf(
  closingDay: ClosingDay,
  ym: ClosingMonth,
): { year: number; month: number; month2: string; from: string; to: string } {
  const r = rangeOf(closingDay, ym);
  return {
    year: ym.year,
    month: ym.month,
    // 2 桁詰め（英語の `2026-08` 用。日本語の「8 月分」は素の数字を使う）。
    month2: ym.month < 10 ? `0${ym.month}` : String(ym.month),
    from: shortDate(r.first),
    to: shortDate(r.last),
  };
}
