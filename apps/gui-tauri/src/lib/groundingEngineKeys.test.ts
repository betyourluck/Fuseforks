/**
 * 接地エンジンの辞書鍵を Rust 側と機械で突き合わせる（Spec 34 P3）。
 *
 * **`GroundingNote.vue` は鍵を実行時に組み立てる**（`grounding.engine.${engine}`）。
 * 組み立てた鍵が辞書に無いと、vue-i18n は**生の鍵をそのまま画面へ出す** —
 * 型検査にも lint にも掛からず、その engine が実際に返る接続先で走らせるまで
 * 分からない（`failures.md` #54 と同じ性質。あちらは実行時に組み立てる
 * `var(--color-role-${色})` が Tailwind に落とされていた）。
 *
 * **この網は Spec 34 P3 で実際に事故を捕まえて生まれた。**
 * コアの `rename_all = "snake_case"` は `OpenAi` を **`open_ai`** へ割るのに、
 * TS と辞書へ `openai` と書いていた。**ワイヤ値を確かめずに綴りを決めた**のが
 * 機序で、`Provider` が既に `open_ai_compat` / `open_ai_responses` を使っている
 * のに、そちらを見ていなかった。
 *
 * 手口は `toolLabel.test.ts` / `defaultEnabledTools.test.ts` と同じ。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { dirname, resolve } from "node:path";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

import ja from "../locales/ja.json";
import en from "../locales/en.json";

const here = dirname(fileURLToPath(import.meta.url));
const rustSource = readFileSync(
  resolve(here, "../../../../crates/agent-core/src/llm/canonical.rs"),
  "utf-8",
);

/**
 * `grounding_engine_wire_values_are_frozen` が凍結しているワイヤ値を読む。
 *
 * **enum の variant 名ではなくテストの期待値を読む。** variant 名から
 * `snake_case` を自前で再現すると、**その変換こそが今回間違えた当のもの**を
 * テスト側でもう一度実装することになる（`XHigh` → `xhigh` のような
 * `rename` の例外も再現できない）。凍結テストが serde の実出力と一致して
 * いることは Rust 側が保証しているので、そこを唯一の出所にする。
 */
function frozenEngineValues(): string[] {
  const block = rustSource.match(
    /fn grounding_engine_wire_values_are_frozen\(\) \{([\s\S]*?)\n    \}/,
  );
  if (!block) {
    throw new Error("Rust 側に grounding_engine_wire_values_are_frozen が見つかりません");
  }
  const values = [...block[1].matchAll(/r#""([a-z_]+)""#/g)].map((m) => m[1]);
  if (values.length === 0) throw new Error("凍結された engine の値が読めません");
  return values.sort();
}

describe("grounding.engine の辞書鍵", () => {
  // 計器の検定を先に取る（#90）— 走査が空振りしていないことを確かめてから
  // 「欠けは無い」を読む。0 件の結果は「起きなかった」と「読めていない」を畳む。
  it("Rust から 3 値以上を読めている（走査の検定）", () => {
    const values = frozenEngineValues();
    expect(values).toContain("google");
    expect(values).toContain("xai");
    expect(values.length).toBeGreaterThanOrEqual(3);
  });

  it("すべてのワイヤ値が ja と en の両方に鍵を持つ", () => {
    const values = frozenEngineValues();
    for (const [lang, dict] of [
      ["ja", ja],
      ["en", en],
    ] as const) {
      const keys = Object.keys(
        (dict as { grounding: { engine: Record<string, string> } }).grounding.engine,
      );
      const missing = values.filter((v) => !keys.includes(v));
      expect(missing, `${lang} に鍵が無い engine: ${missing.join(", ")}`).toEqual([]);
    }
  });

  // 逆向きも見る。辞書にだけある鍵は、**改名の取り残し**として現れる
  // （消えた engine の訳が残り、次に読む人が現役だと読む）。
  it("辞書に余った鍵が無い", () => {
    const values = frozenEngineValues();
    for (const [lang, dict] of [
      ["ja", ja],
      ["en", en],
    ] as const) {
      const keys = Object.keys(
        (dict as { grounding: { engine: Record<string, string> } }).grounding.engine,
      );
      const extra = keys.filter((k) => !values.includes(k));
      expect(extra, `${lang} に余った engine の鍵: ${extra.join(", ")}`).toEqual([]);
    }
  });
});
