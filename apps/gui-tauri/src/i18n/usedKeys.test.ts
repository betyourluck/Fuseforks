/**
 * **字面で書いた辞書の鍵が、ja / en の両方に実在するか**を走査で留める（2026-09-28）。
 *
 * 起点は実機 — システム設定の保存ボタンが `common.save` の字面のまま出ていた。鍵は Spec 41 P3 と
 * Spec 59 P2 の 2 箇所で使われていたが、`common` には `close` しか無かった。vue-i18n は欠けた鍵を
 * 例外にせず**鍵そのものを表示する**ので、型検査にも既存の「ja と en の鍵集合が一致するか」の
 * 検査にも掛からない（両方に無いので一致している）。
 *
 * 見るのは `$t("…")` / `t("…")` / `te("…")` の**文字列リテラル**だけ。組み立てる鍵
 * （`` `blackboard.lane.${key}` `` など）は各機能の配線テストが列挙と突き合わせている。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync, readdirSync, statSync } from "node:fs";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

import en from "../locales/en.json";
import ja from "../locales/ja.json";

const srcDir: string = fileURLToPath(new URL("../", import.meta.url));

function walk(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir) as string[]) {
    const path = `${dir}${dir.endsWith("/") || dir.endsWith("\\") ? "" : "/"}${name}`;
    if ((statSync(path) as { isDirectory(): boolean }).isDirectory()) {
      if (name === "locales" || name === "node_modules") continue;
      out.push(...walk(path));
    } else if (/\.(vue|ts)$/.test(name) && !/\.test\.ts$/.test(name)) {
      out.push(path);
    }
  }
  return out;
}

function has(dict: unknown, key: string): boolean {
  let node: unknown = dict;
  for (const part of key.split(".")) {
    if (typeof node !== "object" || node === null || !(part in node)) return false;
    node = (node as Record<string, unknown>)[part];
  }
  return typeof node === "string";
}

/** `$t("a.b")` / `t('a.b', …)` / `te("a.b")` の字面の鍵。`.` を含むものだけ（鍵は必ず節を持つ）。 */
function literalKeys(source: string): string[] {
  const keys: string[] = [];
  const re = /(?<![\w$.])\$?te?\(\s*(["'])([A-Za-z][\w-]*(?:\.[\w-]+)+)\1/g;
  for (const m of source.matchAll(re)) keys.push(m[2]);
  return keys;
}

describe("字面で書いた辞書の鍵", () => {
  const files = walk(srcDir);
  const used = new Map<string, string>();
  for (const file of files) {
    for (const key of literalKeys(readFileSync(file, "utf8") as string)) {
      if (!used.has(key)) used.set(key, file.slice(srcDir.length));
    }
  }

  it("走査が空振りしていない（検定）", () => {
    expect(used.size).toBeGreaterThan(300);
    expect(used.has("settings.jev.heading")).toBe(true);
  });

  it("ja と en の両方に在る", () => {
    const missing = [...used].filter(([key]) => !has(ja, key) || !has(en, key)).map(([key, file]) => `${key} (${file})`);
    expect(missing).toEqual([]);
  });
});
