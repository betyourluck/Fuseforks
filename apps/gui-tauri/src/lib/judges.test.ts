import { describe, expect, it } from "vitest";

import { deriveId, judgeEdges, judgeStatusText, judgesInOrder, nextJudgeOrder } from "./judges";
import type { JudgeStatus, JudgeView } from "../types";

function view(id: string, status: JudgeStatus, targets: string[] = [], order = 0): JudgeView {
  return { id, name: id, order, status, targets };
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
