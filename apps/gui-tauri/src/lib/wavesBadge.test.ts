/**
 * 作業状況タブの承認待ちの印の**配線**を機械で留める（2026-09-17）。
 *
 * 件数と判定は `wavesBadge.ts` の純関数 2 本で、タブのバッジ・色・ホバーの文言と
 * 編集パネルの見出しが同じ関数を読む。配線は型検査に掛からず、消えると「承認待ちが
 * あるのに黒板タブを見ている人に何も出ない」という沈黙として出る。
 * 文言と色の値そのものは留めない（`theme.test.ts` と同じ規律）。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

import { pendingWaveCount, tabAttention } from "./wavesBadge";

function read(rel: string): string {
  return readFileSync(fileURLToPath(new URL(rel, import.meta.url)), "utf8");
}

describe("wavesBadge: 判定", () => {
  it("数えるのは pending の波だけ（dispatched / discarded は 0）", () => {
    expect(pendingWaveCount([])).toBe(0);
    expect(
      pendingWaveCount([
        { state: "pending" },
        { state: "dispatched" },
        { state: "discarded" },
        { state: "pending" },
      ]),
    ).toBe(2);
  });

  it("印を付けるのは waves のタブだけ。0 件では付けない", () => {
    expect(tabAttention("waves", 1)).toBe(true);
    expect(tabAttention("waves", 3)).toBe(true);
    expect(tabAttention("waves", 0)).toBe(false);
    expect(tabAttention("blackboard", 2)).toBe(false);
  });
});

describe("wavesBadge: 配線", () => {
  const tabs = read("../components/BottomPaneTabs.vue");
  const pane = read("../components/PlanWavePane.vue");

  it("タブと編集パネルは同じ 1 実装から件数と判定を引く", () => {
    expect(tabs).toContain('from "../lib/wavesBadge"');
    expect(pane).toContain('from "../lib/wavesBadge"');
    // 印の判定も純関数から引く（部品に直書きすると単体の網の外へ出る）。
    expect(tabs).toMatch(/return tabAttention\(tab\.id, pendingCount\.value\);/);
    // 編集パネル側に filter の直書きが残っていない（2 箇所で数えない）。
    expect(pane).not.toMatch(/planWaves\.filter\(\(w\) => w\.state === "pending"\)\.length/);
  });

  it("バッジは判定を通して付き、件数を丸めずに出す", () => {
    // `v-if` の値に `>` が入りうるので `[^>]*` では届かない — 印の位置から閉じタグまでを切る。
    const at = tabs.indexOf("data-waves-pending");
    expect(at, "data-waves-pending の span").toBeGreaterThan(-1);
    const badge = tabs.slice(at, tabs.indexOf("</span>", at));
    expect(badge).toContain("{{ pendingCount }}");
    expect(tabs).toMatch(/v-if="attention\(tab\)"\s+data-waves-pending/);
  });

  it("承認待ちがあるあいだはラベルが accent になる（黒板タブを見ていても目に入る）", () => {
    expect(tabs).toMatch(/attention\(tab\)\s*\?\s*'text-accent'/);
  });

  it("辞書の鍵が ja / en に揃っている", () => {
    for (const loc of ["ja", "en"]) {
      const dict = JSON.parse(read(`../locales/${loc}.json`)) as {
        bottomTabs: Record<string, string>;
      };
      expect(dict.bottomTabs.wavesPending, loc).toContain("{count}");
    }
    expect(tabs).toContain("bottomTabs.wavesPending");
  });
});
