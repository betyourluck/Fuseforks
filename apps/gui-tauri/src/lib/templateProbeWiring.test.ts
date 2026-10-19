// @ts-expect-error @types/node を入れない方針のため（editorSaveWiring.test.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import { PROBE_FAILURES, PROBE_FINISHES, PROBE_OUTCOMES } from "./templateProbe";

/**
 * モデルテンプレートの接続テスト（Spec 68 P2）の配線。
 *
 * どれも型検査にも lint にも掛からない形で壊れる — 呼び出しが click 以外から生えると押していないのに外へ出て
 * 課金が起きる（D9）/ 辞書の鍵は実行時に組むので欠けても通る（D11）/ 押せない条件から鍵の入力欄が落ちると
 * 古い鍵の結果が今打った鍵の結果に読まれる（rev2 点 1）/ 結果を消す処理が `reseedDraft` から外れると、鍵を
 * 消した後も「繋がりました」が残る（rev2 点 2）。
 */
const dialog = readFileSync(new URL("../components/ModelTemplateDialog.vue", import.meta.url), "utf8");
const ja = JSON.parse(readFileSync(new URL("../locales/ja.json", import.meta.url), "utf8"));
const en = JSON.parse(readFileSync(new URL("../locales/en.json", import.meta.url), "utf8"));
const coreProbe = readFileSync(
  new URL("../../../../crates/fuseforks-core/src/template_probe.rs", import.meta.url),
  "utf8",
);
const hostBoot = readFileSync(new URL("../../../../crates/fuseforks-host/src/boot.rs", import.meta.url), "utf8");
const guiState = readFileSync(new URL("../../src-tauri/src/state.rs", import.meta.url), "utf8");

function body(source: string, name: string): string {
  const start = source.indexOf(`function ${name}(`);
  expect(start, `function ${name}`).toBeGreaterThan(-1);
  const end = source.indexOf("\n}\n", start);
  expect(end).toBeGreaterThan(start);
  return source.slice(start, end);
}

function lookup(dict: Record<string, unknown>, key: string): unknown {
  return key.split(".").reduce<unknown>((node, part) => (node as Record<string, unknown> | undefined)?.[part], dict);
}

describe("接続テストの配線（Spec 68）", () => {
  it("呼び出しは click の 1 箇所だけ（D9 — 押したときだけ外へ出る）", () => {
    expect(dialog.match(/ipc\.testModelTemplate\(/g)?.length).toBe(1);
    expect(body(dialog, "runProbe")).toContain("ipc.testModelTemplate(");
    expect(dialog.match(/runProbe/g)?.length, "定義と @click の 2 回だけ").toBe(2);
    expect(dialog).toContain('@click="runProbe"');
    // 起動経路は確認を呼ばない。
    expect(hostBoot).not.toContain("probe_template");
    expect(guiState).not.toContain("probe_template");
  });

  it("押せない条件は未保存の判定と鍵の入力欄を読む（D8・rev2 点 1）", () => {
    expect(dialog).toContain(':disabled="probeBlocked !== null"');
    expect(dialog).toContain("isTemplateDirty(draft.value, savedTemplate.value)");
    expect(dialog).toContain('secretTyped: secretInput.value.trim() !== ""');
  });

  it("結果を消す処理は reseedDraft の中と、選択の切り替え・削除にある（D8・rev2 点 2）", () => {
    expect(body(dialog, "reseedDraft")).toContain("clearProbe()");
    expect(body(dialog, "edit")).toContain("clearProbe()");
    expect(body(dialog, "create")).toContain("clearProbe()");
    expect(dialog).toContain("if (probe.value?.templateId === template.id) clearProbe();");
  });

  it("接続先の原文は formatError に通し、結果の区画は v-html を使わない", () => {
    expect(dialog).toContain("formatError(shownProbe.error)");
    const start = dialog.indexOf("data-template-probe");
    const end = dialog.indexOf('@click="runProbe"');
    expect(start).toBeGreaterThan(-1);
    expect(end).toBeGreaterThan(start);
    expect(dialog.slice(start, end)).not.toContain("v-html");
  });

  it("3 つの列挙の辞書の鍵が ja / en の両方にある（D11）", () => {
    const keys = [
      ...PROBE_OUTCOMES.map((o) => `modelTemplate.probe.reached.${o}`),
      ...PROBE_FAILURES.map((f) => `modelTemplate.probe.failure.${f}`),
      ...PROBE_FINISHES.map((f) => `modelTemplate.probe.finish.${f}`),
      ...["button", "busy", "saveFirst", "registerKeyFirst", "running", "usage", "retryAfter", "notChecked"].map(
        (k) => `modelTemplate.probe.${k}`,
      ),
    ];
    for (const key of keys) {
      expect(lookup(ja, key), `ja: ${key}`).toBeTypeOf("string");
      expect(lookup(en, key), `en: ${key}`).toBeTypeOf("string");
    }
  });

  it("TS の失敗の分類は Rust の ProbeFailure と同じ 10 値", () => {
    const asStr = coreProbe.slice(coreProbe.indexOf("impl ProbeFailure"), coreProbe.indexOf("pub enum ProbeFinish"));
    const rust = [...asStr.matchAll(/=> "([a-z_]+)"/g)].map((m) => m[1]).sort();
    expect([...PROBE_FAILURES].sort()).toEqual(rust);
    expect(rust.length).toBe(10);
  });
});
