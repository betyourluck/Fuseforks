/**
 * ツール名の表示（2026-08-08）。
 *
 * **同梱ツールの表を Rust 側と機械で突き合わせる。** ここが抜けると、
 * **自分たちのツールが「外部のもの」として等幅の識別子で出る** —
 * 型検査にも lint にも掛からず、画面を見るまで分からない
 * （`defaultEnabledTools.test.ts` と同じ性質・同じ手口）。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { dirname, resolve } from "node:path";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

import { KNOWN_TOOL_NAMES, toolLabel } from "./toolLabel";
import ja from "../locales/ja.json";
import en from "../locales/en.json";

const here = dirname(fileURLToPath(import.meta.url));
const rustSource = readFileSync(
  resolve(here, "../../../../crates/agent-core/src/tools/mod.rs"),
  "utf-8",
);

/** `pub const NAME: [&str; N] = ["a", "b"];` から名前の集合を取る。 */
function rustList(name: string): string[] {
  const match = rustSource.match(new RegExp(`${name}:\\s*\\[&str;\\s*\\d+\\]\\s*=\\s*([^;]+);`));
  if (!match) throw new Error(`Rust 側に ${name} が見つかりません`);
  return [...match[1].matchAll(/"([a-z_]+)"/g)].map((m) => m[1]).sort();
}

describe("toolLabel", () => {
  it("同梱ツールはすべて表に載っている（Rust と突き合わせ）", () => {
    const bundled = rustList("BUNDLED_TOOL_NAMES");
    const missing = bundled.filter((name) => !KNOWN_TOOL_NAMES.includes(name));
    expect(missing, `表に無い同梱ツール: ${missing.join(", ")}`).toEqual([]);
  });

  it("`rag` も表に載っている（BUNDLED の外に居るがこの村のツール）", () => {
    // `rag` は enabledTools の対象外という点で MCP 由来と同じ棚に住むが、
    // **名付けたのはこちら**。取り違えると「外部ツール」として出る。
    expect(KNOWN_TOOL_NAMES).toContain("rag");
  });

  it("表の全項目に ja / en の訳がある", () => {
    const keys = [...KNOWN_TOOL_NAMES, "ask", "transfer"];
    for (const key of keys) {
      expect(ja.tools, `ja に ${key} が無い`).toHaveProperty(key);
      expect(en.tools, `en に ${key} が無い`).toHaveProperty(key);
    }
  });

  it("同梱ツールは辞書の鍵を返す", () => {
    expect(toolLabel("room_log", (id) => id)).toEqual({
      kind: "known",
      key: "tools.room_log",
    });
  });

  it("委譲は宛先を表示名へ解く", () => {
    const nameOf = (id: string) => (id === "agent_3" ? "ジェミー" : id);
    expect(toolLabel("ask_agent_3", nameOf)).toEqual({
      kind: "known",
      key: "tools.ask",
      target: "ジェミー",
    });
    expect(toolLabel("transfer_to_agent_3", nameOf)).toEqual({
      kind: "known",
      key: "tools.transfer",
      target: "ジェミー",
    });
  });

  it("引けない宛先は id のまま出す（行ごと消さない）", () => {
    // 消えたサーヴァントへの委譲でも、実行された事実は残す。
    expect(toolLabel("ask_agent_99", (id) => id)).toEqual({
      kind: "known",
      key: "tools.ask",
      target: "agent_99",
    });
  });

  it("外部（MCP）は訳さず識別子のまま返す", () => {
    // **名付けたのは接続先。** 訳語を当てると何が走ったかについて嘘になる。
    expect(toolLabel("MCP_DOCKER__fetch", (id) => id)).toEqual({
      kind: "external",
      id: "MCP_DOCKER__fetch",
    });
  });

  it("外部と同梱を対で見る", () => {
    // 片方だけを見ると、全部 known を返す実装でも全部 external を返す実装でも通る。
    expect(toolLabel("grep", (id) => id).kind).toBe("known");
    expect(toolLabel("manuale__search", (id) => id).kind).toBe("external");
  });
});
