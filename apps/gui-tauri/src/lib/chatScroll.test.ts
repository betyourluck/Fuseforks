import { describe, expect, it } from "vitest";
import { NEAR_BOTTOM_PX, isNearBottom } from "./chatScroll";

describe("isNearBottom", () => {
  it("末尾にいれば真", () => {
    expect(isNearBottom({ scrollHeight: 1000, scrollTop: 600, clientHeight: 400 })).toBe(true);
  });

  it("末尾から閾値の手前までは真、閾値ちょうどからは偽", () => {
    const at = (distance: number) =>
      isNearBottom({ scrollHeight: 1000, scrollTop: 600 - distance, clientHeight: 400 });
    expect(at(NEAR_BOTTOM_PX - 1)).toBe(true);
    expect(at(NEAR_BOTTOM_PX)).toBe(false);
    expect(at(300)).toBe(false);
  });

  it("中身が窓より短ければ（スクロールできなければ）真", () => {
    expect(isNearBottom({ scrollHeight: 200, scrollTop: 0, clientHeight: 400 })).toBe(true);
  });
});
