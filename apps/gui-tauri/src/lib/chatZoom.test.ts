/**
 * 会話の表示倍率（{@link isChatZoom}）の規律。
 *
 * ここで固定するのは 3 点:
 * - **選べる刻みだけを通す** — 「範囲内か」で判定すると、刻みの意図（離散にした理由）が
 *   保存値の検査から消える。1.2 は 0.9〜2 の内側だが選べない
 * - **既定は等倍** — 既定を変えると既存の村の会話ペインが黙って拡大する
 * - **表記は言語に追従しない** — `100%` は語ではなく量で、読み手の国で表記が
 *   変わると設定の値と画面の表示が突き合わせられなくなる
 */
import { describe, expect, it } from "vitest";

import {
  CHAT_ZOOM_STEPS,
  DEFAULT_CHAT_ZOOM,
  formatChatZoom,
  isChatZoom,
  type ChatZoom,
} from "./chatZoom";

describe("chatZoom", () => {
  it("既定は等倍で、刻みの中に居る", () => {
    expect(DEFAULT_CHAT_ZOOM).toBe(1);
    expect(CHAT_ZOOM_STEPS).toContain(DEFAULT_CHAT_ZOOM);
  });

  it("刻みは昇順で重複が無く、等倍を挟んで両側にある", () => {
    const steps = [...CHAT_ZOOM_STEPS];
    expect(steps).toEqual([...steps].sort((a, b) => a - b));
    expect(new Set(steps).size).toBe(steps.length);
    // 片側だけだと「小さくして情報を詰める」か「大きくして読む」の片方が選べない。
    expect(steps.some((s) => s < 1)).toBe(true);
    expect(steps.some((s) => s > 1)).toBe(true);
  });

  it("選べる刻みはすべて通る", () => {
    for (const step of CHAT_ZOOM_STEPS) {
      expect(isChatZoom(step)).toBe(true);
    }
  });

  it("刻みに無い値は、範囲の内側でも落とす", () => {
    // **範囲で判定していたらここが緑になる** — 1.2 も 1.05 も 0.9〜2 の内側。
    expect(isChatZoom(1.2)).toBe(false);
    expect(isChatZoom(1.05)).toBe(false);
    expect(isChatZoom(0.95)).toBe(false);
  });

  it("範囲の外・型違い・欠落を落とす", () => {
    expect(isChatZoom(0)).toBe(false);
    expect(isChatZoom(3)).toBe(false);
    expect(isChatZoom(-1)).toBe(false);
    // 手編集の JSON は文字列になりうる。素通しすると CSS の変数に "1.5" が入り、
    // たまたま効いてしまうので型でも落とす。
    expect(isChatZoom("1.5")).toBe(false);
    expect(isChatZoom(null)).toBe(false);
    expect(isChatZoom(undefined)).toBe(false);
    expect(isChatZoom(NaN)).toBe(false);
  });

  it("表記は百分率の整数（言語に追従しない）", () => {
    expect(formatChatZoom(0.9)).toBe("90%");
    expect(formatChatZoom(1)).toBe("100%");
    expect(formatChatZoom(1.25)).toBe("125%");
    expect(formatChatZoom(2)).toBe("200%");
  });

  it("すべての刻みが小数点を残さずに表記できる", () => {
    for (const step of CHAT_ZOOM_STEPS) {
      expect(formatChatZoom(step as ChatZoom)).toMatch(/^\d+%$/);
    }
  });
});
