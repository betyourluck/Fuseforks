// @ts-expect-error @types/node を入れない方針のため（editorSaveWiring.test.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { BROAD_KINDS } from "./broadPattern";

/**
 * 広い許可パターンの警告の配線（2026-11-11）。
 *
 * 判定は `broadPattern.ts` の純関数が持ち、ここが確かめるのは**画面がそれを読んでいること**と、
 * 種類ごとの辞書の鍵が 3 言語に揃っていること。鍵は実行時に組む（`broadPattern.${kind}`）ので、
 * `usedKeys.test.ts` の字面の網には掛からない。
 *
 * `CommandApprovalDialog.vue` は NUL を含む（行の鍵の区切り）ので、比べる前に落とす。
 */
const dialog = readFileSync(
  new URL("../components/CommandApprovalDialog.vue", import.meta.url),
  "utf8",
).replaceAll("\u0000", "");
const editor = readFileSync(new URL("../components/MarkdownEditor.vue", import.meta.url), "utf8");
const locales = ["ja", "en", "zh-Hans"].map((name) => ({
  name,
  dict: JSON.parse(readFileSync(new URL(`../locales/${name}.json`, import.meta.url), "utf8")),
}));

function lookup(dict: Record<string, unknown>, key: string): unknown {
  return key.split(".").reduce<unknown>(
    (node, part) => (node as Record<string, unknown> | undefined)?.[part],
    dict,
  );
}

describe("広い許可パターンの警告の配線", () => {
  it("承認画面は「広いほう」の文字列を純関数に掛けて警告を出す", () => {
    // 出すかどうかの条件そのものを見る — 本文の補間にも同じ字面があるので、字面の有無だけだと
    // 条件を完全一致の側へ変えても緑になる（変異 M6 で踏んだ）。
    expect(dialog).toContain('v-if="broadPatternKind(patternOf(request, true)) !== null"');
    expect(dialog).not.toContain("broadPatternKind(patternOf(request, false))");
    expect(dialog).toMatch(/\$t\(`broadPattern\.\$\{/);
  });

  it("警告は「広いほう」の選択肢の下に置く（完全一致の側には出さない）", () => {
    const prefix = dialog.indexOf("commandApproval.prefix");
    const warning = dialog.indexOf("broadPatternKind(patternOf(request, true))");
    const buttons = dialog.indexOf("commandApproval.reject");
    expect(prefix).toBeGreaterThan(-1);
    expect(warning).toBeGreaterThan(prefix);
    expect(buttons).toBeGreaterThan(warning);
  });

  it("run.json のタブは編集中の本文から拾い、run.json のときだけ出す", () => {
    expect(editor).toContain("broadAllowEntries(content.value)");
    expect(editor).toMatch(/kind\.value === "run" \? broadAllowEntries\(content\.value\)/);
    expect(editor).toContain("broadPattern.listTitle");
  });

  it("種類ごとの文と見出しが 3 言語に揃っている", () => {
    for (const { name, dict } of locales) {
      for (const kind of BROAD_KINDS) {
        const text = lookup(dict, `broadPattern.${kind}`);
        expect(typeof text, `${name}: broadPattern.${kind}`).toBe("string");
        expect((text as string).length, `${name}: broadPattern.${kind}`).toBeGreaterThan(0);
      }
      expect(typeof lookup(dict, "broadPattern.listTitle"), `${name}: listTitle`).toBe("string");
    }
  });

  it("辞書に種類の外の鍵を残さない（種類を減らしたら辞書からも消す）", () => {
    for (const { name, dict } of locales) {
      const keys = Object.keys(lookup(dict, "broadPattern") as Record<string, unknown>).sort();
      expect(keys, name).toEqual([...BROAD_KINDS, "listTitle"].sort());
    }
  });
});
