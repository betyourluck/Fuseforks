// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { idTakenByAnother } from "./templateDraft";

describe("idTakenByAnother", () => {
  const ids = ["gpt5", "claude"];

  it("新規の下書きが登録済みの識別子と同じなら重なり", () => {
    expect(idTakenByAnother(true, "gpt5", ids)).toBe(true);
  });

  it("登録済みの識別子に続けて打った途中の文字列は重ならない", () => {
    expect(idTakenByAnother(true, "gpt5_", ids)).toBe(false);
    expect(idTakenByAnother(true, "gpt5_mini", ids)).toBe(false);
  });

  it("登録済みの行の編集は自分の識別子と同じでも重なりではない", () => {
    expect(idTakenByAnother(false, "gpt5", ids)).toBe(false);
  });
});

/**
 * 配線の走査。新規かどうかを識別子から逆算すると、既存と同じ文字列を打った瞬間に欄が読み取り専用になる
 * （2026-11-05 の不具合）。字面で留める。
 */
describe("ModelTemplateDialog の識別子の配線", () => {
  const src = readFileSync(new URL("../components/ModelTemplateDialog.vue", import.meta.url), "utf8");

  it("新規かどうかは create が立てる印で持ち、登録済みの一覧から逆算しない", () => {
    const def = src.match(/const isNewDraft = computed\(([^;]*)\);/);
    expect(def).not.toBeNull();
    expect(def![1]).toContain("creating.value");
    expect(def![1]).not.toContain("state.templates");
  });

  it("重なっている間は保存も鍵の登録も止める", () => {
    expect(src).toMatch(/:disabled="saving \|\| idTaken"/);
    expect(src).toMatch(/:disabled="!secretInput \|\| savingSecret \|\| idTaken"/);
    expect(src).toMatch(/saving\.value \|\| idTaken\.value\) return;/);
    expect(src).toMatch(/!secret \|\| idTaken\.value\) return;/);
  });
});
