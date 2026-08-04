/**
 * 同梱ツールの名前表が Rust 側と食い違っていないことを機械で留める（Spec 15 P4）。
 *
 * `AgentSettingsDialog.vue` の `BUNDLED_TOOLS` / `DEFAULT_ENABLED_TOOLS` は
 * Rust の `tools/mod.rs` と**手動同期の契約**で、どちらも文字列の並びでしかない。
 * ずれてもコンパイラにも lint にも引っかからず、**画面のチェックが実際の提示と
 * 食い違う**という読みにくい形で出る（`failures.md` #51 と同じ性質 — 記号や
 * 文字列の追従漏れは機械の網の外）。
 *
 * 特に `run` は**既定集合の外**に居る必要がある。ここが揃っていないと、
 * 画面では ON に見えるのに提示されない（またはその逆）になり、
 * 「更新しただけで実行能力が増えない」という約束が画面から読めなくなる。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { dirname, resolve } from "node:path";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

/** このファイルの位置。`__dirname` は ESM に無いので URL から起こす。 */
const here = dirname(fileURLToPath(import.meta.url));

const rustSource = readFileSync(
  resolve(here, "../../../../crates/agent-core/src/tools/mod.rs"),
  "utf-8",
);
const vueSource = readFileSync(
  resolve(here, "../components/AgentSettingsDialog.vue"),
  "utf-8",
);

/** `pub const NAME: [&str; N] = ["a", "b"];` から名前の集合を取る。 */
function rustList(name: string): string[] {
  const match = rustSource.match(new RegExp(`${name}:\\s*\\[&str;\\s*\\d+\\]\\s*=\\s*([^;]+);`));
  if (!match) throw new Error(`Rust 側に ${name} が見つかりません`);
  return [...match[1].matchAll(/"([a-z_]+)"/g)].map((m) => m[1]).sort();
}

/** Vue 側の `DEFAULT_ENABLED_TOOLS` の配列リテラルから名前を取る。 */
function vueDefaults(): string[] {
  const match = vueSource.match(/DEFAULT_ENABLED_TOOLS:\s*readonly string\[\]\s*=\s*\[([^\]]+)\]/);
  if (!match) throw new Error("Vue 側に DEFAULT_ENABLED_TOOLS が見つかりません");
  return [...match[1].matchAll(/"([a-z_]+)"/g)].map((m) => m[1]).sort();
}

/** Vue 側の `BUNDLED_TOOLS` の `name:` から名前を取る。 */
function vueBundled(): string[] {
  const block = vueSource.match(/const BUNDLED_TOOLS = \[([\s\S]*?)\] as const;/);
  if (!block) throw new Error("Vue 側に BUNDLED_TOOLS が見つかりません");
  return [...block[1].matchAll(/name:\s*"([a-z_]+)"/g)].map((m) => m[1]).sort();
}

describe("同梱ツールの名前表", () => {
  it("BUNDLED_TOOL_NAMES が Rust と Vue で一致する", () => {
    expect(vueBundled()).toEqual(rustList("BUNDLED_TOOL_NAMES"));
  });

  it("DEFAULT_ENABLED_TOOLS が Rust と Vue で一致する", () => {
    expect(vueDefaults()).toEqual(rustList("DEFAULT_ENABLED_TOOLS"));
  });

  it("run は既定集合の外に居る（更新しただけで実行能力が増えない）", () => {
    expect(rustList("DEFAULT_ENABLED_TOOLS")).not.toContain("run");
    expect(vueDefaults()).not.toContain("run");
    expect(rustList("BUNDLED_TOOL_NAMES")).toContain("run");
  });

  it("既定集合は同梱ツールの部分集合である", () => {
    const bundled = new Set(rustList("BUNDLED_TOOL_NAMES"));
    for (const name of rustList("DEFAULT_ENABLED_TOOLS")) {
      expect(bundled.has(name)).toBe(true);
    }
  });
});
