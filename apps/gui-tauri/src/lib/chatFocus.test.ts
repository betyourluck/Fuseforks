import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync, readdirSync } from "node:fs";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

import {
  OVERLAY_SELECTOR,
  hasOpenOverlay,
  isEditableElement,
  isFocusChatKey,
  type FocusKeyEvent,
} from "./chatFocus";

function key(overrides: Partial<FocusKeyEvent>): FocusKeyEvent {
  return {
    key: "/",
    altKey: false,
    ctrlKey: false,
    metaKey: false,
    isComposing: false,
    ...overrides,
  };
}

describe("isFocusChatKey", () => {
  it("素の「/」だけを拾う", () => {
    expect(isFocusChatKey(key({}))).toBe(true);
    expect(isFocusChatKey(key({ key: "a" }))).toBe(false);
    expect(isFocusChatKey(key({ key: "?" }))).toBe(false);
  });

  it("Shift は見ない（Shift で「/」を打つ配列がある）", () => {
    expect(isFocusChatKey({ ...key({}), shiftKey: true } as FocusKeyEvent)).toBe(true);
  });

  it("Ctrl / Alt / Meta が付いたら他のショートカットとして見送る", () => {
    expect(isFocusChatKey(key({ ctrlKey: true }))).toBe(false);
    expect(isFocusChatKey(key({ altKey: true }))).toBe(false);
    expect(isFocusChatKey(key({ metaKey: true }))).toBe(false);
  });

  it("IME の変換中は拾わない", () => {
    expect(isFocusChatKey(key({ isComposing: true }))).toBe(false);
  });
});

describe("isEditableElement", () => {
  it("何もフォーカスしていなければ打ち込める要素ではない", () => {
    expect(isEditableElement(null)).toBe(false);
    expect(isEditableElement({ tagName: "BODY" })).toBe(false);
    expect(isEditableElement({ tagName: "BUTTON" })).toBe(false);
  });

  it("input / textarea / select は打ち込める要素", () => {
    expect(isEditableElement({ tagName: "INPUT" })).toBe(true);
    expect(isEditableElement({ tagName: "textarea" })).toBe(true);
    expect(isEditableElement({ tagName: "SELECT" })).toBe(true);
  });

  it("contenteditable（CodeMirror の編集領域）も打ち込める要素", () => {
    expect(isEditableElement({ tagName: "DIV", isContentEditable: true })).toBe(true);
    expect(isEditableElement({ tagName: "DIV", isContentEditable: false })).toBe(false);
  });
});

describe("hasOpenOverlay", () => {
  it("選択子に当たる要素があれば覆いが出ている", () => {
    const seen: string[] = [];
    const root = {
      querySelector(selectors: string) {
        seen.push(selectors);
        return {};
      },
    };
    expect(hasOpenOverlay(root)).toBe(true);
    expect(seen).toEqual([OVERLAY_SELECTOR]);
    expect(hasOpenOverlay({ querySelector: () => null })).toBe(false);
  });
});

/**
 * **覆いの判定は `bg-scrim` という約束に寄りかかっている。** 画面全体を覆う層
 * （`fixed inset-0`）を持つ部品が `bg-scrim` も `aria-modal` も持たないと、その層が
 * 出ている間に「/」が裏の入力欄へフォーカスを移す — 型検査にも lint にも掛からない。
 */
describe("画面全体を覆う層は、覆いの判定に当たる印を持つ", () => {
  const dir = fileURLToPath(new URL("../components/", import.meta.url));
  const files = (readdirSync(dir) as string[]).filter((f: string) => f.endsWith(".vue"));
  const covering = files.filter((f: string) =>
    (readFileSync(`${dir}${f}`, "utf8") as string).includes("fixed inset-0"),
  );

  it("覆う層を持つ部品が 1 つ以上ある（走査が空振りしていない）", () => {
    expect(covering.length).toBeGreaterThan(0);
  });

  it.each(covering)("%s", (file: string) => {
    const text = readFileSync(`${dir}${file}`, "utf8") as string;
    expect(text.includes("bg-scrim") || text.includes('aria-modal="true"')).toBe(true);
  });
});
