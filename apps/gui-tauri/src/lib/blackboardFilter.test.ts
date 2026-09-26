// @ts-expect-error @types/node を入れない方針のため（editorSaveWiring.test.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import {
  effectiveFilter,
  filterColumns,
  foldAllAction,
  ownerOptions,
} from "./blackboardFilter";
import { kanbanNotes, type LaneAgent } from "./blackboardLanes";

const DIR = "D:\\Outcasts.jp";

/** id は表示名と違う綴りにする — 名前で引いても通る実装を見分けるため。 */
function agent(name: string): LaneAgent {
  return { id: `agent_${name}`, name, status: "running", workDir: DIR };
}

const luna = agent("ルナ");
const zari = agent("ザリ");

function board() {
  return kanbanNotes(
    [
      { dir: DIR, state: "doing", name: `${luna.id} - 調査.md` },
      { dir: DIR, state: "done", name: `${luna.id} - 束ね.md` },
      { dir: DIR, state: "doing", name: `${zari.id} - 検証.md` },
      // 区切りの無い名前 = 持ち主が引けない
      { dir: DIR, state: "done", name: "まとめ.md" },
    ],
    { agents: [luna, zari], typing: {}, waves: [] },
  ).columns;
}

const names = (columns: ReturnType<typeof board>) =>
  columns.map((c) => c.notes.map((n) => n.name));

describe("blackboardFilter: 持ち主の選択肢", () => {
  it("付箋を持つ個体だけを表示名の順に並べ、持ち主不明は別の印で持つ", () => {
    const options = ownerOptions(board());
    expect(options.owners).toEqual([
      { id: zari.id, name: "ザリ" },
      { id: luna.id, name: "ルナ" },
    ]);
    expect(options.hasUnowned).toBe(true);
  });
});

describe("blackboardFilter: 絞り込み", () => {
  it("列の形（数と順）は変えず、列の中の付箋だけを減らす", () => {
    const all = board();
    const filtered = filterColumns(all, luna.id);
    expect(filtered.map((c) => c.state)).toEqual(all.map((c) => c.state));
    expect(names(filtered)).toEqual([
      [`${luna.id} - 調査.md`],
      [],
      [`${luna.id} - 束ね.md`],
    ]);
  });

  it("持ち主は id で照合する（表示名で照合すると同名の別個体を拾う）", () => {
    expect(names(filterColumns(board(), "ルナ")).flat()).toEqual([]);
  });

  it("unowned は持ち主が引けない付箋だけ", () => {
    expect(names(filterColumns(board(), "unowned")).flat()).toEqual(["まとめ.md"]);
  });

  it("all は何も落とさない", () => {
    expect(names(filterColumns(board(), "all"))).toEqual(names(board()));
  });

  it("選べなくなった値は all へ戻す（付箋が消えたのに黒板が空に見えるのを防ぐ）", () => {
    const options = ownerOptions(board());
    expect(effectiveFilter(luna.id, options)).toBe(luna.id);
    expect(effectiveFilter("agent_消えた", options)).toBe("all");
    expect(effectiveFilter("unowned", { owners: options.owners, hasUnowned: false })).toBe("all");
  });
});

describe("blackboardFilter: すべて畳む", () => {
  it("1 枚でも開いていれば畳む側、全部畳まれていれば開く側", () => {
    expect(foldAllAction([1, 2], (n) => n === 1)).toBe("collapse");
    expect(foldAllAction([1, 2], () => true)).toBe("expand");
    expect(foldAllAction([], () => true)).toBe("collapse");
  });
});

describe("blackboardFilter: 配線", () => {
  const pane = readFileSync(new URL("../components/BlackboardPane.vue", import.meta.url), "utf8");

  it("列は絞り込んだ後の列から組み、畳むのは見えている付箋だけ", () => {
    expect(pane).toMatch(/filterColumns\(board\.value\.columns, activeFilter\.value\)/);
    expect(pane).toMatch(/collapse\.setCollapsed\(visibleNotes\.value/);
  });

  it("「完了」の一括消しは見えている列から取る（絞り込み中に隠れた付箋を消さない）", () => {
    expect(pane).toMatch(/sections\.value\.find\(\(s\) => isDone\(s\)\)/);
  });

  it("絞り込みは保存しない（localStorage を読まない）", () => {
    const composable = readFileSync(
      new URL("../composables/useBlackboardFilter.ts", import.meta.url),
      "utf8",
    );
    expect(composable).not.toMatch(/localStorage\./);
  });
});
