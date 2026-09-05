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
  listTopologyPositions: vi.fn(async () => ({})),
  listModelTemplates: vi.fn(async () => []),
  listRoles: vi.fn(async () => []),
  // Spec 51。refreshAll が list_groups も引くので、無いと起動の網に掛かる（意図した網）。
  listGroups: vi.fn(async () => []),
  listRagSources: vi.fn(async () => []),
  listMessages: vi.fn(async () => []),
  listPlanWaves: vi.fn(async (): Promise<PlanWaveRecord[]> => [
    {
      planId: 1,
      agentId: "agent_lead",
      wave: 1,
      state: "dispatched",
      startedAtMs: 1000,
      tasks: [
        { to: "agent_w1", state: "answered", elapsedMs: 42, msgChars: 10 },
      ],
      bundleChars: 40,
      elapsedMs: 50,
    },
  ]),
  workspacePath: vi.fn(async () => "C:\\workspace"),
  currentSession: vi.fn(async () => "session_1"),
  getLanguage: vi.fn(async () => "ja"),
  listCommandRequests: vi.fn(async () => []),
  getUserName: vi.fn(async () => null),
  getUserIcon: vi.fn(async () => null),
  listSessions: vi.fn(async () => []),
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

  it("提案は本文つきで現れ、承認が最終形で置き換え、破棄が本文を落とす（Spec 43）", async () => {
    const orchestrator = useOrchestrator();
    await orchestrator.init();

    // 提案（編集窓）。本文が編集 UI の読む真実として届く。
    fire({
      type: "planWaveProposed",
      planId: 100,
      agentId: "agent_lead",
      wave: 1,
      tasks: [
        { to: "agent_w1", message: "Aを調べて" },
        { to: "agent_w2", message: "Bを調べて" },
      ],
      startedAtMs: 5000,
    });
    const pending = orchestrator.state.planWaves.find((w) => w.planId === 100)!;
    expect(pending.state).toBe("pending");
    expect(pending.tasks[0].message).toBe("Aを調べて");
    expect(pending.tasks[0].msgChars).toBe(5);

    // 承認 = planWaveStarted が**人の最終形**でタスクを置き換える（1 件へ編集）。
    fire({
      type: "planWaveStarted",
      planId: 100,
      agentId: "agent_lead",
      wave: 1,
      tasks: [{ to: "agent_w2", msgChars: 7 }],
      startedAtMs: 6000,
    });
    // upsert は要素を新しいオブジェクトで差し替えるので、取り直して読む
    // （古い参照 `pending` は更新されない — reactive でも要素差し替えは別物）。
    const dispatched = orchestrator.state.planWaves.find((w) => w.planId === 100)!;
    expect(dispatched.state).toBe("dispatched");
    expect(dispatched.tasks).toHaveLength(1);
    expect(dispatched.tasks[0].to).toBe("agent_w2");
    expect(dispatched.tasks[0].message, "配送後は本文を持たない").toBeUndefined();

    // 確定済みを pending で巻き戻さない（list の応答が event より古い競合）。
    fire({
      type: "planWaveProposed",
      planId: 100,
      agentId: "agent_lead",
      wave: 1,
      tasks: [{ to: "agent_w1", message: "古い提案" }],
      startedAtMs: 5000,
    });
    expect(
      orchestrator.state.planWaves.find((w) => w.planId === 100)!.state,
      "波レベル状態の遷移は片方向（pending へ巻き戻らない）",
    ).toBe("dispatched");

    // 破棄。状態が閉じ、本文が落ちる。
    fire({
      type: "planWaveProposed",
      planId: 101,
      agentId: "agent_lead",
      wave: 2,
      tasks: [{ to: "agent_w1", message: "破棄される" }],
      startedAtMs: 7000,
    });
    fire({ type: "planWaveDiscarded", planId: 101 });
    const discarded = orchestrator.state.planWaves.find((w) => w.planId === 101)!;
    expect(discarded.state).toBe("discarded");
    expect(discarded.tasks[0].message).toBeUndefined();
  });
});
