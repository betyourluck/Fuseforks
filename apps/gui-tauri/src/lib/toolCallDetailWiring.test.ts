/**
 * ツール行の中身（Spec 57）の配線を、ソースの走査で留める。
 *
 * ここで見ているのは**型検査にも実行時のテストにも掛からない結合**だけ —
 * コマンド名の綴り（Rust と TS で別々に書く文字列）/ 実行時に引く辞書の鍵 /
 * 会話ペインが座標計算を持たないという前提。
 */

import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

/** `apps/gui-tauri` からの相対パスで読む。 */
const read = (path: string): string =>
  readFileSync(fileURLToPath(new URL(`../../${path}`, import.meta.url)), "utf8");

const panel = read("src/components/ChatPanel.vue");

describe("ツール行の中身の配線（Spec 57）", () => {
  it("IPC のコマンド名が Rust の定義・登録と TS の呼び出しで揃っている", () => {
    // 綴りがずれても両側とも単独では通る。落ちるのは行を開いた瞬間だけ。
    expect(read("src-tauri/src/commands.rs")).toMatch(/pub async fn get_tool_call\(/);
    expect(read("src-tauri/src/lib.rs")).toContain("commands::get_tool_call,");
    expect(read("src/lib/ipc.ts")).toContain('"get_tool_call"');
  });

  it("会話ペインは束ねた後の並び（display）を描く", () => {
    // `timeline` のままだと束ねの見出しが 1 本も出ず、純関数のテストは緑のまま。
    expect(panel).toMatch(/v-for="\(entry, index\) in display"/);
    expect(panel).not.toMatch(/v-for="\(entry, index\) in timeline"/);
  });

  it("行と見出しが、開閉の口へ繋がっている", () => {
    expect(panel).toContain('@click="toolCalls.toggle(entry.run.callId)"');
    expect(panel).toContain('@click="toggleGroup(entry.key)"');
    expect(panel).toContain("useToolCallDetails(getToolCall)");
  });

  it("会話ペインは座標計算を持たない（zoom を掛ける場所。高さは CSS だけで決める）", () => {
    expect(panel).not.toContain("getBoundingClientRect");
    expect(panel).not.toContain("elementFromPoint");
  });

  it("会話ペインが引く辞書の鍵が ja / en の両方に在る", () => {
    const used = new Set(
      [...panel.matchAll(/\$?t\(\s*"(chat\.tool(?:Group|Detail\.[a-z]+))"/gi)].map((m) => m[1]),
    );
    // 走査そのものの検定 — 1 つも拾えていなければ、下の検査は何も見ていない。
    expect([...used].sort()).toEqual([
      "chat.toolDetail.chars",
      "chat.toolDetail.empty",
      "chat.toolDetail.failed",
      "chat.toolDetail.gone",
      "chat.toolDetail.input",
      "chat.toolDetail.loading",
      "chat.toolDetail.output",
      "chat.toolDetail.truncated",
      "chat.toolGroup",
    ]);
    for (const lang of ["ja", "en"]) {
      const dict = JSON.parse(read(`src/locales/${lang}.json`)) as Record<string, unknown>;
      for (const key of used) {
        const value = key
          .split(".")
          .reduce<unknown>(
            (node: unknown, part: string) => (node as Record<string, unknown> | undefined)?.[part],
            dict,
          );
        expect(typeof value, `${lang}: ${key}`).toBe("string");
      }
    }
  });

  it("見出しの文面はエラーの本数を持たない", () => {
    for (const lang of ["ja", "en"]) {
      const dict = JSON.parse(read(`src/locales/${lang}.json`)) as { chat: { toolGroup: string } };
      const params = [...dict.chat.toolGroup.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();
      expect(params, lang).toEqual(["count", "name"]);
    }
  });
});
