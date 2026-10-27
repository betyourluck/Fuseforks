/**
 * サーヴァント一覧の絞り込みの**配線**を機械で留める（2026-10-27）。
 *
 * 照合は `agentSearch.test.ts` の単体が守る。ここで留めるのは純関数の外で、どれも
 * 型検査に掛からない —
 * - 絞り込み欄の印 `data-agent-search` と、Alt+↑↓ の許容（`isNavigableFocus`）が同じ字面
 *   （片方だけ改名すると、欄にフォーカスがある間 Alt+↑↓ が黙って効かなくなる）
 * - Alt+↑↓ が当たった個体の中だけを巡る（外すと画面に出ていない個体へ飛ぶ）
 * - 絞り込み中はドラッグの並べ替えと受け入れを止める（Sortable は描いたカードしか返さない
 *   ので、絞った状態の並べ替えは隠れた個体の位置を決められない）
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

function read(rel: string): string {
  return readFileSync(fileURLToPath(new URL(rel, import.meta.url)), "utf8");
}

/** `//` と `/* *\/` と `<!-- -->` のコメントを落とす（説明文に名前を書くのは自由）。 */
function code(source: string): string {
  return source
    .replace(/<!--[\s\S]*?-->/g, "")
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/^\s*\/\/.*$/gm, "");
}

function between(source: string, start: string, end: string): string {
  const from = source.indexOf(start);
  expect(from, `${start} が無い`).toBeGreaterThanOrEqual(0);
  const to = source.indexOf(end, from + start.length);
  return source.slice(from, to === -1 ? undefined : to);
}

const list = code(read("../components/AgentList.vue"));
const app = code(read("../App.vue"));
const nav = code(read("./agentNav.ts"));

describe("一覧の絞り込み", () => {
  it("絞り込み欄の印と Alt+↑↓ の許容が同じ字面", () => {
    const input = between(list, 'v-model="searchQuery"', "/>");
    expect(input).toContain("data-agent-search");
    expect(nav).toContain('hasAttribute("data-agent-search")');
  });

  it("Alt+↑↓ は絞り込みに当たった個体の中だけを巡る", () => {
    const body = between(app, "function onNavKey", "\n}");
    expect(body).toContain("filterAgents(");
    expect(body).toContain("agentSearch.query.value");
  });

  it("絞り込み中はドラッグの並べ替えと受け入れを止める", () => {
    const box = between(list, "<VueDraggable", ">");
    expect(box).toContain(`:group="searching ? SEARCH_DRAG_GROUP : 'agents'"`);
    expect(box).toContain(':sort="!searching"');
    expect(list).toMatch(/SEARCH_DRAG_GROUP = \{ name: "agents", pull: true, put: false \}/);
    // 届いても保留しない（保留が残ると、地図でない場所で離したときに並びが書かれる）。
    const hold = between(list, "function holdReorder", "\n}");
    expect(hold).toContain("if (searching.value) return;");
  });

  it("掴んだカードは描いた並びから引く（区分けの全員から引かない）", () => {
    const end = between(list, "async function onDragEnd", "\n}");
    expect(end).toContain("draggedAgent(evt)");
    const dragged = between(list, "function draggedAgent", "\n}");
    expect(dragged).toContain("cardsOf(");
  });
});
