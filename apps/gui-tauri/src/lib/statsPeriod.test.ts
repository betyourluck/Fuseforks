import { describe, expect, it } from "vitest";

import {
  boundsOf,
  canGoBack,
  canGoForward,
  closingMonthOf,
  isClosingDay,
  labelParamsOf,
  rangeOf,
  shift,
  shortDate,
} from "./statsPeriod";

/** ローカル時刻の 0 時（月は 1 始まりで書く — テストを読む側の目のため）。 */
const at = (y: number, m: number, d: number, h = 0, min = 0, s = 0, ms = 0) =>
  new Date(y, m - 1, d, h, min, s, ms).getTime();

describe("closingMonthOf", () => {
  it("締め日 25 で 8/19 → 8 月分（今日が締め日以前ならその月）", () => {
    expect(closingMonthOf(25, new Date(2026, 7, 19))).toEqual({ year: 2026, month: 8 });
  });

  it("締め日 25 で 8/25 → 8 月分、8/26 → 9 月分（締め日は含む・翌日から次期）", () => {
    expect(closingMonthOf(25, new Date(2026, 7, 25, 23, 59))).toEqual({ year: 2026, month: 8 });
    expect(closingMonthOf(25, new Date(2026, 7, 26, 0, 0))).toEqual({ year: 2026, month: 9 });
  });

  it("月末は今月", () => {
    expect(closingMonthOf("eom", new Date(2026, 7, 31))).toEqual({ year: 2026, month: 8 });
    expect(closingMonthOf("eom", new Date(2026, 7, 1))).toEqual({ year: 2026, month: 8 });
  });

  it("締め日 25 で 12/26 → 翌年 1 月分（年をまたぐ）", () => {
    expect(closingMonthOf(25, new Date(2026, 11, 26))).toEqual({ year: 2027, month: 1 });
  });
});

describe("boundsOf", () => {
  it("締め日 25 / 8 月分 = [7/26 00:00, 8/26 00:00)", () => {
    expect(boundsOf(25, { year: 2026, month: 8 })).toEqual({
      sinceMs: at(2026, 7, 26),
      untilMs: at(2026, 8, 26),
    });
  });

  it("月末 / 8 月分 = [8/1 00:00, 9/1 00:00)", () => {
    expect(boundsOf("eom", { year: 2026, month: 8 })).toEqual({
      sinceMs: at(2026, 8, 1),
      untilMs: at(2026, 9, 1),
    });
  });

  it("締め日 1 は月末と別の期間 — 8 月分 = [7/2, 8/2)", () => {
    expect(boundsOf(1, { year: 2026, month: 8 })).toEqual({
      sinceMs: at(2026, 7, 2),
      untilMs: at(2026, 8, 2),
    });
  });

  it("月末 / 2 月は 28 日（2026）と 29 日（2028 うるう年）で終わる", () => {
    expect(boundsOf("eom", { year: 2026, month: 2 }).untilMs).toBe(at(2026, 3, 1));
    expect(rangeOf("eom", { year: 2026, month: 2 }).last.getDate()).toBe(28);
    expect(rangeOf("eom", { year: 2028, month: 2 }).last.getDate()).toBe(29);
  });

  it("締め日 28 / 2 月分の until は 3/1 00:00（2/28 の翌日。丸めの規則を書かずに済む理由）", () => {
    expect(boundsOf(28, { year: 2026, month: 2 })).toEqual({
      sinceMs: at(2026, 1, 29),
      untilMs: at(2026, 3, 1),
    });
  });

  it("年をまたぐ: 締め日 25 / 1 月分 = [12/26 前年, 1/26)", () => {
    expect(boundsOf(25, { year: 2027, month: 1 })).toEqual({
      sinceMs: at(2026, 12, 26),
      untilMs: at(2027, 1, 26),
    });
  });

  it("隣り合う締め月は until == 次の since（穴も重なりも無い）— 12 か月ぶん", () => {
    for (const closing of [1, 15, 25, 28, "eom"] as const) {
      let ym = { year: 2026, month: 1 };
      for (let i = 0; i < 12; i += 1) {
        const next = shift(ym, 1);
        expect(boundsOf(closing, ym).untilMs).toBe(boundsOf(closing, next).sinceMs);
        ym = next;
      }
    }
  });
});

describe("shift", () => {
  it("+1 と −1 で元に戻り、年をまたぐ", () => {
    const ym = { year: 2026, month: 12 };
    expect(shift(ym, 1)).toEqual({ year: 2027, month: 1 });
    expect(shift(shift(ym, 1), -1)).toEqual(ym);
    expect(shift({ year: 2026, month: 1 }, -1)).toEqual({ year: 2025, month: 12 });
    expect(shift({ year: 2026, month: 3 }, -15)).toEqual({ year: 2024, month: 12 });
  });
});

describe("rangeOf / shortDate", () => {
  it("右端は until − 1 ms を日付化する（9/1 を 8/31 と出す）", () => {
    const r = rangeOf("eom", { year: 2026, month: 8 });
    expect(shortDate(r.first)).toBe("8/1");
    expect(shortDate(r.last)).toBe("8/31");
    const s = rangeOf(25, { year: 2026, month: 8 });
    expect(shortDate(s.first)).toBe("7/26");
    expect(shortDate(s.last)).toBe("8/25");
  });
});

describe("labelParamsOf", () => {
  it("年・月・2 桁月・M/D の範囲を返す（文の枠は辞書が持つ）", () => {
    expect(labelParamsOf(25, { year: 2026, month: 8 })).toEqual({
      year: 2026,
      month: 8,
      month2: "08",
      from: "7/26",
      to: "8/25",
    });
  });
});

describe("canGoBack / canGoForward", () => {
  const ym = { year: 2026, month: 8 };

  it("◀ は最初の記録を含む締め月で止まる。記録が無ければ押せない", () => {
    // 締め日 25 / 8 月分の since は 7/26。最古が 7/26 より前なら遡れる。
    expect(canGoBack(25, ym, at(2026, 7, 25))).toBe(true);
    expect(canGoBack(25, ym, at(2026, 7, 26))).toBe(false);
    expect(canGoBack(25, ym, at(2026, 8, 10))).toBe(false);
    expect(canGoBack(25, ym, null)).toBe(false);
    expect(canGoBack(25, ym, undefined)).toBe(false);
  });

  it("▶ は今日を含む締め月で止まる（未来は開けない）", () => {
    const now = new Date(2026, 7, 19); // 締め日 25 → 8 月分が今
    expect(canGoForward(25, { year: 2026, month: 7 }, now)).toBe(true);
    expect(canGoForward(25, ym, now)).toBe(false);
    // 8/26 なら 9 月分が今なので 8 月分からは進める
    expect(canGoForward(25, ym, new Date(2026, 7, 26))).toBe(true);
  });
});

describe("isClosingDay", () => {
  it("1..=28 の整数と eom だけを通す", () => {
    expect(isClosingDay(1)).toBe(true);
    expect(isClosingDay(28)).toBe(true);
    expect(isClosingDay("eom")).toBe(true);
    for (const bad of [0, 29, 31, 2.5, -1, "25", "EOM", null, undefined, true]) {
      expect(isClosingDay(bad)).toBe(false);
    }
  });
});
