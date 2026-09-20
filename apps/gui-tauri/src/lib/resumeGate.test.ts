import { describe, expect, it } from "vitest";

import { canResume } from "./resumeGate";
import type { AgentStatus } from "../types";

describe("canResume", () => {
  /**
   * **5 状態を全部書く。** 「止まっている側」をまとめて 1 本にすると、
   * 状態が 1 つ増えたときにどちらへ倒れるかが実測で決まらなくなる
   * （型は網羅を強制しない — `canResume` は列挙の match ではなく比較なので、
   * 新しい状態は黙って偽側へ落ちる）。
   */
  const cases: [AgentStatus, boolean][] = [
    ["idle", false],
    ["starting", true],
    ["running", true],
    ["stopping", false],
    ["failed", false],
  ];

  for (const [status, expected] of cases) {
    it(`${status} は ${expected ? "押せる" : "押せない"}`, () => {
      expect(canResume(status)).toBe(expected);
    });
  }

  it("一覧に居ない個体は押せない", () => {
    // 削除された直後・まだ読めていない、のどちらでも押せない側へ倒す。
    expect(canResume(undefined)).toBe(false);
  });
});
