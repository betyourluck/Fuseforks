import { describe, expect, it } from "vitest";

import {
  CONTEXT_FAIL_RATIO,
  CONTEXT_WARN_RATIO,
  contextArc,
  contextPercent,
  contextRatio,
  contextTone,
} from "./contextUsage";

describe("contextRatio", () => {
  it("分子が null（まだ呼び出していない）なら出さない", () => {
    expect(contextRatio(null, 128_000)).toBeNull();
    expect(contextRatio(undefined, 128_000)).toBeNull();
  });

  it("分母が無いか 0 なら出さない（0 で割らない）", () => {
    expect(contextRatio(12_157, null)).toBeNull();
    expect(contextRatio(12_157, 0)).toBeNull();
    expect(contextRatio(12_157, undefined)).toBeNull();
  });

  it("分子 0 は 0（null ではない）— 呼び出したが入力が 0 の応答は「出さない」ではない", () => {
    expect(contextRatio(0, 128_000)).toBe(0);
  });

  it("実機の数字: 3.8 の 12,157 ÷ 1,048,576", () => {
    expect(contextRatio(12_157, 1_048_576)).toBeCloseTo(0.0116, 4);
  });

  it("1.0 を超える比は切り詰めない（設定が窓より小さい診断）", () => {
    expect(contextRatio(150_000, 128_000)).toBeCloseTo(1.1719, 4);
  });
});

describe("contextTone", () => {
  it("境界値込みの 3 段（利用者指定: 75% 以上は黄、90% 以上は赤、通常は青）", () => {
    expect(contextTone(0)).toBe("text-accent");
    expect(contextTone(0.7499)).toBe("text-accent");
    expect(contextTone(CONTEXT_WARN_RATIO)).toBe("text-warn");
    expect(contextTone(0.8999)).toBe("text-warn");
    expect(contextTone(CONTEXT_FAIL_RATIO)).toBe("text-fail");
    expect(contextTone(1.5)).toBe("text-fail");
  });
});

describe("contextArc / contextPercent", () => {
  it("弧は 1.0 で止まり、数字は丸めない", () => {
    expect(contextArc(1.1719)).toBe(1);
    expect(contextPercent(1.1719)).toBe(117);
    expect(contextArc(0.5)).toBe(0.5);
    expect(contextPercent(0.0116)).toBe(1);
  });

  it("負の比は弧 0（守り。分子は u64 なので通常は来ない）", () => {
    expect(contextArc(-0.2)).toBe(0);
  });
});
