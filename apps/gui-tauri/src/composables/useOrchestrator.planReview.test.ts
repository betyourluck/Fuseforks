/**
 * 無人の計画と検証役（Spec 53 P3）の投影規律。
 *
 * 固定するのは 4 点:
 * - 起動時にスイッチと既定の検証役を読む — **画面の再読み込みではコアが生きている**ので、
 *   スイッチが ON のまま戻ってくることがある（起動直後なら必ず OFF）
 * - `planReviewBypassChanged` でスイッチの投影が追従する
 * - 既定の検証役の保存が成功したら投影を書き換える
 * - 承認時の検証役は**押したときの選択**を IPC へ渡す（村の既定を差し込まない）
 */
import { describe, expect, it, vi } from "vitest";

import type { CoreEvent } from "../types";

const h = vi.hoisted(() => ({
  bootStatus: vi.fn(async () => ({ ready: true, error: null })),
  listAgents: vi.fn(async () => []),
  listTopology: vi.fn(async () => []),
  listTopologyPositions: vi.fn(async () => ({})),
  listModelTemplates: vi.fn(async () => []),
  listRoles: vi.fn(async () => []),
  listGroups: vi.fn(async () => []),
  listRagSources: vi.fn(async () => []),
  listMessages: vi.fn(async () => []),
  listPlanWaves: vi.fn(async () => []),
  workspacePath: vi.fn(async () => "C:\\workspace"),
  currentSession: vi.fn(async () => "session_1"),
  getLanguage: vi.fn(async () => "ja"),
  listCommandRequests: vi.fn(async () => []),
  getUserName: vi.fn(async () => null),
  getUserIcon: vi.fn(async () => null),
  mcpHostStatus: vi.fn(async () => ({
    enabled: false,
    listening: false,
    port: 39641,
    token: null,
    lastError: null,
  })),
  // 画面の再読み込みの形 — コアではスイッチが ON・既定の検証役が居る。
  getPlanReviewBypass: vi.fn(async () => true),
  getRunApproval: vi.fn(async () => "required"),
  setRunApproval: vi.fn(async () => undefined),
  getDefaultVerifier: vi.fn(async () => "agent_v"),
  getExternalName: vi.fn(async () => null),
  getExternalIcon: vi.fn(async () => null),
  listSessions: vi.fn(async () => []),
  getAgentIcon: vi.fn(async () => null),
  setPlanReviewBypass: vi.fn(async () => {}),
  setDefaultVerifier: vi.fn(async () => {}),
  dispatchPlanWave: vi.fn(async () => {}),
  handler: null as ((e: { payload: CoreEvent }) => void) | null,
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (_name: string, cb: (e: { payload: CoreEvent }) => void) => {
    h.handler = cb;
    return () => {};
  }),
}));

vi.mock("../lib/ipc", () => ({
  ...h,
  toErrorPayload: (error: unknown) => error,
}));

import { useOrchestrator } from "./useOrchestrator";

function fire(event: CoreEvent): void {
  h.handler!({ payload: event });
}

describe("無人の計画と検証役の投影", () => {
  it("起動時にスイッチと既定の検証役を読み、イベントでスイッチが追従する", async () => {
    const orchestrator = useOrchestrator();
    await orchestrator.init();
    expect(orchestrator.state.planReviewBypass).toBe(true);
    expect(orchestrator.state.defaultVerifier).toBe("agent_v");

    fire({ type: "planReviewBypassChanged", on: false });
    expect(orchestrator.state.planReviewBypass).toBe(false);
  });

  it("スイッチの切り替えと既定の検証役の保存は、成功したら投影を書き換える", async () => {
    const orchestrator = useOrchestrator();
    await orchestrator.init();

    expect(await orchestrator.setPlanReviewBypass(true)).toBe(true);
    expect(h.setPlanReviewBypass).toHaveBeenCalledWith(true);
    expect(orchestrator.state.planReviewBypass).toBe(true);

    expect(await orchestrator.setDefaultVerifier(null)).toBe(true);
    expect(h.setDefaultVerifier).toHaveBeenCalledWith(null);
    expect(orchestrator.state.defaultVerifier).toBeNull();
  });

  it("承認時の検証役は押したときの選択をそのまま渡し、省略は「なし」", async () => {
    const orchestrator = useOrchestrator();
    await orchestrator.init();
    // **既定の検証役をここで、操作を通して置く。** 状態はモジュールで 1 つ・`init` は
    // 2 回目以降は何もしないので、前のテストが既定を null にしていると「省略に既定を
    // 差し込む」壊れ方を判別できない（初版はこの穴でミューテーションが緑のまま通った）。
    // **直接の代入では置けない** — 公開される状態は読み取り専用で、代入は黙って捨てられる
    // （2 版目はそれでまた緑のまま通った）。
    await orchestrator.setDefaultVerifier("agent_v");
    expect(orchestrator.state.defaultVerifier).toBe("agent_v");
    const tasks = [{ to: "agent_w", message: "調べて" }];

    await orchestrator.dispatchPlanWave(7, tasks, "agent_x");
    expect(h.dispatchPlanWave).toHaveBeenLastCalledWith(7, tasks, "agent_x");

    // 省略は null（なし）。**村の既定（agent_v）を差し込まない** — 押したときの値が真実。
    await orchestrator.dispatchPlanWave(8, tasks);
    expect(h.dispatchPlanWave).toHaveBeenLastCalledWith(8, tasks, null);
  });
});
