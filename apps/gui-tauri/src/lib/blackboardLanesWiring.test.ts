// @ts-expect-error @types/node を入れない方針のため（editorSaveWiring.test.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import en from "../locales/en.json";
import ja from "../locales/ja.json";
import { stateDictKey, STATES, type NoteBadge, type ReleasedReason } from "./blackboardLanes";

/**
 * 黒板の列とバッジの配線（Spec 54。2026-09-16 の 3 列を状態の列 + バッジへ）。
 *
 * 列とバッジの辞書の鍵は `BlackboardPane.vue` が**実行時に文字列で組む**
 * （`blackboard.state.${key}` / `blackboard.badge.${badge}` / `blackboard.reason.${reason}`）
 * ので、鍵が 1 つ欠けても vue-tsc も `i18n/index.test.ts` のコンパイル検査も通り、
 * 画面に鍵名がそのまま出る。ここで純関数の列挙と辞書を突き合わせる。
 */
const pane = readFileSync(new URL("../components/BlackboardPane.vue", import.meta.url), "utf8");

const REASONS: ReleasedReason[] = ["orphanUnknown", "orphanMoved", "failed", "stopped", "waiting"];
const LANE_BADGES: Exclude<NoteBadge, ReleasedReason>[] = ["active", "yourTurn"];

function keysOf(dict: unknown, path: string): string[] {
  const node = path.split(".").reduce<unknown>((n, k) => (n as Record<string, unknown>)?.[k], dict);
  return node && typeof node === "object" ? Object.keys(node as object).sort() : [];
}

describe("黒板の列とバッジの配線", () => {
  it("画面は純関数 kanbanNotes を読み、自前で列を決めない", () => {
    expect(pane).toContain("kanbanNotes(notes.value");
    expect(pane).not.toMatch(/lane\s*===\s*["']pending["']/);
    // 5 値をテンプレートに直書きしない（列順は純関数の STATES が持つ）。
    expect(pane).not.toMatch(/\[\s*["']doing["']\s*,/);
  });

  it("列の辞書は STATES（camelCase）+ summary / unfiled / other と一致する（ja / en とも）", () => {
    const expected = [...STATES.map(stateDictKey), "summary", "unfiled", "other"].sort();
    expect(keysOf(ja, "blackboard.state")).toEqual(expected);
    expect(keysOf(en, "blackboard.state")).toEqual(expected);
    expect(keysOf(ja, "blackboard.stateTitle")).toEqual(expected);
    expect(keysOf(en, "blackboard.stateTitle")).toEqual(expected);
    // 「その他」の見出しはフォルダ名を運ぶ（凍結 1）。
    for (const dict of [ja, en]) {
      const state = (dict as { blackboard: { state: Record<string, string> } }).blackboard.state;
      expect(state.other).toContain("{name}");
    }
  });

  it("バッジの辞書は手番 2 値 + duplicate、内訳は ReleasedReason と一致する（ja / en とも）", () => {
    const badges = [...LANE_BADGES, "duplicate"].sort();
    expect(keysOf(ja, "blackboard.badge")).toEqual(badges);
    expect(keysOf(en, "blackboard.badge")).toEqual(badges);
    const reasons = [...REASONS].sort();
    expect(keysOf(ja, "blackboard.reason")).toEqual(reasons);
    expect(keysOf(en, "blackboard.reason")).toEqual(reasons);
    // 「手が離れている」という列名は消えた — 内訳だけを出す。
    expect(keysOf(ja, "blackboard.lane")).toEqual([]);
    expect(keysOf(en, "blackboard.lane")).toEqual([]);
  });

  it("列の一括の消し口は「完了」にだけ付く（凍結 8）", () => {
    const buttons = pane.match(/<button[\s\S]*?@click="clearDone"/g) ?? [];
    expect(buttons).toHaveLength(1);
    expect(buttons[0]).toContain('v-if="isDone(section)"');
    expect(pane).not.toContain("clearReleased");
  });

  it("一括削除は既存の IPC を 1 枚ずつ回し、state を渡す（新しい IPC は無い）", () => {
    expect(pane).toMatch(/deleteBlackboardNote\(note\.dir, note\.name, note\.state\)/);
    expect(pane).not.toContain("clearBlackboardState");
  });

  it("全消しの消し口は残す（列の消し口は置き換えではなく追加）", () => {
    expect(pane).toContain('@click="clearAll"');
  });

  it("付箋が 0 枚でも列を出す — 空の文言は列の代わりではなく列の下", () => {
    expect(pane).not.toMatch(/v-else-if="loaded && notes\.length === 0"/);
    const lanes = pane.indexOf('<section v-for="section in sections"');
    const empty = pane.indexOf('$t("blackboard.empty")');
    expect(lanes).toBeGreaterThan(-1);
    expect(empty).toBeGreaterThan(lanes);
  });

  it("畳みは部品の外の composable に持ち、鍵は state を含む noteKey を使う", () => {
    expect(pane).toContain('from "../composables/useBlackboardCollapse"');
    expect(pane).toMatch(/v-show="!collapse\.isCollapsed\(note\)"/);
    expect(pane).toContain("collapse.prune(notes.value)");
    expect(pane).toContain(':key="noteKey(note)"');
    for (const key of ["blackboard.expand", "blackboard.collapse"]) {
      expect(keysOf(ja, "blackboard")).toContain(key.split(".")[1]);
      expect(keysOf(en, "blackboard")).toContain(key.split(".")[1]);
    }
  });
});
