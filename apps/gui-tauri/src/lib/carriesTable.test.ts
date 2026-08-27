/**
 * carries の表が Rust と TS で食い違っていないことを機械で留める（Spec 36 D12）。
 *
 * 画面の警告（`lib/carries.ts`）は**コアの門の写し**で、判定の本体は
 * `Provider::carries`。写しである以上ずれうるが、ずれてもコンパイラにも
 * lint にも引っかからない — 出るのは「警告が出ないまま送信が拒否される」
 * （またはその逆）という読みにくい形（`failures.md` #51 と同じ性質）。
 *
 * **読む先は `tests/carries_table.rs` の逐語凍結表**（P0 の probe 18 発の観測値）。
 * `Provider::carries` の `match` を直接パースしない — アームの書き方に依存する
 * 脆い網になるうえ、**凍結表のほうが「観測で決めた」という根拠に近い**。
 * 鎖は TS ← 凍結表 → `Provider::carries` → adapter で、
 * 後半は Rust 側の `adapters_match_the_carries_table` が留めている。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { dirname, resolve } from "node:path";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

import { KIND_ORDER, carries, carriersOf, effectiveProvider } from "./carries";
import type { Provider } from "../types";

const here = dirname(fileURLToPath(import.meta.url));
const rustSource = readFileSync(
  resolve(here, "../../../../crates/fuseforks-core/tests/carries_table.rs"),
  "utf-8",
);

/** Rust の variant 名 → `world.json` / TS の綴り（serde の snake_case）。 */
const VARIANT_TO_PROVIDER: Record<string, Provider> = {
  OpenAiCompat: "open_ai_compat",
  Anthropic: "anthropic",
  Gemini: "gemini",
  XaiResponses: "xai_responses",
  OpenAiResponses: "open_ai_responses",
  MetaResponses: "meta_responses",
  PerplexityResponses: "perplexity_responses",
};

/**
 * 凍結表を `(Provider::X, bool, bool, bool, bool)` の行から起こす。
 *
 * 並びは `[image, audio, video, pdf]`（Rust 側のコメントで固定されている）。
 */
function rustTable(): Record<string, boolean[]> {
  const rows = [
    ...rustSource.matchAll(
      /\(Provider::(\w+),\s*(true|false),\s*(true|false),\s*(true|false),\s*(true|false)\)/g,
    ),
  ];
  if (rows.length === 0) throw new Error("Rust 側の凍結表が読めません");
  return Object.fromEntries(
    rows.map((m) => [
      VARIANT_TO_PROVIDER[m[1]] ?? m[1],
      [m[2] === "true", m[3] === "true", m[4] === "true", m[5] === "true"],
    ]),
  );
}

describe("carries の表", () => {
  it("全 28 マスが Rust の凍結表と一致する", () => {
    const rust = rustTable();
    expect(Object.keys(rust).length).toBe(7);
    let checked = 0;
    for (const [provider, flags] of Object.entries(rust)) {
      KIND_ORDER.forEach((kind, index) => {
        expect(
          carries(provider as Provider, kind),
          `${provider} × ${kind}`,
        ).toBe(flags[index]);
        checked += 1;
      });
    }
    expect(checked).toBe(28);
  });

  it("画像はどのワイヤでも運べる（Spec 36 D9 の回収）", () => {
    // 「ネイティブを選ぶと画像が黙って落ちる」3 例が解消したことを、
    // 画面側の表からも読めるようにする。
    expect(carriersOf("image").length).toBe(7);
  });

  it("動画は Gemini と Meta のネイティブだけ（Spec 37 で 2 本目）", () => {
    // Meta の動画は **予測を覆して通った** — openai_responses からの類推で
    // ✗ と書くところを、payload 無しの `input_video` の名指し 400 が実在を教えた。
    expect(carriersOf("video")).toEqual(["gemini", "meta_responses"]);
  });

  it("実効ワイヤは未設定なら base URL から推定する（コアと同じ規則）", () => {
    expect(effectiveProvider(null, "https://api.anthropic.com/v1")).toBe(
      "anthropic",
    );
    expect(effectiveProvider(null, "https://api.openai.com/v1")).toBe(
      "open_ai_compat",
    );
    // 明示設定は base URL より強い（コアの `effective_provider` と同じ順序）。
    expect(effectiveProvider("gemini", "https://api.anthropic.com/v1")).toBe(
      "gemini",
    );
  });
});
