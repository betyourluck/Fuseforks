// @ts-expect-error @types/node を入れない方針のため（editorSaveWiring.test.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import { providerSkills } from "./providerSkills";
import type { ModelTemplate } from "../types";

/**
 * 固有スキルの「効かない設定が残っている」行（ModelTemplateDialog の `strandedRows`）の配線
 * （Spec 71 P2 で新設）。
 *
 * `providerSkills()` が `stranded` を返しても、ダイアログに行が無ければ**警告も「オフにする」も
 * 出ない** — 型検査にも lint にも掛からない形で壊れる（行は手で 1 本ずつ書いており、スキルを
 * 足した人が書き忘れても誰も落ちない）。辞書の鍵も実行時に `$t(row.labelKey)` で引くので、
 * 欠けても通る。ここで両方を走査で留める。
 */
const dialog = readFileSync(new URL("../components/ModelTemplateDialog.vue", import.meta.url), "utf8");
const ja = JSON.parse(readFileSync(new URL("../locales/ja.json", import.meta.url), "utf8"));
const en = JSON.parse(readFileSync(new URL("../locales/en.json", import.meta.url), "utf8"));

/** `strandedRows` の computed の本体（`const strandedRows = computed(() => {` から `return rows;` まで）。 */
function strandedBlock(): string {
  const start = dialog.indexOf("const strandedRows = computed(");
  const end = dialog.indexOf("return rows;", start);
  expect(start).toBeGreaterThan(-1);
  expect(end).toBeGreaterThan(start);
  return dialog.slice(start, end);
}

/** `a.b.c` の鍵が辞書に在るか。 */
function has(dict: Record<string, unknown>, dotted: string): boolean {
  let node: unknown = dict;
  for (const part of dotted.split(".")) {
    if (typeof node !== "object" || node === null || !(part in node)) return false;
    node = (node as Record<string, unknown>)[part];
  }
  return typeof node === "string";
}

const draft: Parameters<typeof providerSkills>[0] = {
  provider: null,
  googleSearch: false,
  geminiUrlContext: false,
  xaiWebSearch: false,
  xaiXSearch: false,
  openaiWebSearch: false,
  openaiReasoningPro: false,
  metaWebSearch: false,
  perplexityWebSearch: false,
  perplexityFinanceSearch: false,
  perplexityPeopleSearch: false,
  perplexityFetchUrl: false,
  anthropicWebSearch: false,
} satisfies Partial<ModelTemplate>;

describe("固有スキルの stranded 行の配線", () => {
  it("操作できる固有スキルはどれも stranded の行を持つ", () => {
    const skillKeys = Object.keys(providerSkills(draft))
      .filter((k) => k !== "passive" && k !== "anyOffered")
      .sort();
    const rowKeys = [...strandedBlock().matchAll(/\bkey: "(\w+)"/g)].map((m) => m[1]).sort();
    expect(rowKeys).toEqual(skillKeys);
  });

  it("各行の判定・辞書の鍵・消し方が揃っている", () => {
    const rows = [
      ...strandedBlock().matchAll(/if \(s\.(\w+)\.stranded\)\s*rows\.push\(\{([^}]*)\}\);/g),
    ];
    // 当たらなければ下の for は 1 度も回らず緑になる — 数を先に合わせる。
    const skillCount = Object.keys(providerSkills(draft)).length - 2;
    expect(rows.length).toBe(skillCount);
    for (const m of rows) {
      const [, skill, fields] = m;
      expect(fields, skill).toContain(`key: "${skill}"`);
      for (const k of fields.matchAll(/(?:labelKey|ownerKey|strongKey|afterKey): "([\w.]+)"/g)) {
        expect(has(ja, k[1]), `ja: ${k[1]}`).toBe(true);
        expect(has(en, k[1]), `en: ${k[1]}`).toBe(true);
      }
      expect(fields, skill).toMatch(/clear: \(\) => \(d\.\w+ = false\)/);
    }
  });
});
