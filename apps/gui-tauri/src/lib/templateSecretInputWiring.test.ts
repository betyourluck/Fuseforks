// @ts-expect-error @types/node を入れない方針のため（editorSaveWiring.test.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

/**
 * モデルテンプレート登録ダイアログの API キー入力欄（2026-10-19）。
 *
 * 入力欄 `secretInput` は下書き `draft` の欄ではなく、テンプレートの選択を切り替えても空にならなかった。
 * 「登録」（`saveSecret`）は**いま選んでいる下書きの id へ**鍵を保存するので、テンプレート A で打った鍵が
 * B を選んだ後も残り、そのまま押すと **B の鍵として保存された**（Spec 68 rev2 の起票の途中で見つかった。
 * Notes 5）。コンポーネントをマウントする土台が無いので、配線を走査で留める。
 */
const dialog = readFileSync(
  new URL("../components/ModelTemplateDialog.vue", import.meta.url),
  "utf8",
);

/** `function name(` から次の行頭の `}` までを切り出す。 */
function body(source: string, name: string): string {
  const start = source.indexOf(`function ${name}(`);
  expect(start).toBeGreaterThan(-1);
  const end = source.indexOf("\n}\n", start);
  expect(end).toBeGreaterThan(start);
  return source.slice(start, end);
}

describe("API キー入力欄の持ち越し", () => {
  it("登録はいま選んでいる下書きの id へ保存する（だから切り替えで空にする必要がある）", () => {
    const save = body(dialog, "saveSecret");
    expect(save).toContain("const template = draft.value;");
    expect(save).toContain("orchestrator.setCredential(id, secret)");
  });

  it("下書きの id が変わったら（選択の切り替え・新規・削除）入力欄を空にする", () => {
    // 判定を id の 1 点に置く。edit / create / remove / reseedDraft を 1 つずつ数えると、
    // 選択を変える経路が増えたときに漏れる。
    expect(dialog).toMatch(
      /watch\(\s*\(\)\s*=>\s*draft\.value\?\.id,\s*\(\)\s*=>\s*\{\s*secretInput\.value = "";\s*\}/,
    );
  });
});
