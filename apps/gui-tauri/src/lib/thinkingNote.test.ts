import { describe, expect, it } from "vitest";

import { thinkingView } from "./thinkingNote";
import type { AgentMessage } from "../types";

function message(reasoningSummary?: string[]): AgentMessage {
  return {
    id: "m1",
    from: { kind: "agent", id: "agent_01" },
    to: { kind: "user" },
    content: "答え",
    tokens: 0,
    tsMs: 0,
    hop: 0,
    reasoningSummary,
  } as unknown as AgentMessage;
}

describe("thinkingView", () => {
  it("要約が無い発話では null（大多数のターンがこちら）", () => {
    expect(thinkingView(message())).toBeNull();
    expect(thinkingView(message([]))).toBeNull();
  });

  it("要約があれば字数を数えて返す", () => {
    const view = thinkingView(message(["abc", "でえふ"]));
    expect(view).not.toBeNull();
    expect(view!.summaries).toEqual(["abc", "でえふ"]);
    expect(view!.chars).toBe(6);
  });

  /**
   * **長さで足切りしない**（Spec 33 D4）。短形（問いの再掲だけ）も出す —
   * 129 字と 919 字の間に線を引くと、probe の産物である数字が表示規則になる
   * （`failures.md` #92）。薄いことは利用者が読めば分かる。
   */
  it("短い要約も落とさない", () => {
    const short = "The problem is in Japanese: 3 人の…";
    expect(thinkingView(message([short]))!.summaries).toEqual([short]);
  });

  /**
   * **重複を潰さない。** 同じ文が 2 度出るなら、それはモデルが 2 周とも
   * 同じことを考えた事実であって重複ではない（接地の来歴とは畳み方が違う）。
   */
  it("同じ要約が並んでも畳まない", () => {
    const view = thinkingView(message(["same", "same"]));
    expect(view!.summaries).toEqual(["same", "same"]);
    expect(view!.chars).toBe(8);
  });

  /** 字数はコードポイントで数える（サロゲートペアを 2 と数えない）。 */
  it("字数は見た目の文字数で数える", () => {
    expect(thinkingView(message(["𝔸𝔹"]))!.chars).toBe(2);
  });
});
