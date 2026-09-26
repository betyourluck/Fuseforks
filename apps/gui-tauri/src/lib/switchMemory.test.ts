import { describe, expect, it } from "vitest";

import { parseSwitchMemory } from "./switchMemory";

describe("switchMemory: 保存値の読み", () => {
  it("正しい値だけを拾い、知らない値・壊れた値はその欄を捨てる（既定へ倒す）", () => {
    expect(parseSwitchMemory(JSON.stringify({ planReviewBypass: true, runApproval: "auto_approve" }))).toEqual({
      planReviewBypass: true,
      runApproval: "auto_approve",
    });
    expect(parseSwitchMemory(JSON.stringify({ planReviewBypass: "yes", runApproval: "everything" }))).toEqual({});
    expect(parseSwitchMemory("{broken")).toEqual({});
    expect(parseSwitchMemory("[1]")).toEqual({});
    expect(parseSwitchMemory(null)).toEqual({});
  });
});
