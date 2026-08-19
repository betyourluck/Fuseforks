import { describe, expect, it } from "vitest";

import { parsePriceInput, todayIsoDate } from "./priceInput";

/**
 * 初版の setter（文字列前提）。**`type="number"` の v-model は数値を渡す**ので、
 * これは数字で落ちる — 実機で「手入力が保存されない」として出たバグの再現。
 */
function legacySetter(raw: string): number | null | undefined {
  const trimmed = raw.trim();
  const parsed = trimmed === "" ? null : Number(trimmed);
  return parsed === null || (Number.isFinite(parsed) && parsed >= 0) ? parsed : undefined;
}

describe("parsePriceInput", () => {
  it("（再現）初版は v-model が渡す数値で TypeError になり、欄が書かれない", () => {
    // Vue の vModelText は type=number の値を looseToNumber で数値にして setter へ渡す。
    expect(() => legacySetter(3 as unknown as string)).toThrow(TypeError);
    // 空欄だけは文字列のまま届くので通っていた（「取得」は効いて手入力だけ効かない形）。
    expect(legacySetter("")).toBeNull();
  });

  it("数値でも文字列でも同じ結果になる（v-model の自動変換に依存しない）", () => {
    expect(parsePriceInput(3)).toBe(3);
    expect(parsePriceInput("3")).toBe(3);
    expect(parsePriceInput(0.25)).toBe(0.25);
    expect(parsePriceInput(" 0.25 ")).toBe(0.25);
    expect(parsePriceInput(0)).toBe(0);
  });

  it("空欄は未設定（null）へ戻す — 0 にしない（未設定は無料ではない）", () => {
    expect(parsePriceInput("")).toBeNull();
    expect(parsePriceInput("   ")).toBeNull();
  });

  it("負数・NaN・通貨記号は undefined（欄を触らない）", () => {
    expect(parsePriceInput(-1)).toBeUndefined();
    expect(parsePriceInput("-0.5")).toBeUndefined();
    expect(parsePriceInput("$3")).toBeUndefined();
    expect(parsePriceInput("abc")).toBeUndefined();
    expect(parsePriceInput(Number.NaN)).toBeUndefined();
    expect(parsePriceInput(Number.POSITIVE_INFINITY)).toBeUndefined();
  });
});

describe("todayIsoDate", () => {
  it("ローカル日付を YYYY-MM-DD で返す（手入力の pricingAsOf 用）", () => {
    expect(todayIsoDate(new Date(2026, 7, 19, 23, 13, 10))).toBe("2026-08-19");
    expect(todayIsoDate(new Date(2026, 0, 5))).toBe("2026-01-05");
  });
});
