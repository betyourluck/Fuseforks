import { describe, expect, it } from "vitest";
import type { Point } from "./kizunaSeed";
import { hasOverlap, untangle } from "./kizunaUntangle";

const GAP = 96;

/** 全部の組が `gap` 以上（押した直後の誤差 0.5 を許す）離れているか。 */
function allApart(points: Record<string, Point>, gap: number): boolean {
  return !hasOverlap(points, gap);
}

function centroid(points: Record<string, Point>): Point {
  const list = Object.values(points);
  return {
    x: list.reduce((s, p) => s + p.x, 0) / list.length,
    y: list.reduce((s, p) => s + p.y, 0) / list.length,
  };
}

describe("hasOverlap", () => {
  it("近すぎる組が 1 つあれば真、全部離れていれば偽", () => {
    expect(hasOverlap({ a: { x: 0, y: 0 }, b: { x: 200, y: 0 } }, GAP)).toBe(false);
    expect(hasOverlap({ a: { x: 0, y: 0 }, b: { x: 50, y: 0 } }, GAP)).toBe(true);
  });

  it("ちょうど gap の距離は重なりではない（押した直後に再発火させない）", () => {
    expect(hasOverlap({ a: { x: 0, y: 0 }, b: { x: GAP, y: 0 } }, GAP)).toBe(false);
  });
});

describe("untangle", () => {
  it("重なりが無ければ何も返さない", () => {
    expect(untangle({ a: { x: 0, y: 0 }, b: { x: 300, y: 0 } }, GAP)).toEqual({});
  });

  it("離れている個体には触れない（動かした個体だけを返す）", () => {
    // **全体を並べ直さない**が裁定の核。遠くの個体は結果に入らない。
    const points = {
      a: { x: 0, y: 0 },
      b: { x: 30, y: 0 },
      far: { x: 1000, y: 1000 },
    };
    const out = untangle(points, GAP);
    expect(Object.keys(out).sort()).toEqual(["a", "b"]);
    expect(allApart({ ...points, ...out }, GAP)).toBe(true);
  });

  it("組を等しく反対向きに押すので、重心が動かない", () => {
    const points = {
      a: { x: 10, y: 10 },
      b: { x: 40, y: 20 },
      c: { x: 25, y: 60 },
    };
    const before = centroid(points);
    const after = centroid({ ...points, ...untangle(points, GAP) });
    expect(after.x).toBeCloseTo(before.x, 6);
    expect(after.y).toBeCloseTo(before.y, 6);
  });

  it("完全に重なった組も離せる（向きは id のハッシュで決まり、毎回同じ）", () => {
    const points = { a: { x: 0, y: 0 }, b: { x: 0, y: 0 } };
    const first = untangle(points, GAP);
    expect(allApart({ ...points, ...first }, GAP)).toBe(true);
    expect(untangle(points, GAP)).toEqual(first);
  });

  it("入力の鍵の順に依らない", () => {
    const one = untangle({ a: { x: 0, y: 0 }, b: { x: 20, y: 5 }, c: { x: 5, y: 30 } }, GAP);
    const other = untangle({ c: { x: 5, y: 30 }, b: { x: 20, y: 5 }, a: { x: 0, y: 0 } }, GAP);
    expect(other).toEqual(one);
  });

  it("押した先で別の個体に近づいたら、その組も押す（連なりごとほどく）", () => {
    // b を a から離すと c に寄る並び。c も結果に入り、全部の組が離れる。
    const points = {
      a: { x: 0, y: 0 },
      b: { x: 10, y: 0 },
      c: { x: 110, y: 0 },
    };
    const out = untangle(points, GAP);
    expect(Object.keys(out).sort()).toEqual(["a", "b", "c"]);
    expect(allApart({ ...points, ...out }, GAP)).toBe(true);
  });

  it("10 体が同じ点に積まれていても、全部の組を離せる", () => {
    const points: Record<string, Point> = {};
    for (let i = 0; i < 10; i += 1) points[`agent_${i}`] = { x: 0, y: 0 };
    const out = untangle(points, GAP);
    expect(Object.keys(out)).toHaveLength(10);
    expect(allApart(out, GAP)).toBe(true);
  });
});
