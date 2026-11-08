// @ts-expect-error @types/node を入れない方針のため（editorSaveWiring.test.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

/**
 * 最新へ飛ぶボタン（2026-11-08）の配線。
 *
 * 判定（末尾が見えているか）は `chatScroll.ts` の純関数が持ち、単体で留めている。ここで確かめるのは
 * **追従とボタンが同じ関数を読んでいること**と、置き場の規律（流れる側の中・座標を測らない）。
 */
const panel: string = readFileSync(new URL("../components/ChatPanel.vue", import.meta.url), "utf8");
const script = panel.slice(0, panel.indexOf("<template"));
const template = panel.slice(panel.indexOf("<template"), panel.indexOf("<style"));

describe("最新へ飛ぶボタン", () => {
  it("新しい行への追従とボタンの出し入れが同じ isNearBottom を読む", () => {
    expect(script).toContain('import { isNearBottom } from "../lib/chatScroll";');
    expect(script).toContain("const follow = isNearBottom(el);");
    expect(script).toContain("atBottom.value = isNearBottom(el);");
    // 判定の式を手で書き直していない（2 箇所に書くと閾値がずれる）。
    expect(script).not.toMatch(/scrollHeight\s*-\s*\w+\.scrollTop/);
  });

  it("流れる側のスクロールで取り直し、行の出入りの後にも取り直す", () => {
    const scroller = template.slice(template.indexOf('ref="scroller"'));
    expect(scroller.slice(0, scroller.indexOf(">"))).toContain('@scroll.passive="syncAtBottom"');
    const watcher = script.slice(script.indexOf("const follow = isNearBottom(el);"));
    expect(watcher.slice(0, watcher.indexOf("},"))).toContain("syncAtBottom();");
  });

  it("ボタンは末尾が見えていないときだけ、流れる側の中に出る", () => {
    const at = template.indexOf("data-jump-latest");
    expect(at).toBeGreaterThan(template.indexOf('ref="scroller"'));
    expect(at).toBeLessThan(template.indexOf("<ChatInput"));
    const line = template.slice(template.lastIndexOf("<div", at), at);
    expect(line).toContain('v-if="!atBottom"');
    expect(line).toContain("sticky bottom-0 h-0");
  });

  it("応答を作っている個体がいる間は三点、いなければ ↓", () => {
    const fab = template.slice(template.indexOf("data-jump-latest"), template.indexOf("<ChatInput"));
    expect(fab).toContain('<span v-if="typingAgents.length" class="chat-jump-dots"');
    expect(fab).toMatch(/<svg\s+v-else/);
    expect(fab).toContain("@click=\"jumpToLatest\"");
  });
});
