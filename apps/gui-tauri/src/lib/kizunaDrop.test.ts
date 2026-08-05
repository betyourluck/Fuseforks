import { describe, expect, it } from "vitest";

import { dropPoint, tieAddition } from "./kizunaDrop";

const agents = [
  { id: "a", connectedAgents: ["b"] },
  { id: "b", connectedAgents: [] },
  { id: "c", connectedAgents: [] },
];

describe("tieAddition", () => {
  it("未接続の相手へは追加した一覧を返す", () => {
    expect(tieAddition(agents, "a", "c")).toEqual(["b", "c"]);
  });

  it("方向付きの重複は null（A→B が既にあるとき A から B へは張れない）", () => {
    expect(tieAddition(agents, "a", "b")).toBeNull();
  });

  it("逆向きは別の絆（A→B があっても B→A は張れる = 双方向化）", () => {
    expect(tieAddition(agents, "b", "a")).toEqual(["a"]);
  });

  it("自分自身へは null", () => {
    expect(tieAddition(agents, "a", "a")).toBeNull();
  });

  it("存在しない source / target は null", () => {
    expect(tieAddition(agents, "ghost", "a")).toBeNull();
    expect(tieAddition(agents, "a", "ghost")).toBeNull();
  });

  it("元の配列を書き換えない", () => {
    tieAddition(agents, "a", "c");
    expect(agents[0].connectedAgents).toEqual(["b"]);
  });
});

describe("dropPoint", () => {
  it("マウス系イベントは clientX/Y を返す", () => {
    expect(dropPoint({ clientX: 10, clientY: 20 })).toEqual({ x: 10, y: 20 });
  });

  it("タッチ系イベントは changedTouches の先頭を返す", () => {
    expect(dropPoint({ changedTouches: [{ clientX: 3, clientY: 4 }] })).toEqual({
      x: 3,
      y: 4,
    });
  });

  it("イベントが無い・座標が無いときは null", () => {
    expect(dropPoint(undefined)).toBeNull();
    expect(dropPoint({})).toBeNull();
    expect(dropPoint({ changedTouches: [] })).toBeNull();
  });
});
