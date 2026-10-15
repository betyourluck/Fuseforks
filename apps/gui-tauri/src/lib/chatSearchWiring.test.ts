/**
 * 会話ログの検索の**配線**を機械で留める（2026-10-15）。
 *
 * 規則は `chatSearch.ts` の純関数で単体が守る。ここで留めるのは純関数の外 —
 * 鍵の受け口が `isChatSearchKey` と覆いの判定を通ること / 行に `data-chat-row` と
 * `data-chat-text` が付いていること（無いと一致はあるのに何も光らず、どこへも
 * スクロールしない = 静かに壊れる）/ ハイライトの名前が ChatPanel と style.css で
 * 揃っていること / 辞書の鍵が ja・en の両方に在ること。
 * **座標計算は持ち込まない**（会話ペインは `zoom` の場所 — `toolCallDetailWiring` と同じ網）。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

function read(rel: string): string {
  return readFileSync(fileURLToPath(new URL(rel, import.meta.url)), "utf8");
}

const panel = read("../components/ChatPanel.vue");
const css = read("../style.css").replace(/\/\*[\s\S]*?\*\//g, "");
const ja = JSON.parse(read("../locales/ja.json")) as Record<string, unknown>;
const en = JSON.parse(read("../locales/en.json")) as Record<string, unknown>;

function dig(dict: Record<string, unknown>, key: string): unknown {
  let node: unknown = dict;
  for (const part of key.split(".")) {
    if (typeof node !== "object" || node === null) return undefined;
    node = (node as Record<string, unknown>)[part];
  }
  return node;
}

describe("会話ログの検索の配線", () => {
  it("鍵の受け口は純関数と覆いの判定を通り、窓に登録される", () => {
    expect(panel).toContain("isChatSearchKey(event)");
    expect(panel).toContain("hasOpenOverlay(document)");
    expect(panel).toContain('window.addEventListener("keydown", onSearchHotkey)');
    expect(panel).toContain('window.removeEventListener("keydown", onSearchHotkey)');
    // エディタの Mod-f を横取りしない。
    expect(panel).toContain("event.defaultPrevented");
  });

  it("発話の行に鍵と本文の印が付いている（告知の行と吹き出しの両方）", () => {
    const rows = panel.match(/:data-chat-row="entry\.row\.message\.id"/g) ?? [];
    expect(rows).toHaveLength(2);
    // 本文の印 = 告知のテキスト / Markdown の吹き出し / 素のテキストの吹き出し。
    const texts = panel.match(/\bdata-chat-text\b/g) ?? [];
    expect(texts.length).toBeGreaterThanOrEqual(3);
    // 行の印を引くのも、本文を歩くのも同じ属性名。
    expect(panel).toContain('[data-chat-row="${CSS.escape(id)}"]');
    expect(panel).toContain('querySelectorAll("[data-chat-text]")');
  });

  it("ハイライトの名前が ChatPanel と style.css で揃っている", () => {
    for (const name of ["chat-search", "chat-search-current"]) {
      expect(panel, name).toContain(`"${name}"`);
      expect(css, name).toMatch(new RegExp(`::highlight\\(${name}\\)\\s*\\{`));
    }
    expect(panel).toContain("CSS.highlights");
  });

  it("座標計算を持ち込まない", () => {
    expect(panel).not.toContain("getBoundingClientRect");
    expect(panel).not.toContain("elementFromPoint");
  });

  it("辞書の鍵が ja と en の両方に在る", () => {
    for (const key of [
      "chat.search.placeholder",
      "chat.search.count",
      "chat.search.none",
      "chat.search.prev",
      "chat.search.next",
      "chat.search.close",
    ]) {
      expect(typeof dig(ja, key), `ja: ${key}`).toBe("string");
      expect(typeof dig(en, key), `en: ${key}`).toBe("string");
      expect(panel, key).toContain(`'${key}'`.replace(/'/g, "'").length > 0 ? key : key);
    }
    // 件数の文は両言語とも同じ 2 つの引数を取る。
    for (const [lang, dict] of [["ja", ja], ["en", en]] as const) {
      const params = [...(dig(dict, "chat.search.count") as string).matchAll(/\{(\w+)\}/g)]
        .map((m) => m[1])
        .sort();
      expect(params, lang).toEqual(["current", "total"]);
    }
  });
});
