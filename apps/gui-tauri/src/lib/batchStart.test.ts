import { describe, expect, it } from "vitest";

import { batchAction, batchLabel } from "./batchStart";
import type { AgentSnapshot, AgentStatus } from "../types";

function agent(
  id: string,
  status: AgentStatus,
  batchStart = true,
): AgentSnapshot {
  return {
    id,
    name: id,
    model: "mock",
    modelTemplateId: "tpl",
    roleId: null,
    groupId: null,
    status,
    uptimeSecs: 0,
    totalTokens: 0,
    promptTokens: 0,
    cachedTokens: 0,
    lastPromptTokens: null,
    ragSources: [],
    connectedAgents: [],
    order: 0,
    workDir: null,
    maxToolIterations: null,
    enabledTools: null,
    hearsRoomLog: true,
    allowHandoff: true,
    planReview: false,
    batchStart,
    lastError: null,
  };
}

describe("batchAction", () => {
  it("停止中の対象があれば起動する", () => {
    const action = batchAction([agent("a", "idle"), agent("b", "idle")]);
    expect(action).toEqual({ mode: "start", targets: ["a", "b"] });
  });

  it("対象外（batchStart: false）は起こさない", () => {
    const action = batchAction([agent("a", "idle"), agent("b", "idle", false)]);
    expect(action.targets).toEqual(["a"]);
  });

  it("失敗した相手も起動対象に含める（再起動の手段になる）", () => {
    const action = batchAction([agent("a", "failed")]);
    expect(action).toEqual({ mode: "start", targets: ["a"] });
  });

  it("対象が全員稼働中なら停止へ役が変わる", () => {
    const action = batchAction([agent("a", "running"), agent("b", "running")]);
    expect(action).toEqual({ mode: "stop", targets: ["a", "b"] });
  });

  it("混在状態では起動を優先し、動いている相手には触らない", () => {
    // 一部だけ落ちている状態で押したときに期待されるのは「揃える」方向。
    // ここで全部止めると、動いていた側の会話を巻き添えに殺す。
    const action = batchAction([agent("a", "running"), agent("b", "idle")]);
    expect(action).toEqual({ mode: "start", targets: ["b"] });
  });

  it("起動処理中は停止へ役を変えない（二度押しで起きた直後の個体を止めない）", () => {
    // 旧規則（starting も停止対象 =「取り消せる」）は 2026-08-27 に反転した。
    // 一括起動の直後は全員がこの状態なので、ここで stop に変わると
    // 二度押しがそのまま誤停止になる。
    const action = batchAction([agent("a", "starting")]);
    expect(action).toEqual({ mode: "none", targets: [] });
  });

  it("稼働と起動処理中の混在でも、処理が明けるまで停止は封じる", () => {
    // 起動は 1 体ずつ running へ抜ける。抜けた順に停止対象へ入れると
    // 二度押しの窓が縮むだけで消えないので、遷移中が 1 体でもいれば none。
    const action = batchAction([agent("a", "running"), agent("b", "starting")]);
    expect(action).toEqual({ mode: "none", targets: [] });
  });

  it("停止処理中は触らない（遷移中に叩くと競合する）", () => {
    const action = batchAction([agent("a", "stopping")]);
    expect(action).toEqual({ mode: "none", targets: [] });
  });

  it("対象が 0 体なら操作なし", () => {
    expect(batchAction([agent("a", "idle", false)])).toEqual({
      mode: "none",
      targets: [],
    });
    expect(batchAction([])).toEqual({ mode: "none", targets: [] });
  });
});

describe("batchLabel", () => {
  it("起動と停止で記号が変わる", () => {
    expect(batchLabel({ mode: "start", targets: ["a"] }, 1).icon).toBe("▶");
    expect(batchLabel({ mode: "stop", targets: ["a"] }, 1).icon).toBe("■");
  });

  it("押せないときは理由を出す（対象なしと遷移中を区別する）", () => {
    expect(batchLabel({ mode: "none", targets: [] }, 0).titleKey).toBe(
      "agentList.batchNoTargets",
    );
    expect(batchLabel({ mode: "none", targets: [] }, 2).titleKey).toBe(
      "agentList.batchTransitioning",
    );
  });
});
