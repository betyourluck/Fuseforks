import { describe, expect, it } from "vitest";

import type { ModelTemplate } from "../types";
import {
  ASSIST_STORAGE_KEY,
  applyNeedsConfirm,
  codePoints,
  draftDelta,
  pickTemplate,
  readRemembered,
  templateChoices,
  writeRemembered,
} from "./assist";

function tpl(id: string, useTools: boolean): ModelTemplate {
  return { id, name: `name-${id}`, model: `model-${id}`, useTools } as ModelTemplate;
}

describe("生成役のテンプレート（Spec 63 D2）", () => {
  const templates = [tpl("no-tools", false), tpl("a", true), tpl("b", true)];

  it("ツールを使わないものは並べるが選べない", () => {
    const choices = templateChoices(templates);
    expect(choices.map((c) => [c.id, c.disabled])).toEqual([
      ["no-tools", true],
      ["a", false],
      ["b", false],
    ]);
  });

  it("覚えた選択が使えればそれ、使えなければ選べる最初のもの", () => {
    expect(pickTemplate(templates, "b")).toBe("b");
    expect(pickTemplate(templates, "no-tools")).toBe("a");
    expect(pickTemplate(templates, "gone")).toBe("a");
    expect(pickTemplate(templates, null)).toBe("a");
    expect(pickTemplate([tpl("x", false)], null)).toBeNull();
  });

  it("覚えた選択を棚へ書いて読む。壊れた値は null", () => {
    const shelf = new Map<string, string>();
    const storage = {
      getItem: (k: string) => shelf.get(k) ?? null,
      setItem: (k: string, v: string) => void shelf.set(k, v),
    };
    writeRemembered(storage, "b");
    expect(shelf.get(ASSIST_STORAGE_KEY)).toBe('{"templateId":"b"}');
    expect(readRemembered(storage)).toBe("b");
    shelf.set(ASSIST_STORAGE_KEY, "{壊れた");
    expect(readRemembered(storage)).toBeNull();
    shelf.set(ASSIST_STORAGE_KEY, '{"templateId":3}');
    expect(readRemembered(storage)).toBeNull();
    expect(readRemembered(null)).toBeNull();
  });

  it("棚が投げても補助は止まらない", () => {
    const throwing = {
      getItem: () => {
        throw new Error("blocked");
      },
      setItem: () => {
        throw new Error("blocked");
      },
    };
    expect(readRemembered(throwing)).toBeNull();
    expect(() => writeRemembered(throwing, "a")).not.toThrow();
  });
});

describe("反映の前の差（Spec 63 D9）", () => {
  it("字数はコードポイントで数える（コアと同じ）", () => {
    expect(codePoints("手順😀")).toBe(3);
    expect("手順😀".length).toBe(4);
  });

  it("増えた行と消えた行を多重集合で数え、並べ替えは差にしない", () => {
    expect(draftDelta("a\nb\nc", "c\nb\na")).toMatchObject({ added: 0, removed: 0 });
    expect(draftDelta("a\nb\nc", "a\nx")).toMatchObject({ added: 1, removed: 2 });
    expect(draftDelta("", "a\nb")).toMatchObject({ beforeChars: 0, afterChars: 3, added: 2, removed: 0 });
    expect(draftDelta("a\na", "a")).toMatchObject({ added: 0, removed: 1 });
  });

  it("編集中の本文が空でなければ確認を出す", () => {
    expect(applyNeedsConfirm("  \n")).toBe(false);
    expect(applyNeedsConfirm("# 手順")).toBe(true);
  });
});
