/**
 * 新しく作るテンプレートとサーヴァントの既定値が、コアと画面で同じ（2026-11-21）。
 *
 * 既定値は 3 か所に住む — コアの `ModelTemplate::new` / `default_timeout_secs` /
 * `OrchestratorConfig::default`、画面のモデル登録の下書き（`ModelTemplateDialog.vue` の `blank()`）、
 * サーヴァント設定の空欄の表示（辞書 `agentSettings.maxToolsPlaceholder`）。どれも型検査に掛からず、
 * 片方だけ直すと**画面が嘘の既定を見せる**（実際、ツール実行の上限をコアで 36 に上げた時点で、
 * 空欄の表示は「既定: 12」のまま残っていた）。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

import en from "../locales/en.json";
import ja from "../locales/ja.json";
import zhHans from "../locales/zh-Hans.json";

const read = (path: string) =>
  readFileSync(fileURLToPath(new URL(`../../${path}`, import.meta.url)), "utf8");

/** 正規表現の 1 つ目の捕獲を数として読む。`32_000` のような区切りは落とす。 */
function number(src: string, pattern: RegExp, what: string): number {
  const found = pattern.exec(src);
  expect(found, `${what} が見つからない`).not.toBeNull();
  return Number(found![1].replace(/_/g, ""));
}

const model = read("../../crates/fuseforks-core/src/model.rs");
const orchestrator = read("../../crates/fuseforks-core/src/orchestrator/mod.rs");
const dialog = read("src/components/ModelTemplateDialog.vue");

const coreMaxOutput = number(model, /\n\s*max_output_tokens: ([\d_]+),/, "ModelTemplate::new の max_output_tokens");
const coreTimeout = number(model, /fn default_timeout_secs\(\) -> u32 \{\s*(\d+)\s*\}/, "default_timeout_secs");
const coreToolRounds = number(
  orchestrator,
  /impl Default for OrchestratorConfig[\s\S]*?max_tool_iterations: (\d+),/,
  "OrchestratorConfig::default の max_tool_iterations",
);

describe("モデル登録の新しいテンプレート", () => {
  it("最大出力トークンがコアと同じ", () => {
    expect(number(dialog, /\n\s*maxOutputTokens: (\d+),/, "blank() の maxOutputTokens")).toBe(coreMaxOutput);
  });

  it("タイムアウトがコアと同じ", () => {
    expect(number(dialog, /\n\s*requestTimeoutSecs: (\d+),/, "blank() の requestTimeoutSecs")).toBe(coreTimeout);
  });
});

describe("サーヴァント設定の空欄の表示", () => {
  for (const [locale, dict] of [
    ["ja", ja],
    ["en", en],
    ["zh-Hans", zhHans],
  ] as const) {
    it(`${locale}: ツール実行の上限の既定がコアと同じ`, () => {
      const placeholder = dict.agentSettings.maxToolsPlaceholder;
      expect(placeholder.match(/\d+/g), placeholder).toEqual([String(coreToolRounds)]);
    });
  }
});
