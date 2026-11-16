/**
 * ファイルのドラッグ&ドロップ（2026-11-16）。
 *
 * 単体: 窓の守りが**ファイルを運ぶドラッグだけ**を止め、テキストのドラッグと入力欄が受けたドラッグには触らないこと。
 * 配線（字面）: 窓の `dragDropEnabled` が false / `main.ts` が守りを張る / 入力欄が受けて貼り付けと同じ `attach` へ渡す。
 * どれも型検査に掛からず、外れると**黙って**崩れる種類 — 設定が true に戻ると drop がページへ届かず
 * 何も起きない、守りが外れると入力欄の外に落としたファイルで画面が置き換わる。
 */
// @ts-expect-error @types/node を入れない方針のため（editorSaveWiring.test.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import { carriesFiles, droppedFiles, guardFileDrop, installFileDropGuard } from "./fileDrop";

/** `dragover` / `drop` の最小の形。 */
function dragEvent(type: string, types: string[], prevented = false) {
  const event = new Event(type, { cancelable: true });
  if (prevented) event.preventDefault();
  const dataTransfer = { types, dropEffect: "move" };
  Object.defineProperty(event, "dataTransfer", { value: dataTransfer });
  return { event, dataTransfer };
}

describe("ファイルを運ぶドラッグか", () => {
  it("Files を含むときだけ", () => {
    expect(carriesFiles(["Files"])).toBe(true);
    expect(carriesFiles(["text/uri-list", "Files"])).toBe(true);
    expect(carriesFiles(["text/plain"])).toBe(false);
    expect(carriesFiles([])).toBe(false);
    expect(carriesFiles(undefined)).toBe(false);
    expect(carriesFiles(null)).toBe(false);
  });

  it("drop されたファイルが無ければ空", () => {
    expect(droppedFiles(null)).toEqual([]);
    expect(droppedFiles(undefined)).toEqual([]);
  });
});

describe("窓の守り", () => {
  it("入力欄の外に落ちたファイルは既定動作を止め、カーソルを「不可」にする", () => {
    for (const type of ["dragover", "drop"]) {
      const { event, dataTransfer } = dragEvent(type, ["Files"]);
      guardFileDrop(event as never);
      expect(event.defaultPrevented, type).toBe(true);
      expect(dataTransfer.dropEffect, type).toBe("none");
    }
  });

  it("テキストのドラッグには触らない（textarea やエディタへの文字の drop を消さない）", () => {
    const { event, dataTransfer } = dragEvent("drop", ["text/plain"]);
    guardFileDrop(event as never);
    expect(event.defaultPrevented).toBe(false);
    expect(dataTransfer.dropEffect).toBe("move");
  });

  it("入力欄が受けたドラッグには触らない（「コピー」のカーソルを上書きしない）", () => {
    const { event, dataTransfer } = dragEvent("dragover", ["Files"], true);
    dataTransfer.dropEffect = "copy";
    guardFileDrop(event as never);
    expect(dataTransfer.dropEffect).toBe("copy");
  });

  it("dragover と drop の両方に張り、外せる", () => {
    const target = new EventTarget();
    const remove = installFileDropGuard(target);
    const over = dragEvent("dragover", ["Files"]);
    target.dispatchEvent(over.event);
    expect(over.event.defaultPrevented).toBe(true);
    const drop = dragEvent("drop", ["Files"]);
    target.dispatchEvent(drop.event);
    expect(drop.event.defaultPrevented).toBe(true);
    remove();
    const after = dragEvent("drop", ["Files"]);
    target.dispatchEvent(after.event);
    expect(after.event.defaultPrevented).toBe(false);
  });
});

/** コメントを落とす（走査テストはコメントも読むので、説明文の字面に当たらないように）。 */
const code = (src: string) =>
  src.replace(/<!--[\s\S]*?-->/g, "").replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");

describe("配線", () => {
  it("窓の dragDropEnabled は false（true だと OS の drop を Tauri が横取りし、ページへ File が届かない）", () => {
    const conf = JSON.parse(readFileSync(new URL("../../src-tauri/tauri.conf.json", import.meta.url), "utf8"));
    expect(conf.app.windows).toHaveLength(1);
    expect(conf.app.windows[0].dragDropEnabled).toBe(false);
  });

  it("main.ts が起動時に窓の守りを張る", () => {
    const main = code(readFileSync(new URL("../main.ts", import.meta.url), "utf8"));
    expect(main).toContain("installFileDropGuard(window);");
  });

  it("入力欄がドラッグの 4 つを受け、落ちたファイルを貼り付けと同じ attach へ渡す", () => {
    const input = code(readFileSync(new URL("../components/ChatInput.vue", import.meta.url), "utf8"));
    for (const handler of ['@dragenter="onDragEnter"', '@dragover="onDragOver"', '@dragleave="onDragLeave"', '@drop="onDrop"']) {
      expect(input, handler).toContain(handler);
    }
    const start = input.indexOf("function onDrop(");
    const body = input.slice(start, input.indexOf("\n}\n", start));
    expect(body).toContain("void attach(droppedFiles(event.dataTransfer));");
    // 貼り付けと「参照…」も同じ attach を呼ぶ（入口ごとに規則が割れない）。
    expect(input.match(/void attach\(/g)?.length).toBe(3);
  });

  it("ドラッグ中の枠は pointer-events-none（枠がドラッグの出入りを横取りすると点滅する）", () => {
    const input = code(readFileSync(new URL("../components/ChatInput.vue", import.meta.url), "utf8"));
    expect(input).toMatch(/v-if="dropActive && !disabled"\s+class="pointer-events-none /);
  });
});
