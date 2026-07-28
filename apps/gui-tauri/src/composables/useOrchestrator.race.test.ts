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
  listModelTemplates: vi.fn(),
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
    lastError: null,
  };
}

describe("refreshAll の並行競合", () => {
  it("古い応答が後から着地しても、新しい状態を上書きしない", async () => {
    const orchestrator = useOrchestrator();

    h.listTopology.mockResolvedValue([]);
    h.listModelTemplates.mockResolvedValue([]);
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

    // 新しい応答が先に着地する。
    freshResponse.resolve([]);
    await refresh2;

    // 古い応答が**後から**着地する（IPC の完了順は保証されない）。
    staleResponse.resolve([snapshot("ghost")]);
    await refresh1;

    // 幽霊が復活してはいけない。最後に開始した取り直しだけが状態を書ける。
    expect(orchestrator.state.agents).toHaveLength(0);
  });
});
