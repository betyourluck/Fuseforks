/**
 * 会話の単位で画面を絞る投影（Spec 75 P2 — D5・D6・D10）。
 *
 * 処理は依頼した会話に結び付き、別の会話を開いている間も走り続ける。固定するのは:
 * - 入力中の印（`typing`）は個体の単位のまま、根の会話（`typingSession`）を別に持つ
 * - ツール行は開いている会話の分だけ積む。カードの直近のツールと周回数は個体の単位のまま
 * - 開いていない会話にサーヴァントの答えが書かれたら、知らせ（「開く」つき）を会話ごとに 1 枚
 *   （利用者の発話・System 行では出さない。開いている会話への書き込みでも出さない）
 * - その会話が開かれたら知らせを閉じ、承認画面の投影を取り直す（印は（個体, 会話）で持つ）
 * - 波は撒いた・提示した会話を持つ（`"-"` = 保存先の無い村は `null`）
 */
import { describe, expect, it, vi } from "vitest";

import type { AgentMessage, CoreEvent, SessionSummary } from "../types";

const SESSIONS: SessionSummary[] = [
  {
    id: "session_9",
    meta: { title: "予定の会話", createdAt: 900, updatedAt: 1000, recordCount: 3 },
  },
];

const h = vi.hoisted(() => ({
  bootStatus: vi.fn(async () => ({ ready: true, error: null })),
  listAgents: vi.fn(async () => []),
  listTopology: vi.fn(async () => []),
  listTopologyPositions: vi.fn(async () => ({})),
  listModelTemplates: vi.fn(async () => []),
  listRoles: vi.fn(async () => []),
  listGroups: vi.fn(async () => []),
  listJudges: vi.fn(async () => []),
  listRagSources: vi.fn(async () => []),
  listMessages: vi.fn(async () => [] as AgentMessage[]),
  listPlanWaves: vi.fn(async () => []),
  workspacePath: vi.fn(async () => "C:\\workspace"),
  currentSession: vi.fn(async () => "session_1"),
  getLanguage: vi.fn(async () => "ja"),
  listCommandRequests: vi.fn(async () => []),
  getUserName: vi.fn(async () => null),
  getUserIcon: vi.fn(async () => null),
  listSessions: vi.fn(async () => [] as SessionSummary[]),
  resumeSession: vi.fn(async () => null),
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

/** マクロタスクを 1 周させる（一覧の取り直しと知らせは非同期）。 */
async function settle(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

/** 開いている会話を session_1 に戻し、知らせを全部閉じた状態から始める。 */
async function fresh() {
  const orchestrator = useOrchestrator();
  await orchestrator.init();
  fire({ type: "sessionSwitched", sessionId: "session_1" });
  await settle();
  for (const toast of [...orchestrator.state.toasts]) orchestrator.dismissToast(toast.id);
  h.listSessions.mockResolvedValue(SESSIONS);
  return orchestrator;
}

describe("会話の単位で絞る投影（Spec 75 P2）", () => {
  it("入力中の印は個体の単位のまま、根の会話を別に持つ", async () => {
    const orchestrator = await fresh();

    fire({ type: "agentTyping", agentId: "agent_zari", active: true, sessionId: "session_9" });
    expect(orchestrator.state.typing.agent_zari, "全ターン停止と黒板が読むので個体の単位").toBe(true);
    expect(orchestrator.state.typingSession.agent_zari).toBe("session_9");

    fire({ type: "agentTyping", agentId: "agent_zari", active: false, sessionId: "session_9" });
    expect(orchestrator.state.typing.agent_zari).toBeUndefined();
    expect(orchestrator.state.typingSession.agent_zari).toBeUndefined();
  });

  it("ツール行は開いている会話の分だけ積み、カードの直近と周回数は個体の単位のまま", async () => {
    const orchestrator = await fresh();
    const before = orchestrator.state.toolRuns.length;

    fire({
      type: "toolInvoked", agentId: "agent_zari", tool: "grep", ok: true,
      reason: { kind: "omitted" }, callId: 501, round: 3, sessionId: "session_9",
    });
    expect(orchestrator.state.toolRuns.length, "別の会話の行は積まない").toBe(before);
    expect(orchestrator.state.lastTool.agent_zari?.callId).toBe(501);
    expect(orchestrator.state.toolRound.agent_zari).toBe(3);

    fire({
      type: "toolInvoked", agentId: "agent_zari", tool: "grep", ok: true,
      reason: { kind: "omitted" }, callId: 502, round: 4, sessionId: "session_1",
    });
    expect(orchestrator.state.toolRuns.length).toBe(before + 1);
    const runs = orchestrator.state.toolRuns;
    expect(runs[runs.length - 1]?.callId).toBe(502);
  });

  it("開いていない会話にサーヴァントの答えが届いたら、会話ごとに 1 枚の知らせ（開くつき）", async () => {
    const orchestrator = await fresh();
    h.listSessions.mockClear();

    fire({ type: "sessionUpdated", sessionId: "session_9", from: { kind: "agent", id: "agent_zari" } });
    await settle();
    expect(h.listSessions, "会話一覧（並びと更新時刻）を引き直す").toHaveBeenCalled();
    const notices = orchestrator.state.toasts.filter((t) => t.openSessionId === "session_9");
    expect(notices).toHaveLength(1);
    expect(notices[0].title).toContain("予定の会話");
    expect(notices[0].title).toContain("agent_zari");

    fire({ type: "sessionUpdated", sessionId: "session_9", from: { kind: "agent", id: "agent_jemmy" } });
    await settle();
    const merged = orchestrator.state.toasts.filter((t) => t.openSessionId === "session_9");
    expect(merged, "同じ会話への知らせは 1 枚にまとめる").toHaveLength(1);
    expect(merged[0].title).toContain("2");
    expect(merged[0].title, "最後の送り手を出す").toContain("agent_jemmy");
  });

  it("利用者の発話・System 行では知らせを出さない（一覧は引き直す）", async () => {
    const orchestrator = await fresh();
    h.listSessions.mockClear();

    fire({ type: "sessionUpdated", sessionId: "session_9", from: { kind: "user" } });
    fire({ type: "sessionUpdated", sessionId: "session_9", from: { kind: "system" } });
    await settle();
    expect(h.listSessions).toHaveBeenCalledTimes(2);
    expect(orchestrator.state.toasts.filter((t) => t.openSessionId)).toHaveLength(0);
  });

  it("届いた時点でその会話が開いていれば知らせない", async () => {
    const orchestrator = await fresh();

    fire({ type: "sessionUpdated", sessionId: "session_1", from: { kind: "agent", id: "agent_zari" } });
    await settle();
    expect(orchestrator.state.toasts.filter((t) => t.openSessionId)).toHaveLength(0);
  });

  it("その会話が開かれたら知らせを閉じ、承認画面の投影を取り直す", async () => {
    const orchestrator = await fresh();
    fire({ type: "sessionUpdated", sessionId: "session_9", from: { kind: "agent", id: "agent_zari" } });
    await settle();
    expect(orchestrator.state.toasts.filter((t) => t.openSessionId === "session_9")).toHaveLength(1);
    h.listCommandRequests.mockClear();

    fire({ type: "sessionSwitched", sessionId: "session_9" });
    await settle();
    expect(orchestrator.state.toasts.filter((t) => t.openSessionId === "session_9")).toHaveLength(0);
    expect(h.listCommandRequests, "印は（個体, 会話）で持つので開いている会話が変われば取り直す")
      .toHaveBeenCalled();
  });

  it("知らせの「開く」はその会話を開いて知らせを閉じる", async () => {
    const orchestrator = await fresh();
    fire({ type: "sessionUpdated", sessionId: "session_9", from: { kind: "agent", id: "agent_zari" } });
    await settle();
    const [notice] = orchestrator.state.toasts.filter((t) => t.openSessionId === "session_9");
    h.resumeSession.mockClear();

    await orchestrator.openNoticedSession(notice);
    expect(h.resumeSession).toHaveBeenCalledWith("session_9");
    expect(orchestrator.state.toasts.some((t) => t.id === notice.id)).toBe(false);
  });

  it("波は撒いた・提示した会話を持つ（保存先の無い村の \"-\" は null）", async () => {
    const orchestrator = await fresh();

    fire({
      type: "planWaveProposed", planId: 900, agentId: "agent_luna", wave: 1,
      tasks: [{ to: "agent_zari", message: "調べて" }], startedAtMs: 1, sessionId: "session_9",
    });
    fire({
      type: "planWaveStarted", planId: 901, agentId: "agent_luna", wave: 1,
      tasks: [{ to: "agent_zari", msgChars: 3 }], startedAtMs: 2, sessionId: "-",
    });
    const proposed = orchestrator.state.planWaves.find((w) => w.planId === 900);
    const started = orchestrator.state.planWaves.find((w) => w.planId === 901);
    expect(proposed?.sessionId).toBe("session_9");
    expect(started?.sessionId).toBeNull();

    // 承認（planWaveStarted）で会話が消えない。
    fire({
      type: "planWaveStarted", planId: 900, agentId: "agent_luna", wave: 1,
      tasks: [{ to: "agent_zari", msgChars: 3 }], startedAtMs: 3, sessionId: "session_9",
    });
    expect(orchestrator.state.planWaves.find((w) => w.planId === 900)?.sessionId).toBe("session_9");
  });
});
