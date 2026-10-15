import { describe, expect, it } from "vitest";

import { NEAR_LIMIT_RATIO, nearLimitFrom, roundBadge } from "./typingRound";

describe("周回数バッジ", () => {
  it("普段は数だけ（上限が分かっていても出さない）", () => {
    expect(roundBadge(2, 64)).toEqual({ label: "2", nearLimit: false });
    expect(roundBadge(2, null)).toEqual({ label: "2", nearLimit: false });
  });

  it("上限の 8 割から「N / 上限」と警告", () => {
    expect(NEAR_LIMIT_RATIO).toBe(0.8);
    expect(nearLimitFrom(64)).toBe(52);
    expect(nearLimitFrom(36)).toBe(29);
    expect(roundBadge(51, 64)).toEqual({ label: "51", nearLimit: false });
    expect(roundBadge(52, 64)).toEqual({ label: "52 / 64", nearLimit: true });
    expect(roundBadge(64, 64)).toEqual({ label: "64 / 64", nearLimit: true });
  });

  it("上限が分からなければ、どれだけ回っても警告しない（分母を捏造しない）", () => {
    expect(roundBadge(200, null)).toEqual({ label: "200", nearLimit: false });
    expect(roundBadge(200, 0)).toEqual({ label: "200", nearLimit: false });
  });

  it("小さな上限でも閾値は 1 以上", () => {
    expect(nearLimitFrom(1)).toBe(1);
    expect(roundBadge(1, 1)).toEqual({ label: "1 / 1", nearLimit: true });
  });
});
