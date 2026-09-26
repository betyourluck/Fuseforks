/**
 * ステータスバーの 2 つのスイッチを端末に覚えて、起動時に戻す（2026-09-27 利用者裁定）。
 *
 * コアは起動のたびに既定（確認あり / 承認が必要）で始まる。画面が `localStorage` から
 * 覚えた値を読み、違えば設定し直す。**書くのはコアが受け入れた値だけ** —
 * 起動時にコアから読んだ既定で、覚えた値を上書きしない。
 */
import { describe, expect, it, vi } from "vitest";

import { SWITCH_STORAGE_KEY } from "../lib/switchMemory";

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
  getPlanReviewBypass: vi.fn(async () => false),
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


function fakeStorage(initial: Record<string, string>) {
  const map = new Map(Object.entries(initial));
  return {
    getItem: (k: string) => map.get(k) ?? null,
    setItem: (k: string, v: string) => void map.set(k, v),
    dump: (k: string) => map.get(k) ?? null,
  };
}

describe("スイッチの記憶", () => {
  it("覚えた値がコアの既定と違えば、起動時にコアへ設定し直す。イベントも覚える", async () => {
    const storage = fakeStorage({
      [SWITCH_STORAGE_KEY]: JSON.stringify({ planReviewBypass: true, runApproval: "no_approval" }),
    });
    vi.stubGlobal("localStorage", storage);

    const orchestrator = useOrchestrator();
    await orchestrator.init();
    expect(h.setPlanReviewBypass).toHaveBeenCalledWith(true);
    expect(h.setRunApproval).toHaveBeenCalledWith("no_approval");
    expect(orchestrator.state.planReviewBypass).toBe(true);
    expect(orchestrator.state.runApproval).toBe("no_approval");

    fire({ type: "runApprovalChanged", mode: "auto_approve" });
    expect(orchestrator.state.runApproval).toBe("auto_approve");
    expect(JSON.parse(storage.dump(SWITCH_STORAGE_KEY) ?? "{}")).toEqual({
      planReviewBypass: true,
      runApproval: "auto_approve",
    });

    expect(await orchestrator.setRunApproval("required")).toBe(true);
    expect(JSON.parse(storage.dump(SWITCH_STORAGE_KEY) ?? "{}").runApproval).toBe("required");
    vi.unstubAllGlobals();
  });
});
