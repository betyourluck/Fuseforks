import { describe, expect, it } from "vitest";
import { parseHidden, toggleHidden } from "./useHiddenGroups";

describe("useHiddenGroups の純関数", () => {
  it("壊れた保存値・配列でない値・文字列でない要素は落とし、重複は畳む", () => {
    expect(parseHidden(null)).toEqual([]);
    expect(parseHidden("{ not json")).toEqual([]);
    expect(parseHidden('{"a":1}')).toEqual([]);
    expect(parseHidden('["g1", 2, "", null, "g1", "g2"]')).toEqual(["g1", "g2"]);
  });

  it("toggle は無ければ足し、あれば外す（他の id は動かさない）", () => {
    expect(toggleHidden([], "g1")).toEqual(["g1"]);
    expect(toggleHidden(["g1", "g2"], "g1")).toEqual(["g2"]);
    expect(toggleHidden(["g2"], "g1")).toEqual(["g2", "g1"]);
  });
});
