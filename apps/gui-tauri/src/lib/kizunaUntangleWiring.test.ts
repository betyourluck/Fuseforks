/**
 * 地図の「重なりをほどく」の**配線**を機械で留める（2026-10-27）。
 *
 * 離し方は `kizunaUntangle.test.ts` の単体が守る。ここで留めるのは純関数の外 —
 * **保存はまとめての 1 回**（1 体ずつ書くと書き込みと地図の取り直しが体数ぶん走り、
 * ほどいている途中の配置が出る。型検査にも掛からない）/ 重なりが無い間は押せない /
 * IPC の名前が Rust の登録に在ること（ずれると押しても保存されない = 静かに壊れる）。
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

const map = code(read("../components/TopologyMap.vue"));
const ipc = read("./ipc.ts");
const libRs = read("../../src-tauri/src/lib.rs");

describe("重なりをほどく", () => {
  it("動いた個体をまとめて 1 回で保存する（1 体ずつの保存を呼ばない）", () => {
    const body = between(map, "async function untangleNodes", "\n}");
    expect(body).toContain("untangle(layouts.nodes, UNTANGLE_GAP)");
    expect(body).toContain("orchestrator.setTopologyPositions(");
    expect(body).not.toContain("setTopologyPosition(");
  });

  it("ボタンは重なりが無い間 disabled で、押すと untangleNodes を呼ぶ", () => {
    const button = between(map, "data-kizuna-untangle", "</button>");
    expect(button).toContain(':disabled="!canUntangle"');
    expect(button).toContain('@click="untangleNodes"');
    expect(map).toContain("hasOverlap(layouts.nodes, UNTANGLE_GAP)");
  });

  it("IPC の名前が Rust の generate_handler に登録されている", () => {
    expect(ipc).toContain('"set_topology_positions"');
    expect(libRs).toContain("commands::set_topology_positions,");
  });
});
