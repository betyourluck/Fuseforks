import { describe, expect, it } from "vitest";

import {
  classifyNote,
  laneNotes,
  ownerNameOf,
  SUMMARY_NOTE,
  type LaneAgent,
  type LaneContext,
  type LaneWave,
} from "./blackboardLanes";

const DIR = "D:\\Outcasts.jp";
const OTHER = "D:\\ManualeRAG";

function agent(name: string, over: Partial<LaneAgent> = {}): LaneAgent {
  return { id: `id-${name}`, name, status: "running", workDir: DIR, ...over };
}

function wave(agentId: string, over: Partial<LaneWave> = {}): LaneWave {
  return { agentId, state: "dispatched", bundleChars: null, tasks: [], ...over };
}

function ctx(over: Partial<LaneContext> = {}): LaneContext {
  return { agents: [], typing: {}, waves: [], ...over };
}

const note = (name: string, dir = DIR) => ({ dir, name });

describe("blackboardLanes: 持ち主の同定", () => {
  it("ファイル名の .md を落とした表示名で引く", () => {
    expect(ownerNameOf("ザリ.md")).toBe("ザリ");
    expect(ownerNameOf("memo")).toBe("memo");
  });

  it("表示名の一致する個体が居なければ孤児（unknown）で手が離れている", () => {
    const r = classifyNote(note("誰か.md"), ctx({ agents: [agent("ザリ")] }));
    expect(r).toEqual({ lane: "released", owner: null, reason: "orphanUnknown" });
  });

  it("居ても付箋の work_dir を向いていなければ孤児（moved）— 8/11 の形", () => {
    const gemmy = agent("ジェミー", { workDir: OTHER });
    const r = classifyNote(note("ジェミー.md"), ctx({ agents: [gemmy], typing: { [gemmy.id]: true } }));
    expect(r.lane).toBe("released");
    expect(r.reason).toBe("orphanMoved");
    expect(r.owner).toBe(gemmy);
  });

  it("dir は持ち主を決めない — 同じ work_dir の別個体は名前で分かれる", () => {
    const zari = agent("ザリ");
    const luna = agent("ルナ", { status: "idle" });
    const c = ctx({ agents: [zari, luna], typing: { [zari.id]: true } });
    expect(classifyNote(note("ザリ.md"), c).lane).toBe("active");
    expect(classifyNote(note("ルナ.md"), c).lane).toBe("released");
  });
});

describe("blackboardLanes: 3 列の判定", () => {
  it("ターンの最中なら動いている", () => {
    const zari = agent("ザリ");
    const r = classifyNote(note("ザリ.md"), ctx({ agents: [zari], typing: { [zari.id]: true } }));
    expect(r).toEqual({ lane: "active", owner: zari, reason: null });
  });

  it("進行役の波が未確定なら typing で無くても動いている", () => {
    const zari = agent("ザリ");
    const r = classifyNote(note("ザリ.md"), ctx({ agents: [zari], waves: [wave(zari.id)] }));
    expect(r.lane).toBe("active");
  });

  it("ワーカーとして running のタスクを持つ波があれば動いている（配送待ちで typing 前）", () => {
    const zari = agent("ザリ");
    const muse = agent("ミュゼ");
    const w = wave(zari.id, { tasks: [{ to: muse.id, state: "running" }] });
    expect(classifyNote(note("ミュゼ.md"), ctx({ agents: [zari, muse], waves: [w] })).lane).toBe("active");
  });

  it("波が完了していれば（bundleChars あり）動いているとは読まない", () => {
    const zari = agent("ザリ");
    const w = wave(zari.id, { bundleChars: 1200, tasks: [{ to: "x", state: "answered" }] });
    expect(classifyNote(note("ザリ.md"), ctx({ agents: [zari], waves: [w] })).reason).toBe("waiting");
  });

  it("持ち主の波が pending なら、typing 中でもあなたの手番が勝つ", () => {
    const zari = agent("ザリ");
    const c = ctx({ agents: [zari], typing: { [zari.id]: true }, waves: [wave(zari.id, { state: "pending" })] });
    expect(classifyNote(note("ザリ.md"), c).lane).toBe("yourTurn");
  });

  it("pending の波はワーカー側の付箋には効かない（手番は進行役のもの）", () => {
    const zari = agent("ザリ");
    const muse = agent("ミュゼ", { status: "idle" });
    const w = wave(zari.id, { state: "pending", tasks: [{ to: muse.id, state: "running" }] });
    expect(classifyNote(note("ミュゼ.md"), ctx({ agents: [zari, muse], waves: [w] })).lane).toBe("released");
  });

  it("discarded の波は動いているにも手番にも数えない", () => {
    const zari = agent("ザリ", { status: "idle" });
    const w = wave(zari.id, { state: "discarded" });
    expect(classifyNote(note("ザリ.md"), ctx({ agents: [zari], waves: [w] }))).toEqual({
      lane: "released",
      owner: zari,
      reason: "stopped",
    });
  });

  it("手が離れている内訳は稼働状態で分かれる", () => {
    const reasonOf = (status: LaneAgent["status"]) =>
      classifyNote(note("A.md"), ctx({ agents: [agent("A", { status })] })).reason;
    expect(reasonOf("idle")).toBe("stopped");
    expect(reasonOf("stopping")).toBe("stopped");
    expect(reasonOf("failed")).toBe("failed");
    expect(reasonOf("running")).toBe("waiting");
    expect(reasonOf("starting")).toBe("waiting");
  });
});

describe("blackboardLanes: 一覧の振り分け", () => {
  it("まとめ.md は列に入れず summary へ、残りはコアの並びを保つ", () => {
    const zari = agent("ザリ");
    const luna = agent("ルナ", { status: "idle" });
    const board = laneNotes(
      [note(SUMMARY_NOTE), note("ザリ.md"), note("ルナ.md")],
      ctx({ agents: [zari, luna], typing: { [zari.id]: true } }),
    );
    expect(board.summary.map((n) => n.name)).toEqual([SUMMARY_NOTE]);
    expect(board.lanes.active.map((n) => n.name)).toEqual(["ザリ.md"]);
    expect(board.lanes.yourTurn).toEqual([]);
    expect(board.lanes.released.map((n) => n.name)).toEqual(["ルナ.md"]);
  });

  it("手が離れている列では孤児を先頭へ寄せ、それ以外は元の並び", () => {
    const a = agent("A", { status: "idle" });
    const c = agent("C", { status: "failed" });
    const moved = agent("M", { workDir: OTHER });
    const board = laneNotes(
      [note("A.md"), note("B.md"), note("C.md"), note("M.md")],
      ctx({ agents: [a, c, moved] }),
    );
    expect(board.lanes.released.map((n) => `${n.name}:${n.info.reason}`)).toEqual([
      "B.md:orphanUnknown",
      "M.md:orphanMoved",
      "A.md:stopped",
      "C.md:failed",
    ]);
  });

  it("空の一覧は 3 列とも空で、列の集合は 3 つのまま", () => {
    const board = laneNotes([], ctx());
    expect(Object.keys(board.lanes).sort()).toEqual(["active", "released", "yourTurn"]);
    expect(board.summary).toEqual([]);
  });
});
