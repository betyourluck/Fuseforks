/**
 * コードブロック単位のコピーの**配線**を機械で留める（2026-10-15）。
 *
 * ボタンは `renderMarkdown` が HTML に埋め（`markdown.test.ts` が形を留める）、押した動作は
 * `ChatPanel` が `data-code-copy` で受ける。属性名は手で揃えているので、片方だけ改名すると
 * **ボタンは見えるのに押しても何も起きない**（型検査にも lint にも掛からない）。
 * CSS の選択子（`.code-block` / `.code-copy` / `[data-copied]`）も同じ理由で留める。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

import { renderMarkdown } from "./markdown";

function read(rel: string): string {
  return readFileSync(fileURLToPath(new URL(rel, import.meta.url)), "utf8");
}

const panel = read("../components/ChatPanel.vue");
const css = read("../style.css").replace(/\/\*[\s\S]*?\*\//g, "");
const ja = JSON.parse(read("../locales/ja.json")) as { chat: Record<string, unknown> };
const en = JSON.parse(read("../locales/en.json")) as { chat: Record<string, unknown> };

describe("コードブロックのコピーの配線", () => {
  it("ChatPanel はラベルを渡して描き、同じ属性名で押下を受ける", () => {
    expect(panel).toContain('codeCopyLabel: t("chat.codeCopy")');
    expect(panel).toContain('closest<HTMLElement>("button[data-code-copy]")');
    const html = renderMarkdown("```\nx\n```", { codeCopyLabel: "L" });
    expect(html).toContain("data-code-copy");
    expect(html).toContain('class="code-block"');
  });

  it("写すのは pre > code の中身で、末尾の改行 1 つを落とす", () => {
    expect(panel).toContain('querySelector("pre code")?.textContent');
    expect(panel).toContain('.replace(/\\n$/, "")');
  });

  it("押した合図の属性と CSS の選択子が揃っている", () => {
    expect(panel).toContain('button.dataset.copied = ""');
    expect(css).toMatch(/\.md-body \.code-copy\[data-copied\]/);
    expect(css).toMatch(/\.md-body \.code-block\s*\{/);
    expect(css).toMatch(/\.md-body \.code-block:hover \.code-copy/);
  });

  it("辞書の鍵が ja と en の両方に在り、合図は発話のコピーと同じ鍵を使う", () => {
    expect(typeof ja.chat.codeCopy).toBe("string");
    expect(typeof en.chat.codeCopy).toBe("string");
    expect(panel).toContain('t("chat.copied")');
  });
});
