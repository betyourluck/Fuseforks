/**
 * 画像変換の worker が `ImageBitmap` を必ず解放する（Spec 77 #20c）。
 *
 * worker は使い回すので（`attachment.ts` の `ensureWorker`）、`close()` の前で例外が出た
 * ビットマップは GC まで残る。`OffscreenCanvas` も `createImageBitmap` もテストの環境に無いので、
 * 配線はソースの字面で留める。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

const source = readFileSync(
  fileURLToPath(new URL("../workers/imageConvert.ts", import.meta.url)),
  "utf8",
)
  .replace(/\/\*[\s\S]*?\*\//g, "")
  .replace(/^\s*\/\/.*$/gm, "");

describe("imageConvert", () => {
  it("bitmap.close() は finally の中で 1 回だけ呼ぶ", () => {
    expect(source.match(/bitmap\.close\(\)/g) ?? []).toHaveLength(1);
    expect(source).toMatch(/finally\s*\{\s*bitmap\.close\(\);\s*\}/);
  });
});
