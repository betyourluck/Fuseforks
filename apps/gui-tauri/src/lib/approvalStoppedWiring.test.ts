// @ts-expect-error @types/node を入れない方針のため（editorSaveWiring.test.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import en from "../locales/en.json";
import ja from "../locales/ja.json";

/**
 * 承認待ちの印（Spec 72 D5）の受け口と、`approval_pending` の表示の配線。
 *
 * - `useOrchestrator` の `switch` は `default` も網羅検査も無いので、`approvalStopped` の
 *   `case` を忘れても黙る（印は `run.json` に無く、`toolInvoked` の経路では動かない）
 * - `PlanWavePane` のセル色は `default` が失敗色なので、腕を書き忘れると承認待ちが失敗色で出る
 *   （型は `STATE_LABEL_KEYS` の網羅しか要求しない）
 */
const orchestrator = readFileSync(
  new URL("../composables/useOrchestrator.ts", import.meta.url),
  "utf8",
);
const waves = readFileSync(new URL("../components/PlanWavePane.vue", import.meta.url), "utf8");

function lookup(dict: Record<string, unknown>, key: string): unknown {
  return key
    .split(".")
    .reduce<unknown>((node, part) => (node as Record<string, unknown> | undefined)?.[part], dict);
}

describe("承認待ちの印の配線", () => {
  it("approvalStopped を受けたら承認画面の投影を取り直す", () => {
    const at = orchestrator.indexOf('case "approvalStopped":');
    expect(at).toBeGreaterThan(-1);
    const body = orchestrator.slice(at, orchestrator.indexOf("break;", at));
    expect(body).toContain("refreshCommandRequests()");
  });

  it("作業状況のセルは approval_pending を注意色の腕に明示する（default は失敗色）", () => {
    expect(waves).toContain('approval_pending: "waves.state.approval_pending"');
    const cell = waves.slice(waves.indexOf("function cellClass("), waves.indexOf("function formatMs("));
    const arm = cell.indexOf('case "approval_pending":');
    expect(arm).toBeGreaterThan(-1);
    const next = cell.indexOf("return", arm);
    expect(cell.slice(next, next + 40)).toContain("bg-warn");
  });

  it("実行時に組む鍵は ja / en の両方にある", () => {
    for (const key of ["waves.state.approval_pending", "stats.stop.approvalPending"]) {
      expect(lookup(ja as Record<string, unknown>, key), `ja: ${key}`).toBeTypeOf("string");
      expect(lookup(en as Record<string, unknown>, key), `en: ${key}`).toBeTypeOf("string");
    }
  });
});
