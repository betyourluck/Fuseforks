import { describe, expect, it } from "vitest";

import {
  buildTimeline,
  collapseRows,
  formatToolArgs,
  groupToolRuns,
  isSystemNotice,
  reasonDisplay,
  TOOL_GROUP_MIN,
  toolGroupKey,
  type TimelineEntry,
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
    callId: 0,
    agentId: "a",
    tool,
    ok,
    reason: { kind: "omitted" },
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

describe("外部クライアントの発話（Spec 25）", () => {
  const external = (client: string): Endpoint => ({ kind: "external", client });

  it("外部からの依頼は場からの告知ではない（吹き出しのまま）", () => {
    expect(isSystemNotice(message(external("Claude Code"), agent("a"), "外からの依頼"))).toBe(
      false,
    );
  });

  it("**別のクライアントは別の送り手として扱う** — 種別だけで畳まない", () => {
    // **宛先を変えるのが要点。** 同じ宛先にすると `foldsInto` の宛先重複の
    // 判定で先に落ち、送り手を比べていなくてもテストが通ってしまう
    // （最初に書いた版がこれで、ミューテーションで気づいた）。
    const rows = collapseRows([
      message(external("Claude Code"), agent("a"), "同じ本文"),
      message(external("Copilot"), agent("b"), "同じ本文"),
    ]);
    expect(rows).toHaveLength(2);
  });

  it("同じクライアントの同報は 1 行に畳む（user と同じ規律）", () => {
    const rows = collapseRows([
      message(external("Claude Code"), agent("a"), "同じ本文"),
      message(external("Claude Code"), agent("b"), "同じ本文"),
    ]);
    expect(rows).toHaveLength(1);
    expect(rows[0].extraTargets).toEqual([agent("b")]);
  });
});

describe("reasonDisplay", () => {
  it("書かれた理由は本文をそのまま返す", () => {
    expect(reasonDisplay({ kind: "written", text: "綴りを確かめるため" })).toEqual({
      kind: "text",
      text: "綴りを確かめるため",
    });
  });

  it("書かれなかったときは辞書の鍵を返す（純関数は訳語を知らない）", () => {
    // `batchLabel` が titleKey を返すのと同じ形。訳語をここで組むと、
    // 表示規則が i18n の外側と内側に割れる。
    expect(reasonDisplay({ kind: "omitted" })).toEqual({
      kind: "labelKey",
      key: "chat.reasonOmitted",
    });
  });

  it("外部ツールと対象外を同じ扱いにしない", () => {
    // **ここを畳むと `ask_agent_3` に「外部ツール」と出て嘘になる。**
    // 片方だけを見ると、両方 null を返す実装でも両方 labelKey を返す実装でも
    // 通ってしまうので、**対で見る**。
    expect(reasonDisplay({ kind: "unsupported" })).toEqual({
      kind: "labelKey",
      key: "chat.reasonUnsupported",
    });
    expect(reasonDisplay({ kind: "excluded" })).toBeNull();
  });

  it("行を出さないことと空文字は別物", () => {
    // 空文字を返すと「理由が空である」という別の主張になり、
    // ask の行に「意図: 」だけが残る。
    const display = reasonDisplay({ kind: "excluded" });
    expect(display).toBeNull();
    expect(display).not.toEqual({ kind: "text", text: "" });
  });
});

describe("groupToolRuns（Spec 57 D8）", () => {
  let next = 0;
  /** ツール行 1 本。`callId` は通し番号。 */
  const tool = (agentId: string, ok = true): TimelineEntry => {
    next += 1;
    return {
      kind: "tool",
      key: `tool-${next}`,
      run: { id: `tool-${next}`, callId: next, agentId, tool: "grep", ok, reason: { kind: "omitted" }, tsMs: next },
    };
  };
  /** 発話 1 通。 */
  const said = (from: Endpoint, to: Endpoint): TimelineEntry => {
    const m = message(from, to, "…");
    return { kind: "message", key: m.id, row: { message: m, extraTargets: [] } };
  };
  const kinds = (entries: ReturnType<typeof groupToolRuns>) => entries.map((e) => e.kind);
  const idle = new Set<string>();
  const none = new Set<string>();

  it("同じ個体の連続 3 本以上を、既定で 1 行へ畳む", () => {
    const entries = [tool("a"), tool("a"), tool("a"), said(agent("a"), user)];
    const out = groupToolRuns(entries, idle, none);
    expect(kinds(out)).toEqual(["toolGroup", "message"]);
    expect(out[0]).toMatchObject({ agentId: "a", count: 3, open: false });
  });

  it(`${TOOL_GROUP_MIN - 1} 本までは束ねない`, () => {
    const out = groupToolRuns([tool("a"), tool("a"), said(agent("a"), user)], idle, none);
    expect(kinds(out)).toEqual(["tool", "tool", "message"]);
  });

  it("処理中の個体の行は束ねない（いま何をしているかが見える）", () => {
    const entries = [tool("a"), tool("a"), tool("a"), tool("a")];
    expect(kinds(groupToolRuns(entries, new Set(["a"]), none))).toEqual(["tool", "tool", "tool", "tool"]);
    // 同じ並びでも、ターンが終われば畳まれる。
    expect(kinds(groupToolRuns(entries, idle, none))).toEqual(["toolGroup"]);
  });

  it("後ろに別の個体の行が来ても、答えていない個体の行は畳まれない", () => {
    // 波の最中: a が 3 本呼んだ後に b の行が来たが、a はまだ処理中。
    const entries = [tool("a"), tool("a"), tool("a"), tool("b")];
    const out = groupToolRuns(entries, new Set(["a", "b"]), none);
    expect(kinds(out)).toEqual(["tool", "tool", "tool", "tool"]);
  });

  it("別の個体の行と発話でまとまりが切れる", () => {
    const entries = [tool("a"), tool("a"), tool("b"), tool("a"), said(user, agent("a")), tool("a"), tool("a")];
    // a の行は 5 本あるが、連続は 2 / 1 / 2 なのでどれも束ねない。
    expect(kinds(groupToolRuns(entries, idle, none)).filter((k) => k === "toolGroup")).toEqual([]);
  });

  it("ok=false を 1 本でも含むまとまりは束ねない（赤いドットを見出しの裏へ隠さない）", () => {
    const entries = [tool("a"), tool("a", false), tool("a"), tool("a")];
    expect(kinds(groupToolRuns(entries, idle, none))).toEqual(["tool", "tool", "tool", "tool"]);
  });

  it("開いたまとまりは、見出しの後ろに行が並ぶ", () => {
    const entries = [tool("a"), tool("a"), tool("a")];
    const first = entries[0];
    if (first.kind !== "tool") throw new Error("unreachable");
    const out = groupToolRuns(entries, idle, new Set([toolGroupKey(first.run)]));
    expect(kinds(out)).toEqual(["toolGroup", "tool", "tool", "tool"]);
    expect(out[0]).toMatchObject({ open: true, count: 3 });
  });

  it("行が増えても、まとまりの鍵は動かない（開いた状態が保たれる）", () => {
    const entries = [tool("a"), tool("a"), tool("a")];
    const before = groupToolRuns(entries, idle, none)[0];
    const after = groupToolRuns([...entries, tool("a"), tool("a")], idle, none)[0];
    expect(after.key).toBe(before.key);
    expect(after).toMatchObject({ count: 5 });
  });

  it("見出しはエラーの本数を持たない（「エラー 0 件」を正常終了と読ませない）", () => {
    const out = groupToolRuns([tool("a"), tool("a"), tool("a")], idle, none)[0];
    expect(Object.keys(out).sort()).toEqual(["agentId", "count", "key", "kind", "open"]);
  });
});

describe("formatToolArgs（Spec 57 D5）", () => {
  it("切っていない引数は字下げして出す", () => {
    expect(formatToolArgs({ pattern: "fn main" }, false)).toBe('{\n  "pattern": "fn main"\n}');
  });

  it("切った引数は整形せず、そのまま出す", () => {
    const head = '{"op":"write","content":"字字字';
    expect(formatToolArgs(head, true)).toBe(head);
  });

  it("形は旗で決める — 切っていない文字列の引数は JSON の文字列として出す", () => {
    // typeof で分岐すると、これが「切った引数」として引用符なしで出る。
    expect(formatToolArgs("plain", false)).toBe('"plain"');
  });
});
