/**
 * 利用者の呼び名とアイコンの投影（Spec 19）。
 *
 * 会話ペインの自分の行は `state.userName` / `state.userIcon` から描くので、
 * **投影が古いまま残る**と「保存したのに画面が変わらない」、**拒否されたのに
 * 更新される**と「保存できていないのに変わって見える」になる。どちらも
 * コアのテストでは出ない（コアは正しく拒否している）。
 */
import { beforeEach, describe, expect, it, vi } from "vitest";

const h = vi.hoisted(() => ({
  listAgents: vi.fn(async () => []),
  listTopology: vi.fn(async () => []),
  listTopologyPositions: vi.fn(async () => ({})),
  listModelTemplates: vi.fn(async () => []),
  listRoles: vi.fn(async () => []),
  // Spec 51。refreshAll が list_groups も引くので、無いと起動の網に掛かる（意図した網）。
  listGroups: vi.fn(async () => []),
  listRagSources: vi.fn(async () => []),
  getAgentIcon: vi.fn(async () => null),
  // Tauri は Rust の () を null として返す。成功しても null（#22）。
  setUserName: vi.fn(async () => null),
  setUserIcon: vi.fn(async () => null),
  clearUserIcon: vi.fn(async () => null),
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

/** WebP の最小コンテナ（RIFF....WEBP）。 */
const WEBP = new Uint8Array([
  0x52, 0x49, 0x46, 0x46, 4, 0, 0, 0, 0x57, 0x45, 0x42, 0x50,
]);

describe("利用者の呼び名とアイコンの投影", () => {
  beforeEach(async () => {
    // 各テストを既定（未設定）から始める。state はモジュール共有なので
    // 前のテストの結果が漏れると、通っている理由が読めなくなる。
    const orchestrator = useOrchestrator();
    await orchestrator.setUserName(null);
    await orchestrator.clearUserIcon();
    vi.clearAllMocks();
  });

  it("呼び名を保存すると投影も変わる", async () => {
    const orchestrator = useOrchestrator();
    await expect(orchestrator.setUserName("たかはし")).resolves.toBe(true);
    expect(orchestrator.state.userName).toBe("たかはし");
    expect(h.setUserName).toHaveBeenCalledWith("たかはし");
  });

  it("拒否されたら投影は変わらない", async () => {
    const orchestrator = useOrchestrator();
    await orchestrator.setUserName("たかはし");

    // コアが書式で拒否した場合（`INVALID_USER_NAME`）。
    h.setUserName.mockRejectedValueOnce(new Error("呼び名を受け付けられません"));
    await expect(orchestrator.setUserName("だめ】")).resolves.toBe(false);
    expect(orchestrator.state.userName).toBe("たかはし");
  });

  it("既定へ戻すと未設定になる（画面は chat.you へ落ちる）", async () => {
    const orchestrator = useOrchestrator();
    await orchestrator.setUserName("たかはし");
    await expect(orchestrator.setUserName(null)).resolves.toBe(true);
    expect(orchestrator.state.userName).toBeNull();
  });

  it("アイコンの保存と削除で投影が入れ替わる", async () => {
    const orchestrator = useOrchestrator();
    expect(orchestrator.state.userIcon).toBeNull();

    await expect(orchestrator.setUserIcon(WEBP)).resolves.toBe(true);
    expect(orchestrator.state.userIcon).toBeTruthy();

    await expect(orchestrator.clearUserIcon()).resolves.toBe(true);
    expect(orchestrator.state.userIcon).toBeNull();
  });

  it("アイコンの保存が失敗したら投影は変わらない", async () => {
    const orchestrator = useOrchestrator();
    h.setUserIcon.mockRejectedValueOnce(new Error("WebP 形式ではありません"));
    await expect(orchestrator.setUserIcon(WEBP)).resolves.toBe(false);
    expect(orchestrator.state.userIcon).toBeNull();
  });
});
