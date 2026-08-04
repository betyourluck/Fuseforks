/**
 * 辞書の規律。
 *
 * **鍵集合は全言語で一致させる。** 片方にだけある鍵は fallback（日本語）で
 * 表示されるため画面は壊れないが、「訳が漏れている」ことは画面から読めない —
 * ここで落として気づかせる。言語を足すとき（zh-Hans / de / fr 予定）も、
 * このテストに追加するだけで同じ網に入る。
 */
import { describe, expect, it } from "vitest";
import { createI18n } from "vue-i18n";

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

/**
 * **すべてのメッセージが vue-i18n でコンパイルできること。**
 *
 * 鍵集合の一致だけでは、**本文がコンパイルできるか**を見ていない。
 * vue-i18n は `{` を補間の開始として解釈するので、JSON の例を辞書へ書くと
 * `Invalid token in placeholder` で**画面が丸ごと描けなくなる**
 * （2026-08-04、`command.json` のプレースホルダで実機発生）。
 * リテラルの波括弧は `{'{'}` と書く必要がある。
 *
 * ビルドも既存のテストも通ってしまい、**開いた人にしか分からない**形で出た。
 */
describe("メッセージのコンパイル", () => {
  it("全言語の全メッセージがコンパイルできる", () => {
    const walk = (node: unknown, path: string[], out: string[][]): void => {
      if (typeof node === "string") {
        out.push(path);
        return;
      }
      if (node && typeof node === "object") {
        for (const [key, value] of Object.entries(node)) {
          walk(value, [...path, key], out);
        }
      }
    };

    for (const locale of ["ja", "en"] as const) {
      const messages = locale === "ja" ? ja : en;
      const keys: string[][] = [];
      walk(messages, [], keys);

      const i18n = createI18n({
        legacy: false,
        locale,
        messages: { ja, en },
      });

      for (const path of keys) {
        const key = path.join(".");
        expect(
          () => i18n.global.t(key),
          `${locale}: ${key} がコンパイルできること（リテラルの波括弧は {'{'} と書く）`,
        ).not.toThrow();
      }
    }
  });
});
