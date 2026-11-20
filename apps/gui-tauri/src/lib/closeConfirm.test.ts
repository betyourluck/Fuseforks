import { describe, expect, it } from "vitest";

import { closeConfirmLines, hidesOnClose, singleExitConfirm } from "./closeConfirm";
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

/**
 * 終了の確認を 1 つに保つ（#21）。✕・OS の閉じる要求（Alt+F4 など）・トレイの「終了」が
 * 同じ守りを通る — 確認を出している間に来た要求は、確認を積まない。
 */
describe("singleExitConfirm（#21）", () => {
  /** 答えを外から与えられる確認。呼ばれた回数を数える。 */
  function deferredAsk() {
    let calls = 0;
    let answer: (value: boolean) => void = () => undefined;
    const ask = () => {
      calls += 1;
      return new Promise<boolean>((resolve) => {
        answer = resolve;
      });
    };
    return { ask, answer: (value: boolean) => answer(value), calls: () => calls };
  }

  it("確認を出している間の 2 回目は確認を積まず null を返し、1 回目は答えを返す", async () => {
    const d = deferredAsk();
    const confirmOnce = singleExitConfirm(d.ask);
    const first = confirmOnce();
    const second = confirmOnce();
    // 確認は呼んだ時点で同期的に出る — 待つ前に数える（積んでいれば 2 になる）。
    expect(d.calls()).toBe(1);
    expect(await second).toBeNull();
    d.answer(true);
    expect(await first).toBe(true);
  });

  it("答えた後は、次の要求でもう一度確認を出す", async () => {
    const d = deferredAsk();
    const confirmOnce = singleExitConfirm(d.ask);
    const first = confirmOnce();
    d.answer(false);
    expect(await first).toBe(false);
    const again = confirmOnce();
    expect(d.calls()).toBe(2);
    d.answer(true);
    expect(await again).toBe(true);
  });

  it("確認が例外で終わっても守りは外れる（次の要求で確認が出なくならない）", async () => {
    let calls = 0;
    const confirmOnce = singleExitConfirm(async () => {
      calls += 1;
      if (calls === 1) throw new Error("boom");
      return true;
    });
    await expect(confirmOnce()).rejects.toThrow("boom");
    expect(await confirmOnce()).toBe(true);
    expect(calls).toBe(2);
  });
});
