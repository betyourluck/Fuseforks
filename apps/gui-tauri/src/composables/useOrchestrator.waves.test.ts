/**
 * 波ペインの投影規律（Spec 08）のテスト。
 *
 * 固定するのは data_contract の projection_rule:
 * - 再投影（list）と event の合流は planId の upsert で、**解決済みを running で
 *   巻き戻さない**（記録の遷移は片方向なので「進んでいる方を採る」が正しい）
 * - 波の完了は残った running を no_answer に倒す（コアの finish_wave と同じ後始末）
 * - 保持はコアのリングと同じ上限 50・古い方から捨てる
 */
import { describe, expect, it, vi } from "vitest";

import type { CoreEvent, PlanWaveRecord } from "../types";

const h = vi.hoisted(() => ({
  bootStatus: vi.fn(async () => ({ ready: true, error: null })),
  listAgents: vi.fn(async () => []),
  listTopology: vi.fn(async () => []),
  listModelTemplates: vi.fn(async () => []),
  listRagSources: vi.fn(async () => []),
  listMessages: vi.fn(async () => []),
  listPlanWaves: vi.fn(async (): Promise<PlanWaveRecord[]> => [
    {
      planId: 1,
      agentId: "agent_lead",
      wave: 1,
      startedAtMs: 1000,
      tasks: [
        { to: "agent_w1", state: "answered", elapsedMs: 42, msgChars: 10 },
      ],
      bundleChars: 40,
      elapsedMs: 50,
    },
  ]),
  workspacePath: vi.fn(async () => "C:\\workspace"),
  getAgentIcon: vi.fn(async () => null),
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

describe("波ペインの投影規律", () => {
  it("再投影と event が planId で合流し、解決済みは running で巻き戻らない", async () => {
    const orchestrator = useOrchestrator();
    await orchestrator.init();

    // 再投影が入っていること（実行済みの波が戻る）。
    expect(orchestrator.state.planWaves).toHaveLength(1);
    expect(orchestrator.state.planWaves[0].tasks[0].state).toBe("answered");

    // 同じ波の Started が遅れて届いても（list との重複）、answered を維持する。
    fire({
      type: "planWaveStarted",
      planId: 1,
      agentId: "agent_lead",
      wave: 1,
      tasks: [{ to: "agent_w1", msgChars: 10 }],
      startedAtMs: 1000,
    });
    expect(orchestrator.state.planWaves).toHaveLength(1);
    expect(
      orchestrator.state.planWaves[0].tasks[0].state,
      "解決済みを running で巻き戻さないこと",
    ).toBe("answered");
    expect(orchestrator.state.planWaves[0].bundleChars).toBe(40);

    // 新しい波は running で現れ、解決 → 完了と個別に進む。
    fire({
      type: "planWaveStarted",
      planId: 2,
      agentId: "agent_lead",
      wave: 2,
      tasks: [
        { to: "agent_w1", msgChars: 5 },
        { to: "agent_w2", msgChars: 6 },
      ],
      startedAtMs: 2000,
    });
    expect(orchestrator.state.planWaves).toHaveLength(2);
    const second = orchestrator.state.planWaves[1];
    expect(second.tasks.map((t) => t.state)).toEqual(["running", "running"]);

    fire({
      type: "planTaskResolved",
      planId: 2,
      to: "agent_w1",
      state: "handed_off",
      elapsedMs: 7,
    });
    expect(second.tasks[0].state).toBe("handed_off");
    expect(second.tasks[1].state).toBe("running");

    // 完了は残った running を no_answer に倒す（永遠の「実行中」を残さない）。
    fire({ type: "planWaveFinished", planId: 2, bundleChars: 99, elapsedMs: 80 });
    expect(second.bundleChars).toBe(99);
    expect(second.tasks[1].state).toBe("no_answer");

    // 保持はコアのリングと同じ上限 50・古い方から捨てる。
    for (let i = 3; i <= 52; i++) {
      fire({
        type: "planWaveStarted",
        planId: i,
        agentId: "agent_lead",
        wave: i,
        tasks: [{ to: "agent_w1", msgChars: 1 }],
        startedAtMs: i * 1000,
      });
    }
    expect(orchestrator.state.planWaves).toHaveLength(50);
    expect(
      orchestrator.state.planWaves[0].planId,
      "古い方から捨てること（planId 1・2 が押し出される）",
    ).toBe(3);
  });
});
