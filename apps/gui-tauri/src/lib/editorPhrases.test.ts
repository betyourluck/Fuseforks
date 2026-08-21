/**
 * 検索パネルの文言表が、ライブラリが実際に引く原文と食い違っていないことを
 * 機械で留める。
 *
 * `@codemirror/search` の文面は**こちらのコードに一度も現れない** — 表の鍵は
 * ライブラリの実装が持つ英語の文字列で、増えても減ってもコンパイラにも lint にも
 * 引っかからない。抜けると**その 1 語だけが英語で残る**（`defaultEnabledTools.test.ts`
 * が Rust の表と突き合わせているのと同じ形の網）。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { dirname, resolve } from "node:path";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

import { searchPhrases } from "./editorPhrases";

const here = dirname(fileURLToPath(import.meta.url));

const librarySource = readFileSync(
  resolve(here, "../../node_modules/@codemirror/search/dist/index.js"),
  "utf-8",
);

/**
 * ライブラリが `phrase()` へ渡している原文を全部集める。呼び方は 2 通りあり、
 * **片方だけ数えると取りこぼす** — パネルの部品は自前の薄い包み
 * `phrase(view, "…")` を通し、通知と行移動は `state.phrase("…")` を直に呼ぶ。
 */
function phrasesUsedByLibrary(source: string): string[] {
  const found = new Set<string>();
  for (const m of source.matchAll(/phrase\(\s*view\s*,\s*"([^"]+)"/g)) found.add(m[1]);
  for (const m of source.matchAll(/\.phrase\(\s*"([^"]+)"/g)) found.add(m[1]);
  return [...found].sort();
}

describe("searchPhrases", () => {
  it("走査そのものが空振りしていない", () => {
    // 0 件を「全部訳せている」と読まないための対照（`failures.md` #90 の処方）。
    expect(phrasesUsedByLibrary(librarySource).length).toBeGreaterThan(10);
  });

  it("ライブラリが引く原文をすべて日本語で持っている", () => {
    const ja = searchPhrases("ja");
    const missing = phrasesUsedByLibrary(librarySource).filter((p) => !(p in ja));
    expect(missing).toEqual([]);
  });

  it("使われていない鍵を抱えていない", () => {
    // 逆向き。ライブラリ更新で文面が消えたときに、死んだ鍵が残り続けるのを防ぐ。
    const used = new Set(phrasesUsedByLibrary(librarySource));
    const stale = Object.keys(searchPhrases("ja")).filter((k) => !used.has(k));
    expect(stale).toEqual([]);
  });

  it("差し替えの席 `$` を落としていない", () => {
    const ja = searchPhrases("ja");
    for (const [key, value] of Object.entries(ja)) {
      expect(value.includes("$"), `${key} の $ が落ちている`).toBe(key.includes("$"));
    }
  });

  it("英語は上書きしない", () => {
    expect(searchPhrases("en")).toEqual({});
  });
});
