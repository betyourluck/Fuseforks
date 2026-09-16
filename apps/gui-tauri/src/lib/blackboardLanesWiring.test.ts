// @ts-expect-error @types/node を入れない方針のため（editorSaveWiring.test.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import en from "../locales/en.json";
import ja from "../locales/ja.json";
import { LANES, type ReleasedReason } from "./blackboardLanes";

/**
 * 黒板の 3 列の配線（2026-09-16）。
 *
 * 列と内訳バッジの辞書の鍵は `BlackboardPane.vue` が**実行時に文字列で組む**
 * （`blackboard.lane.${key}` / `blackboard.reason.${reason}`）ので、鍵が 1 つ欠けても
 * vue-tsc も `i18n/index.test.ts` のコンパイル検査も通り、画面に鍵名がそのまま出る。
 * ここで純関数の列挙と辞書を突き合わせる。
 */
const pane = readFileSync(new URL("../components/BlackboardPane.vue", import.meta.url), "utf8");

const REASONS: ReleasedReason[] = ["orphanUnknown", "orphanMoved", "failed", "stopped", "waiting"];

function keysOf(dict: unknown, path: string): string[] {
  const node = path.split(".").reduce<unknown>((n, k) => (n as Record<string, unknown>)?.[k], dict);
  return node && typeof node === "object" ? Object.keys(node as object).sort() : [];
}

describe("黒板の 3 列の配線", () => {
  it("画面は純関数 laneNotes と LANES を読み、自前で列を決めない", () => {
    expect(pane).toContain("laneNotes(notes.value");
    expect(pane).toContain("LANES.map(");
    expect(pane).not.toMatch(/lane\s*===\s*["']pending["']/);
  });

  it("列の辞書は LANES + summary と一致する（ja / en とも）", () => {
    const expected = [...LANES, "summary"].sort();
    expect(keysOf(ja, "blackboard.lane")).toEqual(expected);
    expect(keysOf(en, "blackboard.lane")).toEqual(expected);
    expect(keysOf(ja, "blackboard.laneTitle")).toEqual(expected);
    expect(keysOf(en, "blackboard.laneTitle")).toEqual(expected);
  });

  it("内訳バッジの辞書は ReleasedReason と一致する（ja / en とも）", () => {
    const expected = [...REASONS].sort();
    expect(keysOf(ja, "blackboard.reason")).toEqual(expected);
    expect(keysOf(en, "blackboard.reason")).toEqual(expected);
  });

  it("列の一括の消し口は「手が離れている」にだけ付く", () => {
    const buttons = pane.match(/<button[\s\S]*?@click="clearReleased"/g) ?? [];
    expect(buttons).toHaveLength(1);
    expect(buttons[0]).toContain('v-if="isReleased(section.key)"');
  });

  it("全消しの消し口は残す（列の消し口は置き換えではなく追加）", () => {
    expect(pane).toContain('@click="clearAll"');
  });
});
