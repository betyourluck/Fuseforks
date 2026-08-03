/**
 * 割り込み停止の投影規律（Spec 10 Phase 3）。
 *
 * 固定するのは「停止要求中…」表示の寿命:
 * - 要求（interruptTurn / interruptAll）で立つ
 * - `turnInterrupted`（検知）で消える
 * - `agentTyping` の終了でも消える — 検知の前にターンが完走で逃げ切ると、
 *   turnInterrupted は永遠に来ない（切るものが無かった）。ここで消さないと
 *   「停止要求中…」が残り続ける
 * - interruptAll が立てるのは**今飛んでいる面々だけ** — 飛んでいない
 *   エージェントは切られないので、立てると解除する契機が来ない
 */
import { describe, expect, it, vi } from "vitest";

import type { CoreEvent } from "../types";

const h = vi.hoisted(() => ({
  bootStatus: vi.fn(async () => ({ ready: true, error: null })),
  listAgents: vi.fn(async () => []),
  listTopology: vi.fn(async () => []),
  listTopologyPositions: vi.fn(async () => ({})),
  listModelTemplates: vi.fn(async () => []),
  listRagSources: vi.fn(async () => []),
  listMessages: vi.fn(async () => []),
  listPlanWaves: vi.fn(async () => []),
  workspacePath: vi.fn(async () => "C:\\workspace"),
  currentSession: vi.fn(async () => "session_1"),
  getLanguage: vi.fn(async () => "ja"),
  listSessions: vi.fn(async () => []),
  getAgentIcon: vi.fn(async () => null),
  interruptTurn: vi.fn(async () => {}),
  interruptAll: vi.fn(async () => {}),
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

describe("割り込み停止の投影規律", () => {
  it("要求で立ち、turnInterrupted で消える", async () => {
    const orchestrator = useOrchestrator();
    await orchestrator.init();

    fire({ type: "agentTyping", agentId: "agent_a", active: true });
    await orchestrator.interruptTurn("agent_a");
    expect(h.interruptTurn).toHaveBeenCalledWith("agent_a");
    expect(orchestrator.state.interruptPending["agent_a"]).toBe(true);

    fire({ type: "turnInterrupted", agentId: "agent_a", turnSeq: 7 });
    expect(orchestrator.state.interruptPending["agent_a"]).toBeUndefined();
  });

  it("検知前に完走で逃げ切ったターンの要求表示は typing 終了で消える", async () => {
    const orchestrator = useOrchestrator();
    await orchestrator.init();

    fire({ type: "agentTyping", agentId: "agent_b", active: true });
    await orchestrator.interruptTurn("agent_b");
    expect(orchestrator.state.interruptPending["agent_b"]).toBe(true);

    // 周回境界に到達する前にターンが普通に終わった。
    fire({ type: "agentTyping", agentId: "agent_b", active: false });
    expect(orchestrator.state.interruptPending["agent_b"]).toBeUndefined();
  });

  it("interruptAll は飛行中の面々にだけ要求表示を立てる", async () => {
    const orchestrator = useOrchestrator();
    await orchestrator.init();

    fire({ type: "agentTyping", agentId: "agent_flying", active: true });
    await orchestrator.interruptAll();
    expect(h.interruptAll).toHaveBeenCalled();
    expect(orchestrator.state.interruptPending["agent_flying"]).toBe(true);
    expect(orchestrator.state.interruptPending["agent_idle"]).toBeUndefined();
  });
});
