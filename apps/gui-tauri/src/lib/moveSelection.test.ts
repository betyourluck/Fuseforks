// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { isShiftKey, toggleMembers } from "./moveSelection";

describe("toggleMembers", () => {
  it("選ばれていない区分けは足す（既に選んだ個体は重ねない）", () => {
    expect(toggleMembers(["a"], ["b", "c"])).toEqual(["a", "b", "c"]);
    expect(toggleMembers(["a", "b"], ["b", "c"])).toEqual(["a", "b", "c"]);
  });

  it("全員が選ばれている区分けは外す（ほかの区分けの選択は残す）", () => {
    expect(toggleMembers(["a", "b", "c"], ["b", "c"])).toEqual(["a"]);
  });

  it("地図に出ている個体が居ない区分けは何も変えない", () => {
    expect(toggleMembers(["a"], [])).toEqual(["a"]);
  });
});

describe("isShiftKey", () => {
  it("Shift だけを拾う", () => {
    expect(isShiftKey({ key: "Shift" })).toBe(true);
    expect(isShiftKey({ key: "Control" })).toBe(false);
  });
});

/** 配線の走査。Shift なしで選択が作られる形・離しても残る形・1 体ずつ保存する形を字面で留める。 */
describe("TopologyMap の移動のための選択の配線", () => {
  const src = readFileSync(new URL("../components/TopologyMap.vue", import.meta.url), "utf8");

  it("選べるのは Shift を押している間だけで、選択は地図と左ペインが共有する状態", () => {
    expect(src).toContain("selectable: () => shiftHeld.value,");
    expect(src).toContain('v-model:selected-nodes="moveSelection"');
  });

  it("Shift のクリックは会話の選択を変えない", () => {
    expect(src).toMatch(/if \(event\.shiftKey && !linkSource\.value\) return;\s+if \(linkSource\.value\)/);
  });

  it("Shift を離すか窓が外れると消える。ドラッグ中はドラッグの終わりまで残す", () => {
    expect(src).toContain('window.addEventListener("keyup", onKeyup);');
    expect(src).toContain('window.addEventListener("blur", releaseShift);');
    expect(src).toContain("if (!dragging.value) clearMoveSelection();");
    expect(src).toContain("if (!shiftHeld.value) clearMoveSelection();");
  });

  it("複数を動かしたら 1 回でまとめて保存する", () => {
    const drag = src.slice(src.indexOf('"node:dragend"'), src.indexOf("};", src.indexOf('"node:dragend"')));
    expect(drag).toContain("entries.length > 1");
    expect(drag).toContain("orchestrator.setTopologyPositions(");
  });

  it("移動のための選択は拡大しない（半径は会話の選択だけが変える）", () => {
    expect(src).toContain("radius: (node) => (node.selected ? NODE_RADIUS * 1.5 : NODE_RADIUS)");
    const rule = src.slice(src.indexOf(".is-move-selected .kizuna-ring"));
    expect(rule.slice(0, rule.indexOf("}"))).not.toContain("scale");
  });
});

describe("AgentList の見出しの Shift クリック", () => {
  const src = readFileSync(new URL("../components/AgentList.vue", import.meta.url), "utf8");

  it("Shift のときだけ、ボタンと掴み手以外で、地図に出ている個体だけを入れる", () => {
    const fn = src.slice(src.indexOf("function onHeaderClick"), src.indexOf("async function toggleGate"));
    expect(fn).toContain("if (!event.shiftKey) return;");
    expect(fn).toContain('closest("button, [data-group-handle]")');
    expect(fn).toContain("isCollapsed(section) ? [] :");
    expect(src).toContain('@click="(e: MouseEvent) => onHeaderClick(section, e)"');
  });
});
