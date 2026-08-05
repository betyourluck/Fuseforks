import { describe, expect, it } from "vitest";

import {
  buildTimeline,
  collapseRows,
  isSystemNotice,
  type ToolRun,
} from "./chatRows";
import type { AgentMessage, Endpoint } from "../types";

let seq = 0;
function message(
  from: Endpoint,
  to: Endpoint,
  content: string,
  hop = 0,
): AgentMessage {
  return { id: `m${++seq}`, from, to, content, tokens: 0, tsMs: 0, hop };
}

const user: Endpoint = { kind: "user" };
const agent = (id: string): Endpoint => ({ kind: "agent", id });

describe("collapseRows", () => {
  it("ユーザー同報を 1 行に畳み、宛先を束ねる", () => {
    const rows = collapseRows([
      message(user, agent("a"), "みんなこんにちは"),
      message(user, agent("b"), "みんなこんにちは"),
      message(user, agent("c"), "みんなこんにちは"),
    ]);
    expect(rows).toHaveLength(1);
    expect(rows[0].extraTargets).toHaveLength(2);
  });

  it("エージェント発の同文 fan-out も畳む", () => {
    const rows = collapseRows([
      message(agent("gemmy"), agent("a"), "はじめまして", 1),
      message(agent("gemmy"), agent("b"), "はじめまして", 1),
    ]);
    expect(rows).toHaveLength(1);
    expect(rows[0].extraTargets).toHaveLength(1);
  });

  it("内容が違えば畳まない", () => {
    const rows = collapseRows([
      message(agent("gemmy"), agent("a"), "A さんへ", 1),
      message(agent("gemmy"), agent("b"), "B さんへ", 1),
    ]);
    expect(rows).toHaveLength(2);
  });

  it("hop が違う言い直しは別の発話として残す", () => {
    const rows = collapseRows([
      message(agent("gemmy"), agent("a"), "確認してください", 1),
      message(agent("gemmy"), agent("b"), "確認してください", 3),
    ]);
    expect(rows).toHaveLength(2);
  });

  it("同じ宛先への送り直しは畳まない", () => {
    const rows = collapseRows([
      message(user, agent("a"), "届いてる？"),
      message(user, agent("a"), "届いてる？"),
    ]);
    expect(rows).toHaveLength(2);
  });

  it("間に別の発話が挟まれば独立した行になる", () => {
    const rows = collapseRows([
      message(user, agent("a"), "こんにちは"),
      message(agent("a"), user, "やあ", 1),
      message(user, agent("b"), "こんにちは"),
    ]);
    expect(rows).toHaveLength(3);
  });
});

describe("buildTimeline", () => {
  const run = (id: string, tool: string, tsMs: number, ok = true): ToolRun => ({
    id,
    agentId: "a",
    tool,
    ok,
    tsMs,
  });

  it("発話とツール実行を時刻順に 1 本へ畳む", () => {
    const rows = collapseRows([
      { ...message(user, agent("a"), "調べて"), tsMs: 100 },
      { ...message(agent("a"), user, "調べました"), tsMs: 300 },
    ]);
    const timeline = buildTimeline(rows, [run("t1", "grep", 200)]);

    expect(timeline.map((e) => e.kind)).toEqual(["message", "tool", "message"]);
  });

  it("同時刻ならツールを発話より前に置く（呼んでから答える）", () => {
    // 時刻の丸めで因果の順序をひっくり返さない。
    const rows = collapseRows([{ ...message(agent("a"), user, "答え"), tsMs: 500 }]);
    const timeline = buildTimeline(rows, [run("t1", "grep", 500)]);

    expect(timeline.map((e) => e.kind)).toEqual(["tool", "message"]);
  });

  it("ツール実行が無ければ発話だけが並ぶ", () => {
    const rows = collapseRows([message(user, agent("a"), "やあ")]);
    expect(buildTimeline(rows, [])).toHaveLength(1);
  });

  it("発話が無くてもツール実行だけで並ぶ", () => {
    // 応答生成中（まだ発話が確定していない）に見える状態。
    const timeline = buildTimeline([], [run("t1", "fd", 10), run("t2", "grep", 20)]);
    expect(timeline.map((e) => e.key)).toEqual(["t1", "t2"]);
  });

  it("キーは発話 ID とツール実行 ID をそのまま使う", () => {
    const rows = collapseRows([{ ...message(user, agent("a"), "やあ"), tsMs: 1 }]);
    const timeline = buildTimeline(rows, [run("t9", "sd", 2)]);
    expect(timeline[0].key).toBe(rows[0].message.id);
    expect(timeline[1].key).toBe("t9");
  });
});

describe("場からの告知の判定（吹き出しにしない行）", () => {
  const at = (from: Endpoint, to: Endpoint): AgentMessage => ({
    id: "m",
    from,
    to,
    content: "agent_01（ザリ）が起動しました",
    tokens: 0,
    tsMs: 0,
    hop: 0,
    coRecipients: [],
  });

  it("System → User は告知（細い行）", () => {
    expect(isSystemNotice(at({ kind: "system" }, { kind: "user" }))).toBe(true);
  });

  it("System → Agent は発話（吹き出しのまま）", () => {
    // 予定の発火。配送されてターンを起こすので、出来事の記録ではなく依頼そのもの。
    expect(
      isSystemNotice(at({ kind: "system" }, { kind: "agent", id: "agent_01" })),
    ).toBe(false);
  });

  it("System 発でなければ告知ではない", () => {
    expect(isSystemNotice(at({ kind: "user" }, { kind: "agent", id: "a" }))).toBe(false);
    expect(
      isSystemNotice(at({ kind: "agent", id: "a" }, { kind: "user" })),
    ).toBe(false);
  });
});
