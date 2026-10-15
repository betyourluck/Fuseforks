import { describe, expect, it } from "vitest";

import {
  findMatches,
  initialCurrent,
  isChatSearchKey,
  matchRanges,
  normalizeQuery,
  stepCurrent,
} from "./chatSearch";

const key = (overrides: Partial<Parameters<typeof isChatSearchKey>[0]>) => ({
  key: "f",
  ctrlKey: true,
  metaKey: false,
  altKey: false,
  shiftKey: false,
  isComposing: false,
  ...overrides,
});

describe("isChatSearchKey", () => {
  it("Ctrl+F と Cmd+F を受ける（大文字でも）", () => {
    expect(isChatSearchKey(key({}))).toBe(true);
    expect(isChatSearchKey(key({ key: "F" }))).toBe(true);
    expect(isChatSearchKey(key({ ctrlKey: false, metaKey: true }))).toBe(true);
  });

  it("修飾が足りない・余る打鍵は拾わない", () => {
    expect(isChatSearchKey(key({ ctrlKey: false }))).toBe(false);
    expect(isChatSearchKey(key({ metaKey: true }))).toBe(false);
    expect(isChatSearchKey(key({ shiftKey: true }))).toBe(false);
    expect(isChatSearchKey(key({ altKey: true }))).toBe(false);
    expect(isChatSearchKey(key({ key: "g" }))).toBe(false);
  });

  it("IME の変換中は拾わない", () => {
    expect(isChatSearchKey(key({ isComposing: true }))).toBe(false);
  });
});

describe("findMatches", () => {
  const items = [
    { id: "m1", text: "cargo test が通りました" },
    { id: "m2", text: "Cargo build は 11 分" },
    { id: "m3", text: "関係ない行" },
  ];

  it("大文字小文字を区別しない部分一致で、画面の並びのまま返す", () => {
    expect(findMatches(items, "cargo")).toEqual(["m1", "m2"]);
    expect(findMatches(items, "CARGO")).toEqual(["m1", "m2"]);
    expect(findMatches(items, "分")).toEqual(["m2"]);
  });

  it("空白だけの問いは一致なし", () => {
    expect(findMatches(items, "")).toEqual([]);
    expect(findMatches(items, "   ")).toEqual([]);
    expect(normalizeQuery("  Ab ")).toBe("ab");
  });
});

describe("matchRanges", () => {
  it("重ならない区間を左から返す", () => {
    expect(matchRanges("abcabc", "bc")).toEqual([
      [1, 3],
      [4, 6],
    ]);
    expect(matchRanges("aaa", "aa")).toEqual([[0, 2]]);
  });

  it("大文字小文字を区別しない", () => {
    expect(matchRanges("Cargo cargo", "CARGO")).toEqual([
      [0, 5],
      [6, 11],
    ]);
  });

  it("小文字化で長さが変わる文章では区別する側へ落ちる（添字をずらさない）", () => {
    // "İ" は toLowerCase で 2 コードユニットになる。
    const text = "İstanbul x";
    expect(text.toLowerCase().length).not.toBe(text.length);
    expect(matchRanges(text, "x")).toEqual([[9, 10]]);
    expect(matchRanges(text, "X")).toEqual([]);
  });
});

describe("現在位置", () => {
  it("新しい問いは末尾（最も新しい一致）から", () => {
    expect(initialCurrent(3)).toBe(2);
    expect(initialCurrent(0)).toBe(-1);
  });

  it("端で巻き戻る", () => {
    expect(stepCurrent(2, 3, 1)).toBe(0);
    expect(stepCurrent(0, 3, -1)).toBe(2);
    expect(stepCurrent(1, 3, 1)).toBe(2);
  });

  it("一致が無ければ -1、範囲外から動かすと末尾へ戻る", () => {
    expect(stepCurrent(0, 0, 1)).toBe(-1);
    expect(stepCurrent(5, 3, 1)).toBe(2);
    expect(stepCurrent(-1, 3, -1)).toBe(2);
  });
});
