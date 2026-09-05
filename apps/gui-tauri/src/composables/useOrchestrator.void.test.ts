/**
 * 戻り値が void のコマンドで、成功が失敗と誤判定されないことの検証。
 *
 * Rust の `CoreResult<()>` は JSON で **null** にシリアライズされる。
 * 「失敗なら null を返す」という mutate の契約と衝突し、
 * `write_ordinance` / `write_agent_config` / `set_agent_icon` などの
 * 成功が全部「失敗」として扱われていた。実機では
 * 「保存を押しても未保存のまま（実データは保存済み）」
 * 「アイコンを変えても再起動まで反映されない」として現れた（failures.md #22）。
 */
import { describe, expect, it, vi } from "vitest";

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
  // Tauri は Rust の () を null として返す。成功しても null。
  writeOrdinance: vi.fn(async () => null),
  writeAgentConfig: vi.fn(async () => null),
  setAgentIcon: vi.fn(async () => null),
  clearAgentIcon: vi.fn(async () => null),
  sendUserMessage: vi.fn(async () => null),
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

describe("void を返すコマンドの成功判定", () => {
  it("条例の保存は成功として扱われる", async () => {
    const orchestrator = useOrchestrator();
    await expect(orchestrator.saveOrdinance("# 条例")).resolves.toBe(true);
  });

  it("設定ファイルの保存は成功として扱われる", async () => {
    const orchestrator = useOrchestrator();
    await expect(
      orchestrator.writeConfig("agent_a", "skill", "# Skill"),
    ).resolves.toBe(true);
  });

  it("アイコンの保存は成功として扱われ、キャッシュが更新される", async () => {
    const orchestrator = useOrchestrator();
    // WebP の最小コンテナ（RIFF....WEBP）。
    const bytes = new Uint8Array([
      0x52, 0x49, 0x46, 0x46, 4, 0, 0, 0, 0x57, 0x45, 0x42, 0x50,
    ]);
    await expect(orchestrator.setAgentIcon("agent_a", bytes)).resolves.toBe(true);
    // 成功したのだから、次の描画で新しい画像が出る状態になっていること。
    expect(orchestrator.state.icons["agent_a"]).toBeTruthy();
  });

  it("失敗（例外）は従来どおり失敗として扱われる", async () => {
    const orchestrator = useOrchestrator();
    h.writeOrdinance.mockRejectedValueOnce(new Error("書けません"));
    await expect(orchestrator.saveOrdinance("# 条例")).resolves.toBe(false);
  });
});
