import { describe, expect, it } from "vitest";

import {
  deriveId,
  isJudgeHidden,
  judgeEdges,
  judgeSpecOf,
  judgeStatusText,
  judgesInOrder,
  linkedGroupOf,
  nextJudgeOrder,
  shownJudges,
} from "./judges";
import type { AgentGroup, JudgeStatus, JudgeView } from "../types";

function view(id: string, status: JudgeStatus, targets: string[] = [], order = 0): JudgeView {
  return {
    id,
    name: id,
    order,
    enabled: status.kind !== "disabled",
    hideWithGroup: null,
    status,
    targets,
    outline: null,
  };
}

describe("deriveId", () => {
  it("名前から id を導く", () => {
    expect(deriveId("Router A", [])).toBe("router_a");
  });

  it("サーヴァントと判断役のどちらの id とも衝突しない（名前空間は 1 つ）", () => {
    // 呼び手は 2 つの一覧を 1 つにまとめて渡す。片方だけだと、コアが拒む id を作る。
    const servants = ["router"];
    const judges = ["router_2"];
    expect(deriveId("router", [...servants, ...judges])).toBe("router_3");
  });

  it("英数字が残らない名前は fallback を土台にする", () => {
    expect(deriveId("振り分け役", [], "judge")).toBe("judge");
    expect(deriveId("振り分け役", ["judge"], "judge")).toBe("judge_2");
  });
});

describe("並び", () => {
  it("order の昇順", () => {
    const list = [view("b", { kind: "active" }, [], 2), view("a", { kind: "active" }, [], 1)];
    expect(judgesInOrder(list).map((j) => j.id)).toEqual(["a", "b"]);
  });

  it("新しい判断役は末尾", () => {
    expect(nextJudgeOrder([])).toBe(0);
    expect(nextJudgeOrder([view("a", { kind: "active" }, [], 4)])).toBe(5);
  });
});

describe("judgeStatusText", () => {
  it("有効のときだけ active", () => {
    expect(judgeStatusText({ kind: "active" }).active).toBe(true);
    expect(judgeStatusText({ kind: "noFile" }).active).toBe(false);
    expect(judgeStatusText({ kind: "noJudgeModel" }).active).toBe(false);
    expect(judgeStatusText({ kind: "disabled" }).active).toBe(false);
  });

  it("利用者が無効にした判断役からは破線を描かない（行き先はファイルに残っていても）", () => {
    expect(judgeEdges([view("a", { kind: "disabled" }, ["agent_3"])])).toEqual([]);
  });

  it("検査に落ちた理由と場所を運ぶ", () => {
    const text = judgeStatusText({ kind: "invalid", location: "rules[2].when", message: "x" });
    expect(text.key).toBe("judges.status.invalid");
    expect(text.params).toEqual({ location: "rules[2].when", message: "x" });
  });

  it("行き先の欠けを名指しする", () => {
    const text = judgeStatusText({ kind: "missingTargets", targets: ["agent_3", "agent_9"] });
    expect(text.params.targets).toBe("agent_3, agent_9");
  });
});

describe("judgeEdges", () => {
  it("有効な判断役の to から線を合成する", () => {
    const edges = judgeEdges([view("j", { kind: "active" }, ["agent_3", "agent_10"])]);
    expect(edges).toEqual([
      { from: "j", to: "agent_3" },
      { from: "j", to: "agent_10" },
    ]);
  });

  it("無効な判断役からは線を描かない（ツールが生えないのに線が出る形を作らない）", () => {
    const edges = judgeEdges([
      view("a", { kind: "noJudgeModel" }, ["agent_3"]),
      view("b", { kind: "missingTargets", targets: ["gone"] }, ["agent_3", "gone"]),
    ]);
    expect(edges).toEqual([]);
  });

  it("同じ相手へは 1 本", () => {
    const edges = judgeEdges([view("j", { kind: "active" }, ["agent_3", "agent_3"])]);
    expect(edges).toHaveLength(1);
  });
});

describe("非表示に連動する（2026-11-29）", () => {
  const groups: AgentGroup[] = [
    { id: "g-invest", name: "投資", batchStart: true },
    { id: "g-research", name: "調査", batchStart: true },
  ];
  const linked = (id: string, hideWithGroup: string | null): JudgeView => ({
    ...view(id, { kind: "active" }),
    hideWithGroup,
  });

  it("連動先のグループを隠すと隠れ、出すと戻る", () => {
    const judge = linked("madoguchi", "g-research");
    expect(isJudgeHidden(judge, groups, ["g-research"])).toBe(true);
    expect(isJudgeHidden(judge, groups, [])).toBe(false);
    // 別のグループを隠しても隠れない。
    expect(isJudgeHidden(judge, groups, ["g-invest"])).toBe(false);
  });

  it("「なし」は何を隠しても隠れない（今までどおり）", () => {
    expect(isJudgeHidden(linked("j", null), groups, ["g-invest", "g-research"])).toBe(false);
  });

  it("消したグループ・引けない id は連動しない（非表示の集合に残っていても）", () => {
    // 保存値の hiddenGroups には消したグループの id が残りうる（useHiddenGroups は読み飛ばさず持つ）。
    const judge = linked("j", "g-deleted");
    expect(isJudgeHidden(judge, groups, ["g-deleted"])).toBe(false);
    expect(linkedGroupOf(judge, groups)).toBeNull();
    expect(linkedGroupOf(linked("j", "g-invest"), groups)).toBe("g-invest");
  });

  it("地図に出す判断役は隠れたものを落とし、並びを保つ", () => {
    const judges = [linked("a", "g-research"), linked("b", null), linked("c", "g-invest"), linked("d", "g-research")];
    expect(shownJudges(judges, groups, ["g-research"]).map((j) => j.id)).toEqual(["b", "c"]);
    expect(shownJudges(judges, groups, []).map((j) => j.id)).toEqual(["a", "b", "c", "d"]);
  });

  it("更新の形は一覧の写しから組み、連動先を落とさない（トグルで外れない）", () => {
    const judge = { ...linked("j", "g-invest"), name: "窓口", order: 3 };
    expect(judgeSpecOf(judge, groups, { enabled: false })).toEqual({
      id: "j",
      name: "窓口",
      order: 3,
      enabled: false,
      hideWithGroup: "g-invest",
    });
    expect(judgeSpecOf(judge, groups, { hideWithGroup: null }).hideWithGroup).toBeNull();
    // 名前だけ直すときも、無効にしてある判断役は無効のまま（コアの既定＝有効へ戻さない）。
    const off = { ...judge, enabled: false };
    expect(judgeSpecOf(off, groups, { name: "受付" })).toMatchObject({ name: "受付", enabled: false, hideWithGroup: "g-invest" });
  });

  it("更新の形では引けない id を null へ正規化する（group_contract 凍結 3 と同じ）", () => {
    expect(judgeSpecOf(linked("j", "g-deleted"), groups).hideWithGroup).toBeNull();
  });
});
