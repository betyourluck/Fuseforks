import { describe, expect, it } from "vitest";
import { contentExtent, fitMargin } from "./kizunaFit";

const R = 26;
const MAX = 2;

describe("fitMargin", () => {
  it("内容が大きければ基準の余白（8%）のまま — 従来の Fit", () => {
    // 10 体が 900×700 に散っている村。上限 zoom でも収まらないので余白は増やさない。
    const points = [
      { x: 0, y: 0 },
      { x: 900, y: 700 },
    ];
    const m = fitMargin(points, R, { width: 1200, height: 400 }, MAX);
    expect(m).toEqual({ top: 32, left: 96, right: 96, bottom: 32 });
  });

  it("2 体だけなら余るぶんを余白へ回し、要求 zoom が上限ちょうどになる", () => {
    // 実機の形: 近い 2 体を幅 1200 のペインで Fit すると要求 zoom は 4.88 → 2 へ clamp され
    // pan だけが 4.88 用のまま残って左上へ寄った。
    const points = [
      { x: 0, y: 0 },
      { x: 120, y: 30 },
    ];
    const pane = { width: 1200, height: 400 };
    const m = fitMargin(points, R, pane, MAX);
    const extent = contentExtent(points, R)!;
    // ライブラリの zoom = (ペイン − 余白) / 内容 が、両軸とも上限以下で、狭いほうが上限ちょうど。
    const zx = (pane.width - m.left - m.right) / extent.width;
    const zy = (pane.height - m.top - m.bottom) / extent.height;
    expect(Math.min(zx, zy)).toBeCloseTo(MAX, 6);
    expect(Math.max(zx, zy)).toBeLessThanOrEqual(MAX + 1e-6);
    expect(m.left).toBeCloseTo((1200 - 172 * MAX) / 2, 6);
    expect(m.top).toBeCloseTo((400 - 82 * MAX) / 2, 6);
  });

  it("余白は基準を下回らない（片軸だけ余るとき）", () => {
    // 横に長い並び: 横は収まりきるが縦は余る。縦だけ余白が増え、横は 8% のまま。
    const points = [
      { x: 0, y: 0 },
      { x: 500, y: 0 },
    ];
    const m = fitMargin(points, R, { width: 1200, height: 400 }, MAX);
    expect(m.left).toBe(96);
    expect(m.top).toBeGreaterThan(32);
  });

  it("1 点以下と上限 0 は基準の余白", () => {
    expect(fitMargin([{ x: 5, y: 5 }], R, { width: 600, height: 400 }, MAX)).toEqual({
      top: 32,
      left: 48,
      right: 48,
      bottom: 32,
    });
    expect(fitMargin([], R, { width: 600, height: 400 }, MAX).left).toBe(48);
    expect(fitMargin([{ x: 0, y: 0 }, { x: 1, y: 1 }], R, { width: 600, height: 400 }, 0).left).toBe(48);
  });

  it("広がりは円の半径だけで見積もる（ラベルを含めない = 控えめ）", () => {
    // 大きく見積もると zoom が上限を超えて元の症状に戻るので、この向きを固定する。
    expect(contentExtent([{ x: 0, y: 0 }, { x: 100, y: 0 }], R)).toEqual({ width: 152, height: 52 });
    expect(contentExtent([{ x: 0, y: 0 }], R)).toBeNull();
  });
});
