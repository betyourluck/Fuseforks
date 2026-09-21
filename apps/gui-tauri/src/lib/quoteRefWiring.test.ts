/**
 * 会話の参照（Spec 58）の配線を、ソースの走査で留める。
 *
 * ここで見ているのは**型検査にも実行時のテストにも掛からない結合**だけ —
 * IPC の引数の綴り（Rust と TS で別々に書く文字列）/ 手で揃えている上限の数 /
 * 実行時に引く辞書の鍵 / 入力欄が結果を待ってから下書きを戻すという順序 /
 * 会話ペインが座標計算を持たないという前提。
 */

import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

import { MAX_QUOTES } from "./quoteRef";

/** `apps/gui-tauri` からの相対パスで読む。 */
const read = (path: string): string =>
  readFileSync(fileURLToPath(new URL(`../../${path}`, import.meta.url)), "utf8");

const input = read("src/components/ChatInput.vue");
const panel = read("src/components/ChatPanel.vue");

describe("会話の参照の配線（Spec 58）", () => {
  it("IPC の引数の綴りが Rust と TS で揃っている", () => {
    // Tauri は `quoteIds` を `quote_ids` へ写す。片方だけ改名すると、参照は
    // **黙って空になる**（`Option` なのでエラーにならず、参照なしで送られる）。
    expect(read("src-tauri/src/commands.rs")).toMatch(/quote_ids: Option<Vec<String>>/);
    expect(read("src/lib/ipc.ts")).toMatch(/attachments, quoteIds \}\)/);
  });

  it("件数の上限がコアと同じ数", () => {
    const core = read("../../crates/fuseforks-core/src/quote.rs");
    const found = /pub const MAX_QUOTES: usize = (\d+);/.exec(core);
    expect(found, "quote.rs に MAX_QUOTES が在ること").not.toBeNull();
    expect(MAX_QUOTES).toBe(Number(found![1]));
  });

  it("入力欄は送信の結果を待ってから、失敗した下書きを戻す", () => {
    // `emit` では親の戻り値を受け取れない。関数の prop で受けていること。
    expect(input).toMatch(/await props\.submit\(/);
    expect(input).not.toMatch(/emit\("send"/);
    // **戻すのは失敗したときだけ、かつ必ず** — 成否の枝ごと留める（コンポーネントを
    // マウントする土台が無いので、順序はここで見る。規則そのものは `restoreDraft` の単体）。
    expect(input).toMatch(
      /const ok = await props\.submit\([\s\S]*?\);\s*if \(ok\) return;\s*const restored = restoreDraft\(/,
    );
    // 戻すのは 3 つとも。片方だけ戻す形にしない。
    for (const field of ["text.value = restored.text", "attachment.value = restored.attachment", "quotes.value = restored.quotes"]) {
      expect(input, field).toContain(field);
    }
    expect(panel).toContain(':submit="send"');
  });

  it("入力欄が運ぶのは発話 ID だけ", () => {
    expect(input).toMatch(/failed\.quotes\.map\(\(chip\) => chip\.id\)/);
  });

  it("補完の入れ替わりは種別で見る", () => {
    // 「開いているか」で見ると、`@` の直後に `@` を打ったときに候補が入れ替わらない。
    expect(input).toMatch(/\(\) => trigger\.value\?\.kind \?\? null/);
  });

  it("会話ペインに座標計算を足していない", () => {
    for (const source of [panel, input]) {
      expect(source).not.toMatch(/getBoundingClientRect|elementFromPoint/);
    }
  });

  it("使っている辞書の鍵が ja / en に在る", () => {
    const used = [
      "chat.quote.label",
      "chat.quote.toggle",
      "chatInput.quoteChars",
      "chatInput.quoteFull",
      "chatInput.quoteNone",
      "chatInput.quoteRemove",
      // 宛先は実行時に組む（`chatInput.quoteTo.${kind}`）。サーヴァント以外の 3 種。
      "chatInput.quoteTo.external",
      "chatInput.quoteTo.system",
      "chatInput.quoteTo.user",
      "errors.INVALID_QUOTE",
    ];
    // 走査が空振りしていないこと — 直書きの鍵はソースに実在する。
    for (const key of used.filter((k) => !k.startsWith("chatInput.quoteTo.") && !k.startsWith("errors."))) {
      expect(`${input}\n${panel}`, key).toContain(key);
    }
    expect(input).toContain("chatInput.quoteTo.${to.kind}");

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
});
