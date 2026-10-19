// @ts-expect-error @types/node を入れない方針のため（editorSaveWiring.test.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { approvalBanner } from "./approvalBanner";
import type { CommandPolicyView } from "../types";

/**
 * 会話ペインの承認待ちの帯の配線（2026-10-19）。
 *
 * 帯の判定は `approvalBanner.ts` の純関数が持ち、ここが確かめるのは**画面がそれを読んでいること**と、
 * 押すとタイトルバーと同じ承認画面が開くことだけ（作業状況タブの印で、判定を部品に直書きしたまま
 * 走査が字面しか見ておらず、判定を倒しても緑だった形の再発防止）。
 */
const panel = readFileSync(new URL("../components/ChatPanel.vue", import.meta.url), "utf8");
const titleBar = readFileSync(new URL("../components/TitleBar.vue", import.meta.url), "utf8");
const app = readFileSync(new URL("../App.vue", import.meta.url), "utf8");
const ja = JSON.parse(readFileSync(new URL("../locales/ja.json", import.meta.url), "utf8"));
const en = JSON.parse(readFileSync(new URL("../locales/en.json", import.meta.url), "utf8"));

function lookup(dict: Record<string, unknown>, key: string): unknown {
  return key.split(".").reduce<unknown>(
    (node, part) => (node as Record<string, unknown> | undefined)?.[part],
    dict,
  );
}

describe("承認待ちの帯の配線", () => {
  it("帯は純関数を読み、承認待ちがある間だけ出す", () => {
    expect(panel).toContain("approvalBanner(state.commandRequests)");
    expect(panel).toContain('v-if="approval"');
    // 件数を部品で数え直さない（バッジと帯で 2 実装になる）。
    expect(panel).not.toMatch(/commandRequests\.reduce/);
  });

  it("帯は入力欄の上に置き、会話の流れ（スクロールする側）には入れない", () => {
    const banner = panel.indexOf('v-if="approval"');
    const input = panel.indexOf("<ChatInput");
    const flow = panel.search(/class="chat-zoom[^"]*overflow-y-auto/);
    expect(banner).toBeGreaterThan(-1);
    expect(flow).toBeGreaterThan(-1);
    expect(banner).toBeLessThan(input);
    // 流れの閉じ（入力欄の直前の空の会話の文言）より後ろにあること。
    expect(banner).toBeGreaterThan(panel.indexOf('$t("chat.empty")'));
  });

  it("押すとタイトルバーと同じ承認画面を開く", () => {
    expect(panel).toContain("emit('open-command-approval')");
    expect(app).toMatch(/<ChatPanel\s+@open-command-approval="commandApprovalOpen = true"/);
    expect(app).toContain('@open-command-approval="commandApprovalOpen = true"');
  });

  it("タイトルバーのバッジも同じ関数で数える", () => {
    expect(titleBar).toContain("pendingTotal(state.commandRequests)");
    expect(titleBar).not.toMatch(/commandRequests\.reduce/);
  });

  it("帯の辞書の鍵は ja / en の両方にある", () => {
    // 鍵を実行時に選ぶので、欠けても vue-tsc も i18n のコンパイル検査も通る。
    const one: CommandPolicyView[] = [{ agentId: "a", name: "A", pending: [{ command: "git", args: [], firstRequestedAtMs: 0, count: 1 }], broken: false }];
    const many: CommandPolicyView[] = [...one, { ...one[0]!, agentId: "b", name: "B" }];
    for (const views of [one, many]) {
      const key = approvalBanner(views)!.key;
      expect(lookup(ja, key), `ja: ${key}`).toBeTypeOf("string");
      expect(lookup(en, key), `en: ${key}`).toBeTypeOf("string");
    }
  });
});
