import { describe, expect, it } from "vitest";

import { collapseRows } from "./chatRows";
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
