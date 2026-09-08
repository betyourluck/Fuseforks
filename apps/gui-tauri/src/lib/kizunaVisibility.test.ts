// @ts-expect-error @types/node を入れない方針のため（editorSaveWiring.test.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

/**
 * 絆の地図が**隠れた個体の座標を Fit に混ぜない**配線（`failures.md` #122）。
 *
 * 2 段で守る — (1) 地図へ渡す `layouts` は見えている個体だけ（`visibleLayouts`）
 * (2) 可視集合が変わったら部品を作り直す（`:key`）。(1) だけでは足りない:
 * v-network-graph は `layouts` を内部の写しへ `Object.assign` で足すだけで、消した鍵が
 * 写しに残る。どちらも型検査には掛からず、実機で「左上へ寄る」としてしか出ない。
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

  it("可視集合が変わったら地図を作り直す（:key）", () => {
    expect(source).toMatch(/<VNetworkGraph[\s\S]*?:key="visibleKey"/);
    expect(source).toContain('autoPanAndZoomOnLoad: "fit-content"');
  });

  it("ライブラリの内部写しは今も足すだけ（消えたら :key は要らなくなる）", () => {
    // ここが replace に変わったら、:key の根拠が消える — そのとき判断し直す。
    expect(lib).toMatch(/Object\.assign\(\w+\.nodes, \(\w+ = \w+\.layouts\.nodes\)/);
  });
});
