import { describe, expect, it } from "vitest";

import { formatClock } from "./clock";

describe("formatClock", () => {
  it("ローカル時刻を YYYY-MM-DD HH:MM:SS で返す", () => {
    // ローカル時刻のコンストラクタ（UTC ではない）。月は 0 始まり。
    expect(formatClock(new Date(2026, 7, 3, 22, 15, 3))).toBe("2026-08-03 22:15:03");
  });

  it("月・日・時・分・秒を 2 桁へゼロ詰めする", () => {
    expect(formatClock(new Date(2026, 0, 1, 0, 0, 0))).toBe("2026-01-01 00:00:00");
  });

  it("24 時間制で出す（午後を 12 時間制へ畳まない）", () => {
    expect(formatClock(new Date(2026, 11, 31, 23, 59, 59))).toBe("2026-12-31 23:59:59");
  });

  it("診断ログの行頭と同じ形になる（スクショとログの突き合わせ）", () => {
    // diag.rs は `%Y-%m-%d %H:%M:%S%.3f`。ミリ秒だけログ側に多い。
    const line = "2026-07-31 04:34:12.481 [concordia] turn: agent=agent";
    const shown = formatClock(new Date(2026, 6, 31, 4, 34, 12));
    expect(line.startsWith(shown)).toBe(true);
  });

  it("無効な Date は — を返す（NaN を画面に出さない）", () => {
    expect(formatClock(new Date(Number.NaN))).toBe("—");
  });
});
