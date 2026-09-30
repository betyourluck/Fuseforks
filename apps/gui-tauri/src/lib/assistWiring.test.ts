/**
 * AI 下書き補助（Spec 63）の配線を**ソースの走査**で留める。型検査にも lint にも掛からない約束だけ:
 *
 * - 入口は SKILL / Construct（`MarkdownEditor`）と判断役（`JudgeDialog`）の 3 箇所だけ（凍結 2）
 * - パネルは `appended` の**中身を読まない**（凍結 4 — 逐語で送り返すだけ）
 * - パネルに保存の経路が無い（凍結 1 — 保存は開いた側の既存の保存ボタン）
 * - 実行時に組み立てる辞書の鍵（`assist.title.${kind}` など）が ja / en に在る
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readdirSync, readFileSync } from "node:fs";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

import en from "../locales/en.json";
import ja from "../locales/ja.json";

const components = fileURLToPath(new URL("../components/", import.meta.url));
const read = (name: string): string => readFileSync(`${components}${name}`, "utf8");
const vueFiles: string[] = (readdirSync(components) as string[]).filter((f: string) => f.endsWith(".vue"));

describe("AI 下書き補助の入口（凍結 2）", () => {
  it("パネルを開くのは MarkdownEditor と JudgeDialog だけ", () => {
    const users = vueFiles.filter((f) => read(f).includes("<AssistPanel"));
    expect(users.sort()).toEqual(["JudgeDialog.vue", "MarkdownEditor.vue"]);
    // 走査が空振りしていないことの検定（0 件なら上の比較が偶然通る形にしない）。
    expect(vueFiles.length).toBeGreaterThan(20);
  });

  it("MarkdownEditor では SKILL と Construct のタブだけに出る", () => {
    const src = read("MarkdownEditor.vue");
    expect(src).toContain('props.editable && (kind.value === "skill" || kind.value === "construct")');
    expect(src).toContain("v-if=\"assistOpen && (kind === 'skill' || kind === 'construct')\"");
  });
});

describe("AssistPanel", () => {
  const src = read("AssistPanel.vue");

  it("appended は中身を読まずに履歴へ足すだけ（凍結 4）", () => {
    expect(src).toContain("history.value = [...history.value, ...reply.appended];");
    for (const forbidden of ["appended[", "appended.map", "appended.find", "appended.filter", "toolCalls", "tool_calls"]) {
      expect(src, forbidden).not.toContain(forbidden);
    }
  });

  it("保存の経路を持たない（凍結 1 — 反映は apply を出すだけ）", () => {
    for (const forbidden of ["writeAgentConfig", "writeConfig", "saveJudgeFile", "save_judge_file"]) {
      expect(src, forbidden).not.toContain(forbidden);
    }
    expect(src).toContain('emit("apply", draft.value.content)');
  });

  it("覆いは bg-scrim（「/」で入力欄へ移る判定がこの印を見る）で、ダイアログ（z-40）の上・確認（z-60）の下", () => {
    expect(src).toContain("fixed inset-0 z-50 flex items-center justify-center bg-scrim");
  });
});

describe("実行時に組み立てる辞書の鍵", () => {
  type Dict = { assist: Record<string, Record<string, string>>; errors: Record<string, string> };
  it.each(["skill", "construct", "judge"])("assist.title / assist.placeholder の %s が ja / en に在る", (kind) => {
    for (const dict of [ja, en] as unknown as Dict[]) {
      expect(typeof dict.assist.title[kind]).toBe("string");
      expect(typeof dict.assist.placeholder[kind]).toBe("string");
    }
  });

  it("コアのエラーコード INVALID_ASSIST_REQUEST の訳がある（無いと原文のまま出る）", () => {
    for (const dict of [ja, en] as unknown as Dict[]) {
      expect(typeof dict.errors.INVALID_ASSIST_REQUEST).toBe("string");
    }
  });
});
