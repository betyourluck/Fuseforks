/**
 * グループの純関数（Spec 51）。留めるのは契約の 4 つ — 引けない id は無所属 /
 * 無所属は隠れない / 全体 ▶ の門は 2 段で非表示を見ない / drop は並びと所属を
 * 1 回で返し、畳んだ区分けの個体を落とさない。
 */

import { describe, expect, it } from "vitest";
import {
  UNASSIGNED,
  batchEligible,
  commitDrop,
  isHidden,
  sectionize,
  settleSelection,
  visibleAgents,
  visibleEdges,
} from "./agentGroups";
import type { AgentGroup, TopologyEdge } from "../types";

const groups: AgentGroup[] = [
  { id: "g-research", name: "調査", batchStart: true },
  { id: "g-release", name: "リリース", batchStart: false },
];

function agent(id: string, order: number, groupId: string | null, batchStart = true) {
  return { id, order, groupId, batchStart };
}

// **わざと order と id の並びを食い違わせる**（id 順に整列する実装なら落ちる）。
const agents = [
  agent("z", 0, null),
  agent("a", 3, "g-research"),
  agent("m", 1, "g-release"),
  agent("orphan", 2, "g-deleted"),
  agent("b", 4, "g-research"),
];

describe("sectionize", () => {
  it("無所属 → グループの配列順で区分けし、各区分けは order 順。引けない id は無所属", () => {
    const sections = sectionize(agents, groups);
    expect(sections.map((s) => s.key)).toEqual([UNASSIGNED, "g-research", "g-release"]);
    expect(sections[0].agents.map((a) => a.id)).toEqual(["z", "orphan"]);
    expect(sections[1].agents.map((a) => a.id)).toEqual(["a", "b"]);
    expect(sections[2].agents.map((a) => a.id)).toEqual(["m"]);
  });

  it("誰も居ないグループの区分けも返す（見出しは落とし場）", () => {
    const sections = sectionize([agent("z", 0, null)], groups);
    expect(sections).toHaveLength(3);
    expect(sections[1].agents).toEqual([]);
  });
});

describe("visibility", () => {
  it("無所属と引けない id は決して隠れない", () => {
    expect(isHidden(agent("z", 0, null), groups, ["g-research", "g-release"])).toBe(false);
    expect(isHidden(agent("o", 0, "g-deleted"), groups, ["g-deleted"])).toBe(false);
    expect(isHidden(agent("a", 0, "g-research"), groups, ["g-research"])).toBe(true);
  });

  it("visibleAgents は一覧の並び順で、隠れた区分けの個体を落とす", () => {
    expect(visibleAgents(agents, groups, ["g-release"]).map((a) => a.id)).toEqual([
      "z",
      "orphan",
      "a",
      "b",
    ]);
  });

  it("選択は見えていればそのまま、隠れたら見えている先頭、可視 0 体なら null", () => {
    const visible = visibleAgents(agents, groups, ["g-release"]);
    expect(settleSelection("a", visible)).toBe("a");
    expect(settleSelection("m", visible)).toBe("z");
    expect(settleSelection("m", [])).toBeNull();
  });

  it("地図の辺は両端が見えているものだけ。隠れる数は全辺との差", () => {
    const edges: TopologyEdge[] = [
      { source: "z", target: "a" },
      { source: "a", target: "m" },
      { source: "m", target: "m2" },
    ];
    const visible = new Set(["z", "a"]);
    expect(visibleEdges(edges, visible)).toEqual([{ source: "z", target: "a" }]);
    expect(edges.length - visibleEdges(edges, visible).length).toBe(2);
  });
});

describe("batchEligible", () => {
  it("個体のトグルとグループのスイッチの 2 段。無所属と引けない id は個体のトグルだけ", () => {
    const eligible = batchEligible(
      [...agents, agent("off", 9, "g-research", false)],
      groups,
    ).map((a) => a.id);
    expect(eligible).toEqual(["z", "a", "orphan", "b"]);
    expect(eligible).not.toContain("m");
    expect(eligible).not.toContain("off");
  });
});

describe("commitDrop", () => {
  const sections = sectionize(agents, groups);

  it("同じ箱の中の移動は regroup が null で、並びだけ変わる", () => {
    const out = commitDrop(sections, { "g-research": ["b", "a"] }, "g-research", "b");
    expect(out.regroup).toBeNull();
    expect(out.order).toEqual(["z", "orphan", "b", "a", "m"]);
  });

  it("箱をまたぐと落ちた箱の所属を書き、畳んだ区分けの個体も並びに残る", () => {
    // z を「リリース」へ。リリースの箱が畳まれていても m は state の並びから入る。
    const out = commitDrop(
      sections,
      { [UNASSIGNED]: ["orphan"], "g-release": ["m", "z"] },
      "g-release",
      "z",
    );
    expect(out.regroup).toEqual({ id: "z", groupId: "g-release" });
    expect(out.order).toEqual(["orphan", "a", "b", "m", "z"]);
  });

  it("無所属の箱へ落ちると null を書く。引けない id は箱の中で動かしただけでも null へ正規化", () => {
    const out = commitDrop(sections, { [UNASSIGNED]: ["orphan", "z"] }, UNASSIGNED, "orphan");
    expect(out.regroup).toEqual({ id: "orphan", groupId: null });
    const same = commitDrop(sections, { [UNASSIGNED]: ["orphan", "z"] }, UNASSIGNED, "z");
    expect(same.regroup).toBeNull();
  });
});
