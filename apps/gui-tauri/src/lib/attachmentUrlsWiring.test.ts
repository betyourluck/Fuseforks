/**
 * 会話ペインの添付の object URL の配線（#20a / N12）。部品をマウントする土台が無いので字面で留める。
 * 規則（何を読むか・いつ解放するか）は `attachmentUrls.test.ts` の単体。
 *
 * 留めているのは、外れても型検査に掛からず**黙って溜まる 2 つ**:
 *
 * 1. **会話を切り替えたら解放する**（#20a）— 会話ペインは `App.vue` の `v-show` の中でアプリの寿命まで
 *    unmount されず、解放しないと見た会話の数だけ画像の Blob が溜まる。切り替えの合図は
 *    `conversationCleared`（会話ログの投影が空の配列に差し替わる）。大きく見ている画像は残す
 * 2. **画像以外は本体を読まない**（N12）— 音声・動画・PDF はファイル名を出すだけで、本体（最大 12 MB）を
 *    `readAttachment`（数値の配列の JSON）で読んで object URL を持ち続ける理由が無い
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";

/** コメントを落とす（走査テストはコメントも読むので、説明文の字面に当たらないように）。 */
const code = (src: string) =>
  src.replace(/<!--[\s\S]*?-->/g, "").replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");

const panel = code(readFileSync(new URL("../components/ChatPanel.vue", import.meta.url), "utf8"));

describe("会話を切り替えたら添付の object URL を解放する（#20a）", () => {
  it("解放の口がある（URL.revokeObjectURL を呼ぶ）", () => {
    expect(panel).toContain("revoke: (url) => URL.revokeObjectURL(url)");
  });

  it("会話ログの投影が空に差し替わったら、大きく見ている画像を残してまとめて解放する", () => {
    expect(panel).toMatch(
      /watch\(\s*\(\) => state\.messages,\s*\(messages\) => \{\s*if \(messages\.length === 0\) attachmentCache\.releaseAll\(preview\.value\?\.src \?\? null\);\s*\},?\s*\)/,
    );
  });

  it("描画は投影キャッシュの 1 実装を通る（会話ペインの中で URL を直に作らない）", () => {
    expect(panel).toContain("return attachmentCache.url(att);");
    expect(panel.split("URL.createObjectURL(").length - 1).toBe(1);
  });
});

describe("画像以外は本体を読まない（N12）", () => {
  /** 添付 1 件を描くテンプレートの塊。 */
  const block = (() => {
    const start = panel.indexOf('<template v-for="att in entry.row.message.attachments ?? []"');
    expect(start, "添付を描く塊が在ること").toBeGreaterThanOrEqual(0);
    return panel.slice(start, panel.indexOf("</template>", start));
  })();

  it("音声・動画・PDF のファイル名の行は、本体の有無を見ない", () => {
    expect(block).toContain('v-else-if="!isImageAttachment(att)"');
    expect(block).not.toContain("!isImageAttachment(att) && attachmentUrl(att)");
  });

  it("「入力欄へ」は画像だけ本体の有無を見る（画像以外は押したときに入力欄が確かめる）", () => {
    expect(block).toContain(
      'v-if="canReattachFrom(entry.row.message) && (!isImageAttachment(att) || attachmentUrl(att))"',
    );
  });

  it("塊の中の attachmentUrl(att) は、どれも画像と分かった後でしか呼ばれない", () => {
    // 条件は v-if → v-else-if の順に評価される。画像以外は 1 つ目（isImageAttachment で短絡）と
    // 2 つ目（!isImageAttachment）で止まり、3 つ目の `=== null` には届かない。
    const conditions = [...block.matchAll(/v-(?:else-)?if="([^"]*)"/g)].map((m) => m[1]);
    for (const condition of conditions) {
      if (!condition.includes("attachmentUrl(att)")) continue;
      const guarded =
        condition.startsWith("isImageAttachment(att) && ") ||
        condition.includes("(!isImageAttachment(att) || attachmentUrl(att))") ||
        condition === "attachmentUrl(att) === null";
      expect(guarded, condition).toBe(true);
    }
    // 3 つ目（削除済み）の前に、画像以外を受ける枝がある。
    const fileRow = block.indexOf('v-else-if="!isImageAttachment(att)"');
    const deleted = block.indexOf('v-else-if="attachmentUrl(att) === null"');
    expect(fileRow).toBeGreaterThanOrEqual(0);
    expect(deleted).toBeGreaterThan(fileRow);
  });
});
