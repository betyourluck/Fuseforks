// @ts-expect-error @types/node を入れない方針のため（editorSaveWiring.test.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { unsavedSummary } from "./agentSettingsUnsaved";
import { CONFIG_FILE_LABELS, type ConfigFileKind } from "../types";

/**
 * 設定ダイアログを閉じるときの確認の配線（2026-10-19）。
 *
 * 左（設定フォーム）と右（`MarkdownEditor`）は保存が別々で、閉じるときの確認は
 * **左の未保存しか見ていなかった** — 右で SKILL.md だけを書き換えて ✕ を押すと、
 * 確認なしで閉じて編集が消えた。どちらの部品も単体では正しく、壊れていたのは
 * 2 つを束ねる親の配線なので、型検査にも lint にも掛からない。ここで留める。
 */
const dialog = readFileSync(
  new URL("../components/AgentSettingsDialog.vue", import.meta.url),
  "utf8",
);
const editor = readFileSync(new URL("../components/MarkdownEditor.vue", import.meta.url), "utf8");
const ja = JSON.parse(readFileSync(new URL("../locales/ja.json", import.meta.url), "utf8"));
const en = JSON.parse(readFileSync(new URL("../locales/en.json", import.meta.url), "utf8"));

/** `function name(` から次の行頭の `}` までを切り出す。 */
function body(source: string, name: string): string {
  const start = source.indexOf(`function ${name}(`);
  expect(start).toBeGreaterThan(-1);
  const end = source.indexOf("\n}\n", start);
  expect(end).toBeGreaterThan(start);
  return source.slice(start, end);
}

function lookup(dict: Record<string, unknown>, key: string): unknown {
  return key.split(".").reduce<unknown>(
    (node, part) => (node as Record<string, unknown> | undefined)?.[part],
    dict,
  );
}

describe("設定ダイアログを閉じるときの確認", () => {
  it("エディタは未保存のファイルの種別を親へ出す", () => {
    expect(editor).toMatch(/defineExpose\(\{\s*unsavedKind\s*\}\)/);
  });

  it("親はエディタへ ref を張り、左右の未保存を 1 つにまとめる", () => {
    expect(dialog).toMatch(/<MarkdownEditor\s+ref="editor"/);
    expect(dialog).toContain('from "../lib/agentSettingsUnsaved"');
    expect(dialog).toContain("unsavedSummary(dirty.value, editor.value?.unsavedKind ?? null)");
  });

  it("閉じるときの確認は左右の両方を見る（左だけの判定に戻さない）", () => {
    const close = body(dialog, "requestClose");
    expect(close).toContain("unsaved.value");
    expect(close).not.toMatch(/dirty\.value\s*&&/);
  });

  it("フッターの表示も同じまとめを読む", () => {
    expect(dialog).toContain("unsaved.noteKey");
    expect(dialog).not.toContain('$t("agentSettings.unsaved")');
  });

  it("まとめが返す辞書の鍵は ja / en の両方にある", () => {
    // 鍵を実行時に選ぶので、欠けても vue-tsc も i18n のコンパイル検査も通る。
    const kinds: (ConfigFileKind | null)[] = [null, ...(Object.keys(CONFIG_FILE_LABELS) as ConfigFileKind[])];
    for (const form of [false, true]) {
      for (const kind of kinds) {
        const summary = unsavedSummary(form, kind);
        if (summary === null) continue;
        for (const key of [summary.messageKey, summary.noteKey]) {
          expect(lookup(ja, key), `ja: ${key}`).toBeTypeOf("string");
          expect(lookup(en, key), `en: ${key}`).toBeTypeOf("string");
        }
      }
    }
  });
});
