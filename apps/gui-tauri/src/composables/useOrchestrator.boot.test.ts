/**
 * 起動ハンドシェイク（boot_status 待ち）のテスト。
 *
 * バックエンドの初期化はバックグラウンドで走り、完了までは他のコマンドを
 * 呼んではいけない（状態未登録で失敗する）。init() が `ready` を確認するまで
 * 一切のデータ取得を始めないこと、失敗が BOOT_FAILED として表面化することを固定する。
 */
import { describe, expect, it, vi } from "vitest";

const h = vi.hoisted(() => ({
  bootStatus: vi.fn(),
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
}));

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));

vi.mock("../lib/ipc", () => ({
  ...h,
  toErrorPayload: (error: unknown) => error,
}));

import { useOrchestrator } from "./useOrchestrator";

describe("起動ハンドシェイク", () => {
  it("バックエンドの初期化失敗は BOOT_FAILED として表面化し、データ取得を始めない", async () => {
    h.bootStatus.mockResolvedValueOnce({
      ready: false,
      error: "MCP の設定が壊れています",
    });

    const orchestrator = useOrchestrator();
    await orchestrator.init();

    expect(orchestrator.state.ready).toBe(false);
    expect(orchestrator.state.initError?.code).toBe("BOOT_FAILED");
    expect(orchestrator.state.initError?.detail).toContain("MCP");
    expect(h.listAgents).not.toHaveBeenCalled();
  });

  it("ready が返るまで待ってから読み込みを始める", async () => {
    // 1 回目は準備中 → 2 回目で完了。ポーリングを 1 周は回す。
    h.bootStatus
      .mockResolvedValueOnce({ ready: false, error: null })
      .mockResolvedValue({ ready: true, error: null });

    const orchestrator = useOrchestrator();
    await orchestrator.init();

    expect(h.bootStatus.mock.calls.length).toBeGreaterThanOrEqual(2);
    expect(orchestrator.state.ready).toBe(true);
    expect(orchestrator.state.initError).toBeNull();
    expect(h.listAgents).toHaveBeenCalled();
  }, 10_000);
});
