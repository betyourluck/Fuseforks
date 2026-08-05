/**
 * コマンド承認の投影（Spec 20）。
 *
 * 画面は `state.commandRequests` から一覧とバッジを描くので、**投影が古いまま
 * 残ると「承認したのに消えない」**、**失敗したのに消えると「却下できたように
 * 見える」**になる。どちらもコアのテストでは出ない（コアは正しく動いている）。
 *
 * **並び順は問わない。** 一覧の並びは実機の溜まり方を見てから決める（Spec の
 * Notes 2）ので、テストが先に順序を固定すると、決めた時点で壊れるのはテスト側。
 */
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { CommandPolicyView } from "../types";

const view = (pending: string[], broken = false): CommandPolicyView => ({
  agentId: "agent_a",
  name: "ザリ",
  pending: pending.map((command) => ({
    command,
    args: ["log"],
    firstRequestedAtMs: 1,
    count: 1,
  })),
  broken,
});

const h = vi.hoisted(() => ({
  listAgents: vi.fn(async () => []),
  listTopology: vi.fn(async () => []),
  listTopologyPositions: vi.fn(async () => ({})),
  listModelTemplates: vi.fn(async () => []),
  listRoles: vi.fn(async () => []),
  listRagSources: vi.fn(async () => []),
  getAgentIcon: vi.fn(async () => null),
  listCommandRequests: vi.fn(),
  approveCommand: vi.fn(),
  rejectCommand: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));

vi.mock("../lib/ipc", () => ({
  ...h,
  toErrorPayload: (error: unknown) => ({
    code: "TEST",
    message: String(error),
    detail: null,
    agentId: null,
    retryable: false,
  }),
}));

import { useOrchestrator } from "./useOrchestrator";

describe("コマンド承認の投影", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    h.listCommandRequests.mockResolvedValue([view(["git", "curl"])]);
  });

  it("引き直すと一覧が投影に載る", async () => {
    const orchestrator = useOrchestrator();
    await expect(orchestrator.refreshCommandRequests()).resolves.toBe(true);
    expect(orchestrator.state.commandRequests[0].pending).toHaveLength(2);
  });

  it("承認すると一覧を引き直す（承認したものが消える）", async () => {
    const orchestrator = useOrchestrator();
    await orchestrator.refreshCommandRequests();

    h.approveCommand.mockResolvedValue("applied");
    h.listCommandRequests.mockResolvedValue([view(["curl"])]);

    await expect(
      orchestrator.approveCommand("agent_a", "git", ["log"], true),
    ).resolves.toBe("applied");
    expect(h.approveCommand).toHaveBeenCalledWith("agent_a", "git", ["log"], true);
    expect(orchestrator.state.commandRequests[0].pending).toHaveLength(1);
  });

  it("`notFound` はそのまま返る（画面が告げるため）", async () => {
    const orchestrator = useOrchestrator();
    h.approveCommand.mockResolvedValue("notFound");
    await expect(
      orchestrator.approveCommand("agent_a", "git", ["log"], false),
    ).resolves.toBe("notFound");
  });

  it("失敗したら null で、投影は引き直した内容のまま", async () => {
    const orchestrator = useOrchestrator();
    await orchestrator.refreshCommandRequests();

    h.rejectCommand.mockRejectedValueOnce(new Error("書けません"));
    await expect(
      orchestrator.rejectCommand("agent_a", "git", ["log"], false),
    ).resolves.toBeNull();
    // 失敗したのだから、判断待ちは消えていない。
    expect(orchestrator.state.commandRequests[0].pending).toHaveLength(2);
  });

  it("読めなかった個体は broken のまま運ぶ（既定で埋めない）", async () => {
    const orchestrator = useOrchestrator();
    h.listCommandRequests.mockResolvedValue([view([], true)]);
    await orchestrator.refreshCommandRequests();

    const [only] = orchestrator.state.commandRequests;
    expect(only.broken).toBe(true);
    expect(only.pending).toHaveLength(0);
  });
});
