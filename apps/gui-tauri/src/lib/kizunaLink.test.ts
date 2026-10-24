/**
 * 絆の地図の上で絆を張る・切る規則（`kizunaLink.ts`）。
 *
 * 固定するのは、双方向の線の `forward` / `backward` が**描いた向き**で切れることと、
 * 判断役を起点にできないこと。どちらも画面の部品に直書きすると、走査では向きの取り違えを
 * 捕まえられない（ボタンの文言と切れる向きが食い違っても字面は同じ）。
 */
import { describe, expect, it } from "vitest";

import { canStartLink, edgeCutPlan } from "./kizunaLink";
import type { TieHolder } from "./kizunaDrop";

const agents: TieHolder[] = [
  { id: "a", connectedAgents: ["b", "c"] },
  { id: "b", connectedAgents: ["a"] },
  { id: "c", connectedAgents: [] },
];

describe("起点にできるか", () => {
  it("サーヴァントは起点にできる", () => {
    expect(canStartLink(agents, "a")).toBe(true);
  });

  it("サーヴァントの一覧に無い id（判断役）は起点にできない", () => {
    expect(canStartLink(agents, "judge_1")).toBe(false);
  });
});

describe("線の切り方", () => {
  const both = { source: "a", target: "b", bidirectional: true };

  it("双方向の both は両方の向きを切る", () => {
    expect(edgeCutPlan(agents, both, "both")).toEqual([
      { id: "a", connectedAgents: ["c"] },
      { id: "b", connectedAgents: [] },
    ]);
  });

  it("双方向の forward は描いた向き（source → target）だけを切る", () => {
    expect(edgeCutPlan(agents, both, "forward")).toEqual([{ id: "a", connectedAgents: ["c"] }]);
  });

  it("双方向の backward は逆向き（target → source）だけを切る", () => {
    expect(edgeCutPlan(agents, both, "backward")).toEqual([{ id: "b", connectedAgents: [] }]);
  });

  it("描いた向きが逆でも、forward は描いた向きで切る", () => {
    const drawn = { source: "b", target: "a", bidirectional: true };
    expect(edgeCutPlan(agents, drawn, "forward")).toEqual([{ id: "b", connectedAgents: [] }]);
  });

  it("片方向の線は切り方を見ず、その 1 本を切る", () => {
    const one = { source: "a", target: "c", bidirectional: false };
    for (const cut of ["both", "forward", "backward"] as const) {
      expect(edgeCutPlan(agents, one, cut)).toEqual([{ id: "a", connectedAgents: ["b"] }]);
    }
  });

  it("持っていない向きは書かない", () => {
    const stale = { source: "c", target: "a", bidirectional: false };
    expect(edgeCutPlan(agents, stale, "both")).toEqual([]);
  });

  it("他の接続先は並びごと残す", () => {
    const many: TieHolder[] = [{ id: "x", connectedAgents: ["p", "q", "r"] }, { id: "q", connectedAgents: ["x"] }];
    expect(edgeCutPlan(many, { source: "x", target: "q", bidirectional: true }, "forward")).toEqual([
      { id: "x", connectedAgents: ["p", "r"] },
    ]);
  });
});
