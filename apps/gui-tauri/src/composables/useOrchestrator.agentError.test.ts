/**
 * カードの失敗理由を閉じる（2026-08-11）。
 *
 * 起点は実機 — 529（Anthropic の過負荷）で落ちた個体のカードに、復帰したあとも
 * 前回の失敗が残り続けた。`AgentRecord.last_error` はプロセス寿命なので、
 * 閉じるのは**画面の 1 枚だけ**にしてコアへは送らない。
 *
 * **本題は 2 本目**。閉じた印が残ったままだと、その個体の**次の失敗が画面に
 * 出なくなる** — 消す機能が黙らせる機能に化ける。1 本目（閉じられること）
 * だけでは、常に隠す実装でも緑になる。
 */
import { describe, expect, it, vi } from "vitest";

import type { AgentSnapshot, CoreEvent } from "../types";

function snapshot(id: string, lastError: AgentSnapshot["lastError"]): AgentSnapshot {
  return {
    id,
    name: id,
    modelTemplateId: "tpl",
    roleId: null,
    status: "failed",
    connectedAgents: [],
    enabledTools: null,
    hearsRoomLog: true,
    allowHandoff: true,
    batchStart: false,
    workDir: null,
    ragSources: [],
    maxToolIterations: null,
    order: 0,
    uptimeSecs: 0,
    totalTokens: 0,
    promptTokens: 0,
    cachedTokens: 0,
    lastError,
  } as unknown as AgentSnapshot;
}

const failure = {
  code: "LLM_API",
  message: "API エラー (status=529)",
  detail: "overloaded_error",
  agentId: "agent_1",
  retryable: true,
};

const h = vi.hoisted(() => ({
  bootStatus: vi.fn(async () => ({ ready: true, error: null })),
  listAgents: vi.fn(async () => [] as unknown[]),
  listTopology: vi.fn(async () => []),
  listTopologyPositions: vi.fn(async () => ({})),
  listModelTemplates: vi.fn(async () => []),
  listRoles: vi.fn(async () => []),
  listRagSources: vi.fn(async () => []),
  listMessages: vi.fn(async () => []),
  listPlanWaves: vi.fn(async () => []),
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

describe("カードの失敗理由", () => {
  it("閉じられる。閉じても次の失敗は必ず出る", async () => {
    h.listAgents.mockResolvedValue([snapshot("agent_1", null)]);
    const orchestrator = useOrchestrator();
    await orchestrator.init();

    const agent = () => orchestrator.state.agents.find((a) => a.id === "agent_1")!;

    // 正の対照: 失敗する前は枠が無い。**これが無いと以下は何も証明しない**
    // （常に隠す実装でも「消えた」は緑になる）。
    expect(orchestrator.showsError(agent())).toBe(false);

    fire({ type: "agentFailed", agentId: "agent_1", error: failure } as CoreEvent);
    expect(orchestrator.showsError(agent()), "失敗したら出ること").toBe(true);

    orchestrator.dismissError("agent_1");
    expect(orchestrator.showsError(agent()), "閉じたら消えること").toBe(false);
    // **コアの投影は残す** — 消えるのは画面の 1 枚で、失敗した事実ではない。
    expect(agent().lastError, "lastError は消さないこと").not.toBeNull();

    // 本題: 次の失敗で開き直る。
    fire({ type: "agentFailed", agentId: "agent_1", error: failure } as CoreEvent);
    expect(
      orchestrator.showsError(agent()),
      "閉じた印が残って次の失敗を隠してはいけない",
    ).toBe(true);
  });

  it("閉じるのは押した 1 体だけ", async () => {
    h.listAgents.mockResolvedValue([
      snapshot("agent_1", null),
      snapshot("agent_2", null),
    ]);
    const orchestrator = useOrchestrator();
    await orchestrator.init();

    const at = (id: string) => orchestrator.state.agents.find((a) => a.id === id)!;

    fire({ type: "agentFailed", agentId: "agent_1", error: failure } as CoreEvent);
    fire({ type: "agentFailed", agentId: "agent_2", error: failure } as CoreEvent);
    orchestrator.dismissError("agent_1");

    expect(orchestrator.showsError(at("agent_1"))).toBe(false);
    expect(
      orchestrator.showsError(at("agent_2")),
      "他の個体の失敗まで消してはいけない",
    ).toBe(true);
  });
});
