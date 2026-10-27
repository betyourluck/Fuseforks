import { describe, expect, it } from "vitest";

import {
  SHORTCUT_SECTIONS,
  isShortcutsKey,
  keyLabel,
  platformOf,
  type KeyToken,
} from "./shortcuts";

/** `isShortcutsKey` へ渡す打鍵。既定は修飾なし・変換中でない。 */
function press(key: string, extra: Partial<Parameters<typeof isShortcutsKey>[0]> = {}) {
  return { key, altKey: false, ctrlKey: false, metaKey: false, isComposing: false, ...extra };
}

describe("isShortcutsKey", () => {
  it("「?」で開く（Shift は見ない — key が実際に出た文字を運ぶ）", () => {
    expect(isShortcutsKey(press("?"))).toBe(true);
  });

  it("「/」は入力欄へ移る鍵で、一覧ではない", () => {
    expect(isShortcutsKey(press("/"))).toBe(false);
  });

  it("Ctrl / Alt / Meta が付いたら他のショートカットとして見送る", () => {
    expect(isShortcutsKey(press("?", { ctrlKey: true }))).toBe(false);
    expect(isShortcutsKey(press("?", { altKey: true }))).toBe(false);
    expect(isShortcutsKey(press("?", { metaKey: true }))).toBe(false);
  });

  it("IME の変換中は拾わない", () => {
    expect(isShortcutsKey(press("?", { isComposing: true }))).toBe(false);
  });
});

describe("platformOf", () => {
  it("mac の WebView は Macintosh を名乗る", () => {
    expect(
      platformOf("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)"),
    ).toBe("mac");
  });

  it("Windows と Linux は mac ではない", () => {
    expect(platformOf("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")).toBe("other");
    expect(platformOf("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15")).toBe("other");
  });
});

describe("keyLabel", () => {
  it("修飾キーは mac で記号、他で名前", () => {
    expect(keyLabel("Mod", "mac")).toBe("⌘");
    expect(keyLabel("Mod", "other")).toBe("Ctrl");
    expect(keyLabel("Alt", "mac")).toBe("⌥");
    expect(keyLabel("Alt", "other")).toBe("Alt");
    expect(keyLabel("Shift", "mac")).toBe("⇧");
    expect(keyLabel("Shift", "other")).toBe("Shift");
  });

  it("矢印は記号、文字の鍵はそのまま", () => {
    expect(keyLabel("UpDown", "other")).toBe("↑↓");
    expect(keyLabel("LeftRight", "mac")).toBe("←→");
    expect(keyLabel("@@", "other")).toBe("@@");
    expect(keyLabel("?", "mac")).toBe("?");
  });
});

describe("SHORTCUT_SECTIONS", () => {
  const items = SHORTCUT_SECTIONS.flatMap((s) => s.items);

  it("項目の id は一覧の中で重ならない（辞書と根拠の表の鍵になる）", () => {
    const ids = items.map((i) => i.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("区分けの id も重ならず、空の区分けは無い", () => {
    const ids = SHORTCUT_SECTIONS.map((s) => s.id);
    expect(new Set(ids).size).toBe(ids.length);
    for (const s of SHORTCUT_SECTIONS) expect(s.items.length, s.id).toBeGreaterThan(0);
  });

  it("どの項目にも鍵が 1 つ以上あり、空の組は無い", () => {
    for (const item of items) {
      expect(item.keys.length, item.id).toBeGreaterThan(0);
      for (const combo of item.keys) expect(combo.length, item.id).toBeGreaterThan(0);
    }
  });

  it("一覧自身を開く鍵が先頭に載っている（見つけた人が次に何を押すか分かる）", () => {
    expect(SHORTCUT_SECTIONS[0].items[0]).toEqual({ id: "showShortcuts", keys: [["?" as KeyToken]] });
  });
});
