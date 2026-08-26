import { describe, expect, it } from "vitest";

import { drawDirection, edgeIsLive } from "./kizunaEdges";

/** 左ペインの並び。a が上、b が下。 */
const order = (id: string) => ({ a: 0, b: 1, c: 2 })[id] ?? Number.MAX_SAFE_INTEGER;

describe("drawDirection", () => {
  it("双方向は、左ペインで上にいるほうを source にする", () => {
    expect(drawDirection("b", "a", true, order)).toEqual(["a", "b"]);
  });

  it("既に上→下なら並べ替えない", () => {
    expect(drawDirection("a", "b", true, order)).toEqual(["a", "b"]);
  });

  it("一方向は触らない（向きが事実そのものだから）", () => {
    // **ここが単純化で壊れる場所。** 「いつも sort すればいい」にすると
    // `b→a` の絆が `a→b` として描かれ、**矢印が逆を指す**。
    expect(drawDirection("b", "a", false, order)).toEqual(["b", "a"]);
    expect(drawDirection("a", "b", false, order)).toEqual(["a", "b"]);
  });

  it("並び順を入れ替えると描画方向も入れ替わる", () => {
    // 「左ペインに従う」が本当に効いているかは、**並びを変えて対で見る**。
    // 片方だけだと、id の辞書順で並べる実装でも通ってしまう（a < b なので）。
    const reversed = (id: string) => ({ a: 1, b: 0 })[id] ?? Number.MAX_SAFE_INTEGER;
    expect(drawDirection("a", "b", true, reversed)).toEqual(["b", "a"]);
  });

  it("引けない id は末尾へ倒す（描画順が揺れない）", () => {
    expect(drawDirection("zombie", "a", true, order)).toEqual(["a", "zombie"]);
  });

  it("同じ順序なら受け取った並びのまま（比較が不定にならない）", () => {
    const flat = () => 0;
    expect(drawDirection("b", "a", true, flat)).toEqual(["b", "a"]);
  });
});

describe("edgeIsLive", () => {
  const runningOnly = (ids: string[]) => (id: string) => ids.includes(id);

  it("両端が稼働しているときだけ生きる", () => {
    expect(edgeIsLive({ source: "a", target: "b" }, runningOnly(["a", "b"]))).toBe(true);
  });

  it("片端だけの稼働では生きない（source 側でも target 側でも）", () => {
    // **ここが 2026-08-27 の変更点。** 旧規則は source の稼働だけで発火し、
    // 片方しか稼働していない辺が流れて「両方稼働」と誤認させていた。
    expect(edgeIsLive({ source: "a", target: "b" }, runningOnly(["a"]))).toBe(false);
    expect(edgeIsLive({ source: "a", target: "b" }, runningOnly(["b"]))).toBe(false);
  });

  it("両端とも停止なら生きない", () => {
    expect(edgeIsLive({ source: "a", target: "b" }, runningOnly([]))).toBe(false);
  });
});
