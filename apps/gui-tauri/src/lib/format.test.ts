import { describe, expect, it } from "vitest";

import { compactNumber, exactNumber } from "./format";

describe("compactNumber", () => {
  it("1000 未満はそのまま整数で返す", () => {
    expect(compactNumber(0)).toBe("0");
    expect(compactNumber(7)).toBe("7");
    expect(compactNumber(999)).toBe("999");
  });

  it("1000 以上は小数第 1 位までの短縮表記にする", () => {
    expect(compactNumber(1_000)).toBe("1.0K");
    expect(compactNumber(1_234)).toBe("1.2K");
    expect(compactNumber(1_234_567)).toBe("1.2M");
    expect(compactNumber(5_000_000)).toBe("5.0M");
    expect(compactNumber(1_234_567_890)).toBe("1.2G");
  });

  it("末尾が 0 でも桁を落とさない（並べたときに幅が揃う）", () => {
    expect(compactNumber(2_000_000)).toBe("2.0M");
    expect(compactNumber(2_040_000)).toBe("2.0M");
    expect(compactNumber(2_060_000)).toBe("2.1M");
  });

  it("単位を使い切ったら T のまま桁を伸ばす", () => {
    expect(compactNumber(1_000_000_000_000)).toBe("1.0T");
    // 1000T を超えても P へは進まない（桁がそのまま伸びる）。
    expect(compactNumber(9_999_000_000_000_000)).toBe("9999.0T");
  });

  it("負数は符号を保つ", () => {
    expect(compactNumber(-1_500)).toBe("-1.5K");
    expect(compactNumber(-12)).toBe("-12");
  });

  it("有限でない値は NaN を画面に出さない", () => {
    expect(compactNumber(Number.NaN)).toBe("—");
    expect(compactNumber(Number.POSITIVE_INFINITY)).toBe("—");
  });
});

describe("exactNumber", () => {
  it("桁区切りで正確な値を返す", () => {
    expect(exactNumber(1_234_567)).toBe("1,234,567");
  });

  it("有限でない値は短縮表記と同じ見た目にする", () => {
    expect(exactNumber(Number.NaN)).toBe("—");
  });
});
