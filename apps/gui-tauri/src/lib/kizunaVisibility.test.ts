// @ts-expect-error @types/node を入れない方針のため（editorSaveWiring.test.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

/**
 * 絆の地図の Fit の配線（`failures.md` #122）。
 *
 * 真因はズームの clamp — `fitToContents` は内容が小さいと上限を超える zoom を要求し、
 * zoom だけが丸められて pan はそのまま残る。守るのは 3 点: (1) `fit()` は `fitMargin` の
 * 余白で要求 zoom を上限の内側に留める (2) 読み込み時の Fit もライブラリの
 * `"fit-content"` ではなく `fit()`（`view:load`）を通す (3) 地図へ渡す `layouts` は
 * 見えている個体だけ（`visibleLayouts`）で、可視集合が変わったら `:key` で作り直して
 * Fit し直す。どれも型検査には掛からず、実機で「左上へ寄る」としてしか出ない。
 */
const source = readFileSync(new URL("../components/TopologyMap.vue", import.meta.url), "utf8");
const lib = readFileSync(
  new URL("../../node_modules/v-network-graph/lib/index.js", import.meta.url),
  "utf8",
);

describe("絆の地図の可視集合と Fit", () => {
  it("layouts は visibleLayouts で絞る", () => {
    expect(source).toContain("visibleLayouts(");
    expect(source).not.toContain("seedPositions(");
  });

  it("可視集合が変わったら地図を作り直し、view:load で fit() を掛け直す（:key）", () => {
    expect(source).toMatch(/<VNetworkGraph[\s\S]*?:key="visibleKey"/);
    expect(source).toContain("autoPanAndZoomOnLoad: false");
    expect(source).toMatch(/"view:load": \(\) => \{\s*fit\(\);/);
  });

  it("fit() は fitMargin の余白で zoom を上限の内側に留める", () => {
    expect(source).toMatch(/fitMargin\([\s\S]*?MAX_ZOOM,/);
    expect(source).toContain("maxZoomLevel: MAX_ZOOM");
    expect(source).toMatch(/fitToContents\(\{ margin \}\)/);
    expect(source).not.toMatch(/fitToContents\(\)/);
  });

  it("ライブラリの zoom は setter で clamp される（消えたら余白の細工は要らなくなる）", () => {
    // fitToContents が上限を超えた zoom を要求すると、ここで zoom だけが丸められ pan は残る。
    // この形が変わったら fitMargin の根拠を読み直す。
    expect(lib).toMatch(/"zoomLevel", \w+, \(\w+\) => \(\w+ = Math\.max\(\w+, \w+\.view\.minZoomLevel\)/);
    // 内部の写しは今も足すだけ（:key が消した鍵を捨てる根拠）。
    expect(lib).toMatch(/Object\.assign\(\w+\.nodes, \(\w+ = \w+\.layouts\.nodes\)/);
  });
});
