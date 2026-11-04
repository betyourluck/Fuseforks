/**
 * 表示の言語（Spec 74）の 3 値が、5 か所で同じであることを機械で留める。
 *
 * 値は Rust の `world::DisplayLanguage` の serde 値 / `types.ts` の `DisplayLanguage` /
 * 設定ページの言語の選択肢 / i18n の `messages` / 簡体字のフォントの規則に、それぞれ手で
 * 書いてある。どれか 1 つが欠けても型検査も lint も通る — 選択肢が欠ければ中文へ切り替えられず、
 * 辞書が欠ければ黙って日本語で出て、フォントの規則が欠ければ漢字が日本の字形で描かれる。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { dirname, resolve } from "node:path";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

import { i18n } from "../i18n";

const here = dirname(fileURLToPath(import.meta.url));
const read = (path: string): string => readFileSync(resolve(here, path), "utf-8");

const rustWorld = read("../../../../crates/fuseforks-core/src/world.rs");
const types = read("../types.ts");
const settings = read("../components/SettingsDialog.vue");
const style = read("../style.css");

/** Rust の `DisplayLanguage` の serde rename を、宣言の並びで集める。 */
function rustWireValues(): string[] {
  const start = rustWorld.indexOf("pub enum DisplayLanguage {");
  const body = rustWorld.slice(start, rustWorld.indexOf("\n}", start));
  return [...body.matchAll(/#\[serde\(rename = "([^"]+)"\)\]/g)].map((m) => m[1]);
}

describe("表示の言語の 3 値", () => {
  it("走査が空振りしていない（Rust の宣言から 3 値を読めた）", () => {
    expect(rustWireValues()).toEqual(["ja", "en", "zh-Hans"]);
  });

  it("types.ts の DisplayLanguage が Rust の serde 値と同じ", () => {
    const m = types.match(/export type DisplayLanguage = ([^;]+);/);
    expect(m, "DisplayLanguage の宣言が見つからない").not.toBeNull();
    const values = [...m![1].matchAll(/"([^"]+)"/g)].map((x) => x[1]);
    expect(values).toEqual(rustWireValues());
  });

  it("設定ページの言語の選択肢が 3 値すべてを持つ", () => {
    const radios = [...settings.matchAll(/v-model="languageInput" type="radio" value="([^"]+)"/g)].map(
      (m) => m[1],
    );
    expect(radios).toEqual(rustWireValues());
  });

  it("i18n の辞書が 3 値すべてを持つ", () => {
    expect(Object.keys(i18n.global.messages.value).sort()).toEqual([...rustWireValues()].sort());
  });

  it("简体中文のときだけ SC のフォントを Noto Sans JP より先に立てる", () => {
    const m = style.match(/html:lang\(zh-Hans\) body \{([^}]*)\}/);
    expect(m, "html:lang(zh-Hans) body の規則が無い").not.toBeNull();
    const family = m![1];
    expect(family).toContain('"Noto Sans SC"');
    expect(family).toContain('"Microsoft YaHei UI"');
    expect(family).toContain('"PingFang SC"');
    expect(family).not.toContain("Noto Sans JP");
  });
});
