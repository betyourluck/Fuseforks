// @ts-expect-error @types/node を入れない方針のため（editorSaveWiring.test.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import ja from "../locales/ja.json";
import en from "../locales/en.json";
import {
  JEV_DEFAULT_THRESHOLD,
  JEV_THRESHOLDS,
  jevThresholdKey,
  normalizeJevThreshold,
} from "./jevThreshold";

/**
 * ツール結果の即時圧縮の配線（Spec 59 P2）。
 *
 * ここが留めているのは**型でも lint でも落ちない 3 つ**:
 *
 * 1. **閾値の表が Rust と TS で 2 枚ある。** ずれると「画面で選んだ強さと実際に
 *    落ちる量が違う」になる（値は `jev_settings.rs` が正で、TS はその写し）
 * 2. **ラベルの辞書鍵を実行時に組む**（`settings.jev.threshold.${値}`）ので、
 *    鍵が欠けても vue-tsc も `i18n/index.test.ts` のコンパイル検査も通り、
 *    画面に鍵名がそのまま出る
 * 3. **開いただけで外へ出ない。** ページの表示で `testJev` を呼ぶ実装へ変わると、
 *    押していないのにツール結果の採点（= 外部送信）が起きる（D10 の凍結）
 */
const dialog = readFileSync(new URL("../components/SettingsDialog.vue", import.meta.url), "utf8");
const settingsRs = readFileSync(
  new URL("../../src-tauri/src/jev_settings.rs", import.meta.url),
  "utf8",
);

/** Rust の `THRESHOLDS` を読む（`pub const THRESHOLDS: [f32; 4] = [...]`）。 */
function rustThresholds(): number[] {
  const m = settingsRs.match(/pub const THRESHOLDS: \[f32; \d+\] = \[([^\]]+)\];/);
  if (!m) throw new Error("jev_settings.rs の THRESHOLDS を読めない");
  return m[1].split(",").map((part: string) => Number(part.trim()));
}

/** Rust の `DEFAULT_THRESHOLD` を読む。 */
function rustDefault(): number {
  const m = settingsRs.match(/pub const DEFAULT_THRESHOLD: f32 = ([\d.]+);/);
  if (!m) throw new Error("jev_settings.rs の DEFAULT_THRESHOLD を読めない");
  return Number(m[1]);
}

describe("Jev の設定ページの配線", () => {
  it("閾値の表が Rust と一致する（値も並びも既定も）", () => {
    expect([...JEV_THRESHOLDS]).toEqual(rustThresholds());
    expect(JEV_DEFAULT_THRESHOLD).toBe(rustDefault());
    // 既定は集合の中に居る（集合から外れると `normalize` が無限に既定へ落とす）。
    expect([...JEV_THRESHOLDS]).toContain(JEV_DEFAULT_THRESHOLD);
  });

  it("集合に無い値は既定へ落ちる（範囲で受けない）", () => {
    for (const good of JEV_THRESHOLDS) expect(normalizeJevThreshold(good)).toBe(good);
    for (const bad of [0, 0.15, 0.22, 0.9, -1, Number.NaN, "0.2", null, undefined]) {
      expect(normalizeJevThreshold(bad)).toBe(JEV_DEFAULT_THRESHOLD);
    }
  });

  it("ラベルの辞書鍵が 4 値すべてに在る（ja / en とも）", () => {
    for (const dict of [ja, en] as const) {
      const table = (dict as { settings: { jev: { threshold: Record<string, string> } } }).settings
        .jev.threshold;
      const keys = Object.keys(table).sort();
      expect(keys).toEqual(JEV_THRESHOLDS.map((t) => jevThresholdKey(t).split(".").pop()!).sort());
      for (const value of Object.values(table)) expect(value.length).toBeGreaterThan(0);
    }
  });

  it("画面はラベルを純関数の鍵から引き、値を直書きしない", () => {
    expect(dialog).toContain("jevThresholdKey(level)");
    expect(dialog).toContain("v-for=\"level in JEV_THRESHOLDS\"");
    // 4 値をテンプレートへ並べ直さない（表は 1 つ）。
    expect(dialog).not.toMatch(/:value="0\.[1235]"/);
  });

  it("チェックの disabled は Rust が返した canEnable を読む", () => {
    expect(dialog).toContain(":disabled=\"!jev?.canEnable || jevBusy\"");
    // 条件を画面で組み直さない（`hasToken && accountId` の再実装を作らない）。
    expect(dialog).not.toMatch(/jev\?\.hasToken\s*&&\s*jev/);
  });

  it("「いま掛かっているか」も Rust の active を読む", () => {
    expect(dialog).toContain("jev.active");
    expect(dialog).toContain("settings.jev.activeYes");
    expect(dialog).toContain("settings.jev.activeNo");
  });

  it("ページを開いても外へ出ない（読むのは設定だけ）", () => {
    expect(dialog).toContain('if (next === "jev") void loadJev();');
    // `selectPage` の中で接続の確認を呼ばない。
    const select = dialog.slice(dialog.indexOf("function selectPage("));
    expect(select.slice(0, select.indexOf("}"))).not.toContain("testJev");
    // 確認はボタンからだけ。
    expect(dialog).toContain('@click="testJev"');
  });

  it("設定ファイルが読めないときの表示がある（黙って OFF にしない）", () => {
    expect(dialog).toContain("jev?.blocked");
    expect(dialog).toContain("settings.jev.blocked");
    for (const dict of [ja, en] as const) {
      const text = (dict as { settings: { jev: { blocked: string } } }).settings.jev.blocked;
      expect(text).toContain("{reason}");
      expect(text).toContain("jev.json");
    }
  });

  it("トークンの値を読み出す口を画面から呼ばない", () => {
    // 登録・削除はあるが、取得は無い（そもそも IPC が無い）。
    expect(dialog).toContain("ipc.setJevToken");
    expect(dialog).toContain("ipc.clearJevToken");
    expect(dialog).not.toContain("getJevToken");
  });
});
