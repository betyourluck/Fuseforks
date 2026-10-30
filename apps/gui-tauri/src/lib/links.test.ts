import { describe, expect, it } from "vitest";

import { isOpenableLink, isRenderableLink } from "./links";

/**
 * リンクの閉じた許容（2026-10-30 の監査 8）。
 *
 * 描いてよいスキームと、押したとき外部ブラウザへ渡してよいスキームは別の集合で、
 * どちらも**許すものを列挙する**（`javascript:` を落とす除外リストにしない）。
 */
describe("isRenderableLink — Markdown がリンクとして描いてよい href", () => {
  it("http / https / mailto だけ", () => {
    expect(isRenderableLink("https://example.com/a?b=c")).toBe(true);
    expect(isRenderableLink("http://127.0.0.1:8080/")).toBe(true);
    expect(isRenderableLink("mailto:someone@example.com")).toBe(true);
    expect(isRenderableLink("HTTPS://EXAMPLE.COM")).toBe(true);
  });

  it("それ以外のスキームと相対パスは描かない", () => {
    for (const href of [
      "javascript:alert(1)",
      "vbscript:x",
      "data:text/html,<b>x</b>",
      "file:///C:/Users/x/world.json",
      "ftp://example.com/x",
      "fuseforks://open",
      "docs/readme.md",
      "/etc/passwd",
      "//example.com/x",
      "",
      " https://example.com",
    ]) {
      expect(isRenderableLink(href), href).toBe(false);
    }
  });
});

describe("isOpenableLink — 押したとき外部ブラウザへ渡してよい href", () => {
  it("http / https だけ（mailto は描くが開かない — LLM 出力由来のリンクを信頼しない）", () => {
    expect(isOpenableLink("https://example.com")).toBe(true);
    expect(isOpenableLink("http://example.com")).toBe(true);
    expect(isOpenableLink("mailto:someone@example.com")).toBe(false);
    expect(isOpenableLink("javascript:alert(1)")).toBe(false);
    expect(isOpenableLink("")).toBe(false);
  });

  it("開けるものは必ず描けるもの（部分集合）", () => {
    for (const href of ["https://a.example", "http://b.example/x"]) {
      expect(isOpenableLink(href) && isRenderableLink(href), href).toBe(true);
    }
  });
});
