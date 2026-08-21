/**
 * `Ctrl+S` の受け口が、保存ボタンと**同じ述語**で守られていることを機械で留める。
 *
 * `CodeEditor` は合図（`save`）を出すだけで、押せるかどうかは親が決める。
 * ここが割れると **`Ctrl+S` だけがボタンの無効条件をすり抜ける** — 保存中に
 * 二重で走る / 壊れた JSON を保存しにいく、という形で出る。**型検査にも
 * 実行時にも引っかからない**（どちらの経路も単独では正しく見える）。
 *
 * 検査するのは 3 点だけで、文言も並びも留めない。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { dirname, resolve } from "node:path";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

/** 書き込みができる `CodeEditor` を持つ面。読み取り専用の面はここに載せない。 */
const EDITABLE_HOSTS = [
  "OrdinanceDialog.vue",
  "McpDialog.vue",
  "MarkdownEditor.vue",
  "RoleDialog.vue",
];

const sources = new Map(
  EDITABLE_HOSTS.map((name) => [
    name,
    readFileSync(resolve(here, "../components", name), "utf-8"),
  ]),
);

describe("Ctrl+S の配線", () => {
  it.each(EDITABLE_HOSTS)("%s は CodeEditor の save を受けている", (name) => {
    expect(sources.get(name)).toContain('@save="saveFromEditor"');
  });

  it.each(EDITABLE_HOSTS)("%s は canSave で守っている", (name) => {
    expect(sources.get(name)).toContain("if (canSave.value) void save();");
  });

  it.each(EDITABLE_HOSTS)("%s の保存ボタンは canSave の否定を見ている", (name) => {
    // ここが `:disabled="loading || …"` のような直書きへ戻ると、条件が
    // 2 箇所に生えてボタンと `Ctrl+S` が別々に腐りはじめる。
    expect(sources.get(name)).toContain(':disabled="!canSave"');
  });
});

describe("CodeEditor の鍵", () => {
  const editor = readFileSync(resolve(here, "../components/CodeEditor.vue"), "utf-8");

  it("Mod- で書いてある（Ctrl- だと mac で効かない）", () => {
    expect(editor).toContain('key: "Mod-s"');
    expect(editor).toContain('key: "Mod-r"');
    expect(editor).not.toContain('key: "Ctrl-');
  });

  it("既定の keymap より先に置いてある", () => {
    // 同じ keymap では先頭ほど強い。後ろへ回ると既定に食われうる。
    expect(editor.indexOf('key: "Mod-s"')).toBeLessThan(editor.indexOf("indentWithTab,"));
    expect(editor.indexOf('key: "Mod-r"')).toBeLessThan(editor.indexOf("indentWithTab,"));
  });
});
