// @ts-expect-error @types/node を入れない方針のため（editorSaveWiring.test.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

/**
 * 絆の地図の上で絆を張る・切る操作の配線（2026-10-24）。
 *
 * 規則（起点にできるか・どの向きが切れるか）は `kizunaLink.ts` の純関数が持ち、単体で留めている。
 * ここで確かめるのは**地図がそれを読んでいること**と、規則が字面で決まる 3 点 —
 * 接続モードの間はクリックで選択を変えない / 双方向の線は「線削除の確認」が OFF でも向きを選ばせる /
 * 右クリックで OS のメニューを止める。
 */
const map = readFileSync(new URL("../components/TopologyMap.vue", import.meta.url), "utf8");

/** `"name": (…) => { … },` の本体を、次のハンドラの鍵の手前まで切り出す。 */
function handler(name: string): string {
  const start = map.indexOf(`"${name}":`);
  expect(start, `${name} のハンドラが無い`).toBeGreaterThan(-1);
  const next = map.slice(start + 1).search(/\n {2}"[a-z]+:[a-z]+":/);
  return next === -1 ? map.slice(start) : map.slice(start, start + 1 + next);
}

/** `async function name(` の本体を、次の関数の手前まで切り出す。 */
function fn(name: string): string {
  const start = map.search(new RegExp(`(async )?function ${name}\\(`));
  expect(start, `${name} が無い`).toBeGreaterThan(-1);
  const next = map.slice(start + 1).search(/\n(async )?function |\nconst [a-zA-Z]+(: [A-Za-z<>]+)? = /);
  return next === -1 ? map.slice(start) : map.slice(start, start + 1 + next);
}

describe("地図の上で絆を張る", () => {
  it("接続モードの間はノードのクリックを行き先に使い、選択も判断役のダイアログも変えない", () => {
    const body = handler("node:click");
    const link = body.indexOf("if (linkSource.value)");
    expect(link).toBeGreaterThan(-1);
    expect(body.indexOf("finishLink(")).toBeGreaterThan(link);
    // 選択と判断役のダイアログは、接続モードの分岐（return）の後ろにだけある。
    expect(body.indexOf("orchestrator.select(")).toBeGreaterThan(body.indexOf("return;"));
    expect(body.indexOf("judgeDialog.open(")).toBeGreaterThan(body.indexOf("return;"));
  });

  it("右クリックは OS のメニューを止め、起点にできる個体でだけメニューを開く", () => {
    const body = handler("node:contextmenu");
    expect(body).toContain("event.preventDefault()");
    expect(body.indexOf("canStartLink(state.agents")).toBeLessThan(body.indexOf("openMenu("));
  });

  it("張る規則は drop と同じ tieAddition（判断役は行き先にだけなれる）", () => {
    const body = fn("finishLink");
    expect(body).toContain("tieAddition(state.agents, source, target, state.judges.map((j) => j.id))");
  });

  it("接続モードの間に線を押したら、切らずにモードを取り消す", () => {
    const body = handler("edge:click");
    expect(body.indexOf("cancelLink()")).toBeGreaterThan(-1);
    expect(body.indexOf("cancelLink()")).toBeLessThan(body.indexOf("removeEdge("));
  });

  it("メニューは画面全体の覆い（fixed inset-0）を作らない", () => {
    // 覆いを作ると「/」での入力欄への移動や Alt+↑↓ が覆いとして止まる（chatFocus の判定）。
    const start = map.indexOf('class="kizuna-menu');
    expect(start).toBeGreaterThan(-1);
    expect(map.slice(start - 200, start + 300)).not.toMatch(/fixed inset-0/);
  });
});

describe("双方向の線を向きを選んで切る", () => {
  it("双方向は askChoice で向きを選ばせ、「線削除の確認」の分岐はその後ろ（else if）", () => {
    const body = fn("removeEdge");
    const choice = body.indexOf("askChoice(");
    const confirm = body.indexOf("} else if (settings.confirmEdgeDelete)");
    expect(choice).toBeGreaterThan(-1);
    expect(confirm).toBeGreaterThan(choice);
    // 双方向の分岐が確認の設定に包まれていない。
    expect(body.slice(0, choice)).not.toContain("settings.confirmEdgeDelete");
  });

  it("切る向きは edgeCutPlan が決め、地図は接続先を自分で組み直さない", () => {
    const body = fn("removeEdge");
    expect(body).toContain("edgeCutPlan(state.agents, drawn, cut)");
    expect(map).not.toMatch(/connectedAgents\.filter\(/);
  });

  it("片方向の確認も同じ札の形（askChoice）で出し、はい／いいえの確認には戻さない", () => {
    const body = fn("removeEdge");
    const oneWay = body.slice(body.indexOf("} else if (settings.confirmEdgeDelete)"));
    expect(oneWay).toContain("askChoice(");
    expect(oneWay).toContain('key: "forward"');
    expect(map).not.toContain("askConfirm(");
  });

  it("3 つの選択肢の鍵は EdgeCut の 3 値と同じ", () => {
    const body = fn("removeEdge");
    for (const key of ["both", "forward", "backward"]) {
      expect(body).toContain(`key: "${key}"`);
    }
  });
});
