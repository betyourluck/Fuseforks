/**
 * `snapshotToSpec`（Spec 29 D4）の規律。
 *
 * 留めるのは 3 点:
 * - **全欄が投影の値で写る** — 新しい欄の欠落はコンパイラが捕まえるが、
 *   「写したが値が違う」（既定値の直書き）は型が合ってしまう。既定値と
 *   重ならない値の投影で全欄を突き合わせる
 * - **overrides は指定した欄だけを変える**（#87 — 検査したい欄と、
 *   変わらないことを見る欄を分ける）
 * - **配列は複製される** — 下書き編集が投影を汚さない
 */
import { describe, expect, it } from "vitest";

import { snapshotToSpec } from "./agentSpec";
import type { AgentSnapshot } from "../types";

/**
 * **全欄が既定値と重ならない**投影。`hearsRoomLog` / `batchStart` は
 * serde の既定が true なので false にする — 既定値を直書きした実装が
 * 緑になるのを防ぐ側。
 */
function snapshot(): AgentSnapshot {
  return {
    id: "agent_7",
    name: "検算役",
    model: "mock-model",
    modelTemplateId: "tpl_x",
    status: "running",
    uptimeSecs: 42,
    totalTokens: 999,
    promptTokens: 500,
    cachedTokens: 250,
    lastError: null,
    ragSources: ["D:\\docs\\a", "D:\\docs\\b"],
    connectedAgents: ["agent_2", "agent_9"],
    order: 5,
    workDir: "D:\\work\\proj",
    maxToolIterations: 24,
    enabledTools: ["grep", "fd"],
    hearsRoomLog: false,
    allowHandoff: false,
    planReview: true,
    batchStart: false,
    roleId: "role_auditor",
  };
}

describe("snapshotToSpec", () => {
  it("全欄が投影の値で写る", () => {
    // **丸ごと固定が正しい場所** — 全欄の複写そのものがこの関数の仕事なので、
    // #87 の「欄を選ぶ」の例外側（golden と同じ扱い）。
    expect(snapshotToSpec(snapshot())).toEqual({
      id: "agent_7",
      name: "検算役",
      modelTemplateId: "tpl_x",
      ragSources: ["D:\\docs\\a", "D:\\docs\\b"],
      connectedAgents: ["agent_2", "agent_9"],
      order: 5,
      workDir: "D:\\work\\proj",
      maxToolIterations: 24,
      enabledTools: ["grep", "fd"],
      hearsRoomLog: false,
    allowHandoff: false,
      planReview: true,
      batchStart: false,
      roleId: "role_auditor",
    });
  });

  it("overrides は指定した欄だけを変える", () => {
    const spec = snapshotToSpec(snapshot(), { workDir: "E:\\other" });

    expect(spec.workDir).toBe("E:\\other");
    // 変わらない側を対で見る（指定していない欄が巻き込まれない）。
    expect(spec.batchStart).toBe(false);
    expect(spec.name).toBe("検算役");
  });

  it("null での上書きも通る（未設定へ戻す形）", () => {
    expect(snapshotToSpec(snapshot(), { workDir: null }).workDir).toBeNull();
  });

  it("配列は複製され、返した spec を編集しても投影が汚れない", () => {
    const source = snapshot();
    const spec = snapshotToSpec(source);

    spec.ragSources.push("D:\\docs\\c");
    spec.connectedAgents.push("agent_1");
    spec.enabledTools?.push("run");

    expect(source.ragSources).toEqual(["D:\\docs\\a", "D:\\docs\\b"]);
    expect(source.connectedAgents).toEqual(["agent_2", "agent_9"]);
    expect(source.enabledTools).toEqual(["grep", "fd"]);
  });

  it("enabledTools が null（既定に従う）のときは null のまま", () => {
    // `?? []` で書くと「既定に従う」が「何も提示しない」へ化ける — 意味が逆転する。
    const source = { ...snapshot(), enabledTools: null };
    expect(snapshotToSpec(source).enabledTools).toBeNull();
  });
});
