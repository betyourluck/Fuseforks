import { describe, expect, it } from "vitest";

import { LONG_PROMPT_CHARS, LONG_PROMPT_LINES, isLongPrompt } from "./longPrompt";

describe("isLongPrompt", () => {
  it("短い発言は畳まない", () => {
    expect(isLongPrompt("")).toBe(false);
    expect(isLongPrompt("調べてください")).toBe(false);
  });

  it("行数が境界を超えたら畳む（境界ちょうどは畳まない）", () => {
    const lines = (n: number) => Array.from({ length: n }, (_, i) => `${i}`).join("\n");
    expect(isLongPrompt(lines(LONG_PROMPT_LINES))).toBe(false);
    expect(isLongPrompt(lines(LONG_PROMPT_LINES + 1))).toBe(true);
  });

  it("改行の無い長い 1 段落は文字数で畳む（境界ちょうどは畳まない）", () => {
    expect(isLongPrompt("あ".repeat(LONG_PROMPT_CHARS))).toBe(false);
    expect(isLongPrompt("あ".repeat(LONG_PROMPT_CHARS + 1))).toBe(true);
  });

  it("文字は code point で数える（UTF-16 の length で数えると絵文字で早く畳まれる）", () => {
    // 🍣 は length 2 / code point 1。length で数える実装だと LONG_PROMPT_CHARS / 2 + 1 個で畳まれる。
    expect(isLongPrompt("🍣".repeat(LONG_PROMPT_CHARS))).toBe(false);
  });
});
