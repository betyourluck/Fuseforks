/**
 * refreshAll の並行競合（後勝ち上書き）の再現テスト。
 *
 * コアは全 CRUD で `TopologyChanged` を emit し、フロントは
 * 「イベント由来の void refreshAll()」と「mutate 完了時の refreshAll()」の
 * 2 本を並行で走らせる。取り直しの**開始順**と**着地順**は一致しないので、
 * 古い状態を持った応答が後から着地すると、新しい状態を黙って上書きする。
 * 実機では「削除したのにリストに残る」「保存したのに表示が戻る」として現れ、
 * 再起動するまで直らない。
 */
import { describe, expect, it, vi } from "vitest";

const h = vi.hoisted(() => ({
  listAgents: vi.fn(),
  listTopology: vi.fn(),
  listTopologyPositions: vi.fn(),
  listModelTemplates: vi.fn(),
  listRoles: vi.fn(),
  listRagSources: vi.fn(),
  getAgentIcon: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}));

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

/** 手動で resolve できる Promise。応答の着地順を試験側が制御する。 */
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
}

/** テストに必要な最小のスナップショット。 */
function snapshot(id: string) {
  return {
    id,
    name: id,
    model: "mock",
    modelTemplateId: "tpl",
    status: "idle",
    uptimeSecs: 0,
    totalTokens: 0,
    ragSources: [],
    connectedAgents: [],
    order: 0,
    workDir: null,
    maxToolIterations: null,
    enabledTools: null,
    hearsRoomLog: true,
    lastError: null,
  };
}

describe("refreshAll の並行競合", () => {
  it("古い応答が後から着地しても、新しい状態を上書きしない", async () => {
    const orchestrator = useOrchestrator();

    h.listTopology.mockResolvedValue([]);
    h.listTopologyPositions.mockResolvedValue({});
    h.listModelTemplates.mockResolvedValue([]);
    h.listRoles.mockResolvedValue([]);
    h.listRagSources.mockResolvedValue([]);
    h.getAgentIcon.mockResolvedValue(null);

    // 取り直し #1 は削除**前**の一覧（ghost が居る）を、
    // 取り直し #2 は削除**後**の一覧（空）を返す。
    const staleResponse = deferred<unknown[]>();
    const freshResponse = deferred<unknown[]>();
    h.listAgents
      .mockReturnValueOnce(staleResponse.promise)
      .mockReturnValueOnce(freshResponse.promise);

    const refresh1 = orchestrator.refreshAll();
    const refresh2 = orchestrator.refreshAll();

    // 着地順を敵対的にする: 新しい応答が先、古い応答が後
    // （IPC の完了順は開始順と一致しない）。
    freshResponse.resolve([]);
    staleResponse.resolve([snapshot("ghost")]);
    await Promise.all([refresh1, refresh2]);

    // 幽霊が復活してはいけない。単一飛行 + 追走により、
    // 後から呼んだ取り直しの結果が必ず最後に書かれる。
    expect(orchestrator.state.agents).toHaveLength(0);
  });

  it("await から戻った時点で、追走ぶんの取得が完了している", async () => {
    const orchestrator = useOrchestrator();

    h.listTopology.mockResolvedValue([]);
    h.listTopologyPositions.mockResolvedValue({});
    h.listModelTemplates.mockResolvedValue([]);
    h.listRoles.mockResolvedValue([]);
    h.listRagSources.mockResolvedValue([]);
    h.getAgentIcon.mockResolvedValue(null);

    // 取り直し #1 が飛行中に #2 を呼ぶ。#2 は「#1 完了後の再取得」まで待つこと。
    // 進行中の #1 は #2 の呼び出し元の変更より古いデータを持ちうるので、
    // #1 への相乗りだけでは「保存したのに古い状態を掴む」が再発する。
    const first = deferred<unknown[]>();
    h.listAgents
      .mockReturnValueOnce(first.promise)
      .mockResolvedValueOnce([snapshot("fresh_agent")]);

    const refresh1 = orchestrator.refreshAll();
    const refresh2 = orchestrator.refreshAll();

    first.resolve([]);
    await refresh2;

    // #2 の await 明けには、2 回目の取得結果（呼び出し時点より新しい）が見えている。
    expect(orchestrator.state.agents).toHaveLength(1);
    expect(orchestrator.state.selectedAgentId).toBe("fresh_agent");
    await refresh1;
  });
});
