import { describe, expect, it } from "vitest";

import { closeConfirmLines, hidesOnClose } from "./closeConfirm";
import ja from "../locales/ja.json";
import en from "../locales/en.json";

/** 鍵だけを取り出す（本文は辞書の担当）。 */
const keys = (state: Parameters<typeof closeConfirmLines>[0]) =>
  closeConfirmLines(state).map((line) => line.key);

describe("closeConfirmLines", () => {
  it("何も走っていなくても、前置きと予定は必ず出る", () => {
    // **予定が止まる事実は稼働状態に依らない。** しかも終了すれば予定は発火せず、
    // その事実は他のどの画面にも出ていない（トレイに隠れている間は終了していない）。
    expect(keys({ runningAgents: 0, mcpListening: false })).toEqual([
      "closeConfirm.lead",
      "closeConfirm.schedules",
    ]);
  });

  it("稼働中が居るときだけ、その行が件数つきで挟まる", () => {
    const lines = closeConfirmLines({ runningAgents: 3, mcpListening: false });
    expect(lines).toContainEqual({
      key: "closeConfirm.running",
      params: { count: 3 },
    });
  });

  it("0 体を「0 体です」と書かない", () => {
    // 失われないものを並べても判断の材料にならない。
    expect(keys({ runningAgents: 0, mcpListening: false })).not.toContain(
      "closeConfirm.running",
    );
  });

  it("扉は開いているときだけ書く（対で見る）", () => {
    // 片方だけを見ると、常に出す実装でも常に出さない実装でも通る。
    expect(keys({ runningAgents: 0, mcpListening: true })).toContain("closeConfirm.mcp");
    expect(keys({ runningAgents: 0, mcpListening: false })).not.toContain("closeConfirm.mcp");
  });

  it("全部当てはまると 4 行、順序は前置き → 稼働 → 扉 → 予定", () => {
    expect(keys({ runningAgents: 2, mcpListening: true })).toEqual([
      "closeConfirm.lead",
      "closeConfirm.running",
      "closeConfirm.mcp",
      "closeConfirm.schedules",
    ]);
  });

  it("出しうる鍵はすべて ja / en に訳がある", () => {
    // 訳の漏れは fallback で画面に出ず、テストでしか見えない
    // （i18n の鍵集合一致テストと同じ性質）。
    const all = new Set([
      ...keys({ runningAgents: 1, mcpListening: true }),
      ...keys({ runningAgents: 0, mcpListening: false }),
    ]);
    for (const key of all) {
      const leaf = key.split(".")[1];
      expect(ja.closeConfirm, `ja に ${key} が無い`).toHaveProperty(leaf);
      expect(en.closeConfirm, `en に ${key} が無い`).toHaveProperty(leaf);
    }
    // ボタンのラベルも同じ棚から引く。
    for (const leaf of ["title", "confirm", "cancel"]) {
      expect(ja.closeConfirm).toHaveProperty(leaf);
      expect(en.closeConfirm).toHaveProperty(leaf);
    }
  });
});

describe("hidesOnClose（Spec 69 D2）", () => {
  it("トレイ ON で組み立て済みなら隠す", () => {
    expect(hidesOnClose({ minimizeToTray: true, ready: true, failed: false })).toBe(true);
  });
  it("トレイ OFF なら隠さない（今までどおり確認 → 終了）", () => {
    expect(hidesOnClose({ minimizeToTray: false, ready: true, failed: false })).toBe(false);
  });
  it("組み立てていない間は隠さない — 村を開けていないプロセスをトレイに残さない", () => {
    expect(hidesOnClose({ minimizeToTray: true, ready: false, failed: false })).toBe(false);
    expect(hidesOnClose({ minimizeToTray: true, ready: false, failed: true })).toBe(false);
    expect(hidesOnClose({ minimizeToTray: true, ready: true, failed: true })).toBe(false);
  });
});
