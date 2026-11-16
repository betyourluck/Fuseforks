/**
 * 会話の画像を大きく見る（Spec 76 P2 の実機の要望）の配線。部品をマウントする土台が無いので字面で留める。
 *
 * 留めているのは、外れても型検査に掛からず**黙って崩れる 3 つ**:
 *
 * 1. **`body` へ出す** — 会話ペインの根は container query のコンテナで、その中の `fixed` の層は
 *    会話ペインの枠に閉じ込められる（窓いっぱいにならない）
 * 2. **Esc をここで止める** — 止めないと、窓で聴いている他の Esc（会話の検索など）にも届く
 * 3. **会話の画像を押すと開く**（利用者の画像も生成画像の写しも同じ）。URL は投影キャッシュのものを渡す
 */
// @ts-expect-error @types/node を入れない方針のため（editorSaveWiring.test.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

/** コメントを落とす（走査テストはコメントも読むので、説明文の字面に当たらないように）。 */
const code = (src: string) =>
  src.replace(/<!--[\s\S]*?-->/g, "").replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");

const preview = code(readFileSync(new URL("../components/ImagePreview.vue", import.meta.url), "utf8"));
const panel = code(readFileSync(new URL("../components/ChatPanel.vue", import.meta.url), "utf8"));

describe("プレビューの層", () => {
  it("body へ出して窓いっぱいに描く", () => {
    expect(preview).toContain('<Teleport to="body">');
    expect(preview).toContain("fixed inset-0");
  });

  it("覆いの印を持つ（「/」と Ctrl+F が覆いの下で入力欄を奪わない）", () => {
    expect(preview).toContain("bg-scrim");
    expect(preview).toContain('aria-modal="true"');
  });

  it("背景のクリックと Esc で閉じ、Esc は外へ届かせない", () => {
    expect(preview).toContain("@click.self=\"emit('close')\"");
    expect(preview).toContain("@keydown.esc.stop.prevent=\"emit('close')\"");
  });
});

describe("会話の画像を押すと開く", () => {
  it("画像は押せるボタンに包み、押すと openPreview", () => {
    expect(panel).toMatch(
      /<button\s+v-if="isImageAttachment\(att\) && attachmentUrl\(att\)"[\s\S]*?@click="openPreview\(att\)"[\s\S]*?<img/,
    );
    expect(panel).toContain('<ImagePreview\n      v-if="preview"');
  });

  it("開くときに読み直さない（投影キャッシュの URL を渡す）", () => {
    const start = panel.indexOf("function openPreview(");
    const end = panel.indexOf("\n}\n", start);
    const body = panel.slice(start, end);
    expect(body).toContain("attachmentUrl(att)");
    expect(body).not.toContain("readAttachment");
  });
});
