/**
 * 辞書の規律。
 *
 * **鍵集合は全言語で一致させる。** 片方にだけある鍵は fallback（日本語）で
 * 表示されるため画面は壊れないが、「訳が漏れている」ことは画面から読めない —
 * ここで落として気づかせる。言語を足すとき（zh-Hans / de / fr 予定）も、
 * このテストに追加するだけで同じ網に入る。
 */
import { describe, expect, it } from "vitest";

import en from "../locales/en.json";
import ja from "../locales/ja.json";

/** 入れ子の辞書を "a.b.c" 形式の鍵一覧へ畳む。 */
function flattenKeys(node: unknown, prefix = ""): string[] {
  if (typeof node !== "object" || node === null) return [prefix];
  return Object.entries(node).flatMap(([key, value]) =>
    flattenKeys(value, prefix ? `${prefix}.${key}` : key),
  );
}

describe("翻訳辞書", () => {
  it("ja と en の鍵集合が一致する（訳の漏れ・余りを作らない）", () => {
    expect(flattenKeys(en).sort()).toEqual(flattenKeys(ja).sort());
  });

  it("すべての値が空文字ではない", () => {
    for (const dict of [ja, en]) {
      const walk = (node: unknown, path: string): void => {
        if (typeof node === "string") {
          expect(node.trim(), path).not.toBe("");
          return;
        }
        for (const [key, value] of Object.entries(node as Record<string, unknown>)) {
          walk(value, `${path}.${key}`);
        }
      };
      walk(dict, "");
    }
  });
});
