// @ts-expect-error @types/node を入れない方針のため（editorSaveWiring.test.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { JUDGE_EDGE_PREFIX, JUDGE_MARKER_COLOR, edgeKey } from "./kizunaEdges";

/**
 * 判断役に触れる辺の色分け（2026-11-08）の配線。
 *
 * 色は CSS が塗り、どの辺を塗るかは**字面**で決まる — 辺の鍵の頭（`data-edge-id`）と、
 * 矢印の id に埋め込まれる色の base64。どちらも型検査にも lint にも掛からず、
 * 片方だけ変えると**黙って元の色に戻る**ので、ここで突き合わせる。
 */
const map: string = readFileSync(
  new URL("../components/TopologyMap.vue", import.meta.url),
  "utf8",
);
const style = map.slice(map.indexOf("<style"));
const script = map.slice(0, map.indexOf("<template"));

/** 描画ライブラリの実物。辺の `data-edge-id` と矢印の id の作り方に寄りかかっているので読む。 */
const lib: string = readFileSync(
  new URL("../../node_modules/v-network-graph/lib/index.js", import.meta.url),
  "utf8",
);

/** ライブラリと同じ符号化（`=` を落とした base64）。 */
const markerFragment = btoa(JUDGE_MARKER_COLOR).replace(/=/g, "");

describe("判断役に触れる辺の鍵", () => {
  it("判断役に触れる辺だけ鍵の頭に印が付く", () => {
    expect(edgeKey("agent->agent_2", false)).toBe("agent->agent_2");
    expect(edgeKey("agent->agent_9", true)).toBe(`${JUDGE_EDGE_PREFIX}agent->agent_9`);
  });

  it("地図は両方向の辺の鍵を edgeKey で作る", () => {
    // サーヴァント → 判断役（人が引いた絆）は端点が判断役かで決める。
    expect(script).toMatch(/const touchesJudge = isJudge\(edge\.source\) \|\| isJudge\(edge\.target\);/);
    expect(script).toMatch(/const id = edgeKey\(/);
    // 判断役 → サーヴァント（judge.toml の写し）は常に印付き。
    expect(script).toMatch(/result\[edgeKey\(`\$\{edge\.from\}->\$\{edge\.to\}`, true\)\]/);
    // 鍵を手で組んだ `judge:` が残っていない（定数と二重に書かない）。
    expect(script).not.toContain("`judge:");
  });
});

describe("判断役に触れる辺の色", () => {
  it("線は鍵の頭で選んで --color-judge で塗る", () => {
    expect(style).toContain(
      `.kizuna :deep(.v-ng-edge[data-edge-id^="${JUDGE_EDGE_PREFIX}"]) {\n  stroke: var(--color-judge);`,
    );
  });

  it("矢印は JUDGE_MARKER_COLOR の base64 を id に持つものだけ --color-judge で塗る", () => {
    expect(script).toContain("color: ([edge]) => (edge.touchesJudge ? JUDGE_MARKER_COLOR : null)");
    expect(style).toContain(
      `.kizuna :deep(.v-ng-marker[id*="${markerFragment}"] polygon) {\n  fill: var(--color-judge);`,
    );
    // 普通の辺の矢印も線と同じ accent（ライブラリの既定 #4466cc のまま残さない）。
    expect(style).toContain(".kizuna :deep(.v-ng-marker polygon) {\n  fill: var(--color-accent);");
  });

  it("描画ライブラリは直線の辺に data-edge-id を付け、矢印の id に色の base64 を埋める", () => {
    // ライブラリを上げてこの 2 つが変わると、CSS の選択子が何にも当たらなくなる。
    expect(lib).toContain('"data-edge-id": t.id');
    expect(lib).toMatch(/return `marker_\$\{e\}_\$\{t\.type\}_[^`]*\$\{n\}[^`]*`/);
    expect(lib).toMatch(/btoa\(t\)\.replaceAll\("=", ""\)/);
  });
});
