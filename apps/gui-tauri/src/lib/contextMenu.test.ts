import { describe, expect, it } from "vitest";

import { keepsNativeMenu } from "./contextMenu";

/** `closest` だけを持つ偽の要素。渡された選択子に一致するかを固定で答える。 */
function fakeTarget(matches: boolean) {
  return {
    closest: (selectors: string) => {
      // 選択子そのものも検証対象 — 入力欄の 3 種が落ちていたら退行。
      expect(selectors).toContain("input");
      expect(selectors).toContain("textarea");
      expect(selectors).toContain("contenteditable");
      return matches ? {} : null;
    },
  };
}

describe("keepsNativeMenu", () => {
  it("入力欄の上ではメニューを残す（貼り付けの導線）", () => {
    expect(keepsNativeMenu(fakeTarget(true), false)).toBe(true);
  });

  it("入力欄でなければ抑止する", () => {
    expect(keepsNativeMenu(fakeTarget(false), false)).toBe(false);
  });

  it("テキストを選択していれば場所を問わず残す（コピーの導線）", () => {
    expect(keepsNativeMenu(fakeTarget(false), true)).toBe(true);
  });

  it("closest を持たない対象（テキストノード・null）は抑止側へ倒す", () => {
    // instanceof で弾く実装だと node 環境のテストが書けない（kizunaDrop と同じ理由）。
    expect(keepsNativeMenu(null, false)).toBe(false);
    expect(keepsNativeMenu(undefined, false)).toBe(false);
    expect(keepsNativeMenu({}, false)).toBe(false);
  });

  it("選択があれば closest を持たない対象でも残す", () => {
    expect(keepsNativeMenu(null, true)).toBe(true);
  });

  it("CodeMirror の編集領域を拾う選択子であること（役職ダイアログの貼り付け）", () => {
    // CodeEditor.vue は CodeMirror 6 で、編集領域は textarea ではなく
    // `contenteditable` を持つ div.cm-content。選択子から contenteditable を
    // 落とすと役職の本文が貼り付けられなくなる（画面でしか気づけない退行）。
    const cmContent = {
      closest: (selectors: string) => (selectors.includes("contenteditable") ? {} : null),
    };
    expect(keepsNativeMenu(cmContent, false)).toBe(true);
  });
});
