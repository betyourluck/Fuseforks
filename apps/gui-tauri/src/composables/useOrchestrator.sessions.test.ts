/**
 * 会話（セッション）の投影規律（Spec 12 P3）のテスト。
 *
 * 固定するのは 2 点:
 * - `conversationCleared` → `sessionSwitched` の順で届いたとき、**空にした直後に
 *   コア側の復元結果を引き直す**。開き直しと分岐ではコアが会話ログを戻して
 *   おり、`conversationCleared` の空のままにすると画面だけが白紙に見える
 * - `conversationCleared` は**通知を出さない**。この 1 本は新規チャットでも
 *   開き直しでも分岐でも同じように飛ぶので、「新規チャットを開始しました」と
 *   言うと開き直しのときに嘘になる
 */
import { describe, expect, it, vi } from "vitest";

import type { AgentMessage, CoreEvent, SessionSummary } from "../types";

const RESTORED: AgentMessage[] = [
  {
    id: "m1",
    from: { kind: "user" },
    to: { kind: "agent", id: "agent_01" },
    content: "開き直した会話の発話",
    tokens: 0,
    tsMs: 1000,
    hop: 0,
  },
];

const SESSIONS: SessionSummary[] = [
  {
    id: "session_2",
    meta: {
      title: "開き直した会話の発話",
      createdAt: 900,
      updatedAt: 1000,
      recordCount: 3,
    },
  },
];

const h = vi.hoisted(() => ({
  bootStatus: vi.fn(async () => ({ ready: true, error: null })),
  listAgents: vi.fn(async () => []),
  listTopology: vi.fn(async () => []),
  listTopologyPositions: vi.fn(async () => ({})),
  listModelTemplates: vi.fn(async () => []),
  listRagSources: vi.fn(async () => []),
  listMessages: vi.fn(async () => [] as AgentMessage[]),
  listPlanWaves: vi.fn(async () => []),
  workspacePath: vi.fn(async () => "C:\\workspace"),
  currentSession: vi.fn(async () => "session_1"),
  getLanguage: vi.fn(async () => "ja"),
  listSessions: vi.fn(async () => [] as SessionSummary[]),
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

/** マイクロタスクを 1 周させる（sessionSwitched の取り直しは非同期）。 */
async function settle(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

describe("会話の投影規律（Spec 12）", () => {
  it("切り替えの 2 本を受けて、空にした直後に復元結果を引き直す", async () => {
    const orchestrator = useOrchestrator();
    await orchestrator.init();

    expect(orchestrator.state.currentSessionId).toBe("session_1");

    // コアは切り替えの直前に会話ログを戻している。
    h.listMessages.mockResolvedValueOnce(RESTORED);
    h.listSessions.mockResolvedValueOnce(SESSIONS);

    fire({ type: "conversationCleared" });
    expect(orchestrator.state.messages).toHaveLength(0);
    expect(orchestrator.state.toolRuns).toHaveLength(0);

    fire({ type: "sessionSwitched", sessionId: "session_2" });
    await settle();

    expect(orchestrator.state.currentSessionId).toBe("session_2");
    expect(orchestrator.state.messages).toHaveLength(1);
    expect(orchestrator.state.messages[0].content).toBe("開き直した会話の発話");
    expect(orchestrator.state.sessions).toHaveLength(1);
  });

  it("conversationCleared は通知を出さない（何が起きたかは操作した側が知っている）", async () => {
    const orchestrator = useOrchestrator();
    await orchestrator.init();
    const before = orchestrator.state.toasts.length;

    fire({ type: "conversationCleared" });
    fire({ type: "sessionSwitched", sessionId: "session_3" });
    await settle();

    expect(orchestrator.state.toasts.length).toBe(before);
  });
});
