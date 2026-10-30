// @ts-expect-error @types/node を入れない方針のため（editorSaveWiring.test.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

/**
 * Markdown のリンクの**配線**を機械で留める（2026-10-30 の監査 8）。
 *
 * - `markdown.ts` は markdown-it 既定の `validateLink`（`javascript:` 等を落とす除外リスト）ではなく、
 *   `lib/links.ts` の閉じた許容を使う
 * - 会話ペインと黒板ペインは **同じ述語** `isOpenableLink` でクリックを横取りし、外部ブラウザへ渡す。
 *   黒板はそれまで横取りが無く、`target="_blank"` の保険だけで「押しても無反応」だった
 *
 * 述語を 2 箇所に書くと片方だけが広がる（型検査にも lint にも掛からない）ので、字面で留める。
 */
const read = (rel: string): string => readFileSync(new URL(rel, import.meta.url), "utf8");
const markdown = read("./markdown.ts");
const chat = read("../components/ChatPanel.vue");
const blackboard = read("../components/BlackboardPane.vue");

describe("Markdown のリンクの配線", () => {
  it("markdown.ts は validateLink を閉じた許容へ差し替える", () => {
    expect(markdown).toContain('import { isRenderableLink } from "./links";');
    expect(markdown).toMatch(/md\.validateLink = \(url\) => isRenderableLink\(url\)/);
  });

  it("会話ペインは isOpenableLink で判定して openUrl へ渡す", () => {
    expect(chat).toContain('import { isOpenableLink } from "../lib/links";');
    expect(chat).toContain("if (isOpenableLink(href)) {");
    expect(chat).not.toMatch(/\/\^https\?:\\\/\\\/\/i\.test\(href\)/);
  });

  it("黒板ペインも同じ述語で横取りし、描いた本文の要素がハンドラを持つ", () => {
    expect(blackboard).toContain('import { openUrl } from "@tauri-apps/plugin-opener";');
    expect(blackboard).toContain('import { isOpenableLink } from "../lib/links";');
    expect(blackboard).toContain("if (isOpenableLink(href)) {");
    // v-html の要素そのものが @click を持つ（別の要素に付けると、畳んだときに外れる）。
    expect(blackboard).toMatch(/@click="onMarkdownClick"\s+v-html="renderMarkdown\(note\.content\)"/);
  });
});
