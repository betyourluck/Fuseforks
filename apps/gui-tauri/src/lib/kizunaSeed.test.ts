import { describe, expect, it } from "vitest";
import { seedPositions, visibleLayouts } from "./kizunaSeed";

describe("seedPositions", () => {
  it("既に置かれている座標には触れない", () => {
    // **ここが触ると「動かしたのに戻る」になる。**
    const placed = { a: { x: 10, y: 20 } };
    const out = seedPositions(["a"], placed);
    expect(out).toEqual({});
  });

  it("置かれていないものだけを埋める", () => {
    const out = seedPositions(["a", "b"], { a: { x: 0, y: 0 } });
    expect(Object.keys(out)).toEqual(["b"]);
  });

  it("同じ入力なら同じ座標（乱数を使っていない）", () => {
    const a = seedPositions(["x", "y", "z"], {});
    const b = seedPositions(["x", "y", "z"], {});
    expect(a).toEqual(b);
  });

  it("既にある塊の外側へ出す", () => {
    // 途中から加わった個体が**既存の配置の中へ割り込まない**。
    const placed = {
      a: { x: 0, y: 0 },
      b: { x: 100, y: 0 },
      c: { x: 50, y: 80 },
    };
    const out = seedPositions(["a", "b", "c", "new"], placed);
    const centre = { x: 50, y: 80 / 3 };
    const reach = Math.max(
      ...Object.values(placed).map((p) => Math.hypot(p.x - centre.x, p.y - centre.y)),
    );
    const added = out["new"]!;
    expect(Math.hypot(added.x - centre.x, added.y - centre.y)).toBeGreaterThan(reach);
  });

  it("全員が未配置なら重ならずに散る", () => {
    const ids = ["a", "b", "c", "d", "e"];
    const out = seedPositions(ids, {});
    for (let i = 0; i < ids.length; i += 1) {
      for (let j = i + 1; j < ids.length; j += 1) {
        const p = out[ids[i]!]!;
        const q = out[ids[j]!]!;
        expect(Math.hypot(p.x - q.x, p.y - q.y)).toBeGreaterThan(40);
      }
    }
  });

  it("空でも落ちない", () => {
    expect(seedPositions([], {})).toEqual({});
  });
});

describe("visibleLayouts", () => {
  it("隠れている個体の座標は載せない（Fit が隠れた位置へ寄らない）", () => {
    // v-network-graph の fitToContents は layouts の全件から外接矩形を取る。
    // 8 体のうち 2 体だけ見えているとき、残り 6 体の座標が layouts に残ると
    // 2 体は矩形の隅へ追いやられる（実機 2026-09-08）。
    const placed = {
      a: { x: 0, y: 0 },
      b: { x: 40, y: 0 },
      hidden: { x: 2000, y: 2000 },
    };
    const out = visibleLayouts(["a", "b"], placed);
    expect(Object.keys(out).sort()).toEqual(["a", "b"]);
    expect(out).not.toHaveProperty("hidden");
  });

  it("見えている個体の座標は写すだけで触らない", () => {
    const placed = { a: { x: 10, y: 20 } };
    expect(visibleLayouts(["a"], placed).a).toEqual({ x: 10, y: 20 });
  });

  it("見えていて未配置の個体だけを埋める", () => {
    const out = visibleLayouts(["a", "b"], { a: { x: 0, y: 0 } });
    expect(out.a).toEqual({ x: 0, y: 0 });
    expect(out.b).toBeDefined();
    expect(out.b).not.toEqual({ x: 0, y: 0 });
  });
});
