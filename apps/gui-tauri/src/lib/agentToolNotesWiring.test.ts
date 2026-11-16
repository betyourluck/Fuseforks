/**
 * サーヴァント設定の、ツール固有の注記の置き場（Spec 76 P2 の実機の指摘）。
 *
 * 注記を一覧の下にまとめて出すと、**末尾の行の説明に見える** — `image` を一覧の末尾に足した時点で、
 * `run` の注記が `image` の行の直下に来た。型にも lint にも掛からないので、字面で留める:
 * ツール固有の注記は一覧の `v-for` の中で、`tool.name` でその行に結び付けて出す。
 */
// @ts-expect-error @types/node を入れない方針のため（editorSaveWiring.test.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

/** コメントを落とす（走査テストはコメントも読むので、説明文の字面に当たらないように）。 */
const code = (src: string) =>
  src.replace(/<!--[\s\S]*?-->/g, "").replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");

const dialog = code(readFileSync(new URL("../components/AgentSettingsDialog.vue", import.meta.url), "utf8"));

const NOTES = [
  { tool: "remember", key: "agentSettings.rememberOffWarn" },
  { tool: "run", key: "agentSettings.runOnWarn" },
  { tool: "image", key: "agentSettings.imageInactiveWarn" },
] as const;

const loopStart = dialog.indexOf('<template v-for="tool in BUNDLED_TOOLS"');
const loopEnd = dialog.indexOf("</template>", loopStart);
const loop = dialog.slice(loopStart, loopEnd);
const outside = dialog.slice(0, loopStart) + dialog.slice(loopEnd);

describe("ツール固有の注記はその行の直下", () => {
  it("一覧は行ごとに label と注記を並べる v-for", () => {
    expect(loopStart).toBeGreaterThan(-1);
    expect(loopEnd).toBeGreaterThan(loopStart);
  });

  for (const { tool, key } of NOTES) {
    it(`${tool} の注記は ${tool} の行に結び付いている`, () => {
      expect(loop).toContain(`v-if="tool.name === '${tool}'`);
      expect(loop).toContain(`data-tool-note="${tool}"`);
      expect(loop).toContain(`$t("${key}")`);
      // 一覧の後ろにまとめて出す形へ戻さない。
      expect(outside).not.toContain(`$t("${key}")`);
    });
  }

  it("image の注記は画像生成が効いていないときだけ（判定は Rust の active）", () => {
    expect(loop).toContain(
      `v-if="tool.name === 'image' && isToolChecked('image') && imageGenActive === false"`,
    );
  });
});
