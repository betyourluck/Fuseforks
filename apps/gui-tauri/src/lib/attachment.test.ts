import { describe, expect, it } from "vitest";

import { MAX_EDGE_PX, bytesToBase64, fitWithin, isWebpBytes } from "./attachment";

describe("fitWithin", () => {
  it("長辺が上限ちょうどなら縮小しない（境界値）", () => {
    expect(fitWithin(MAX_EDGE_PX, 10)).toEqual({
      width: MAX_EDGE_PX,
      height: 10,
      scaled: false,
    });
  });

  it("長辺が上限を 1px 超えたら縮小し、長辺は正確に上限になる", () => {
    const result = fitWithin(MAX_EDGE_PX + 1, 100);
    expect(result.scaled).toBe(true);
    expect(result.width).toBe(MAX_EDGE_PX);
  });

  it("アスペクト比を保つ（16:9 の 3136×1764 → 1568×882）", () => {
    expect(fitWithin(3136, 1764)).toEqual({
      width: 1568,
      height: 882,
      scaled: true,
    });
  });

  it("縦長でも同じ規則（長辺は高さ側）", () => {
    expect(fitWithin(100, MAX_EDGE_PX * 2)).toEqual({
      width: 50,
      height: MAX_EDGE_PX,
      scaled: true,
    });
  });

  it("極端な比率でも 0px にならない", () => {
    const result = fitWithin(100_000, 1);
    expect(result.width).toBe(MAX_EDGE_PX);
    expect(result.height).toBeGreaterThanOrEqual(1);
  });
});

describe("bytesToBase64", () => {
  it("atob で往復できる（チャンク境界をまたぐ長さ）", () => {
    // 0x8000 の刻みをまたがせて、チャンク分割の継ぎ目を検査する。
    const bytes = new Uint8Array(0x8000 + 17);
    for (let i = 0; i < bytes.length; i += 1) bytes[i] = i % 256;

    const decoded = atob(bytesToBase64(bytes));
    expect(decoded.length).toBe(bytes.length);
    for (let i = 0; i < bytes.length; i += 1) {
      expect(decoded.charCodeAt(i)).toBe(bytes[i]);
    }
  });
});

describe("isWebpBytes", () => {
  it("RIFF....WEBP を通し、それ以外を落とす（コアの is_webp と同じ規則）", () => {
    const webp = new Uint8Array([
      0x52, 0x49, 0x46, 0x46, 0, 0, 0, 0, 0x57, 0x45, 0x42, 0x50,
    ]);
    expect(isWebpBytes(webp)).toBe(true);

    const png = new Uint8Array([
      0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0,
    ]);
    expect(isWebpBytes(png)).toBe(false);
    expect(isWebpBytes(new Uint8Array(4))).toBe(false);
  });
});
