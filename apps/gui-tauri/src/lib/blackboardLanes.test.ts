import { describe, expect, it } from "vitest";

import {
  badgeOf,
  classifyNote,
  kanbanNotes,
  noteHeading,
  splitNoteName,
  stateDictKey,
  STATES,
  type LaneAgent,
  type LaneContext,
  type LaneWave,
} from "./blackboardLanes";

const DIR = "D:\\Outcasts.jp";
const OTHER = "D:\\ManualeRAG";

/** id は表示名と**違う綴り**にする — 同じだと「名前で引いても通る」実装を見分けられない。 */
function agent(name: string, over: Partial<LaneAgent> = {}): LaneAgent {
  return { id: `agent_${name}`, name, status: "running", workDir: DIR, ...over };
}

/** ツールが書く形のファイル名（`<agent_id> - <仕事名>.md`。Spec 55 D2）。 */
const own = (a: LaneAgent, task = "調査") => `${a.id} - ${task}.md`;

function wave(agentId: string, over: Partial<LaneWave> = {}): LaneWave {
  return { agentId, state: "dispatched", bundleChars: null, tasks: [], ...over };
}

function ctx(over: Partial<LaneContext> = {}): LaneContext {
  return { agents: [], typing: {}, waves: [], ...over };
}

const note = (name: string, state?: string, dir = DIR) =>
  state === undefined ? { dir, name } : { dir, name, state };

describe("blackboardLanes: 持ち主の同定", () => {
  it("区切りは最初の ` - `。仕事名の中の ` - ` は仕事名の一部（コアの split_note_name と同じ）", () => {
    expect(splitNoteName("agent_2 - 調査.md")).toEqual({ key: "agent_2", task: "調査" });
    expect(splitNoteName("agent_2 - 調査 - 続き.md")).toEqual({ key: "agent_2", task: "調査 - 続き" });
    // 空白の無いハイフンは区切りではない。
    expect(splitNoteName("agent-3.md")).toEqual({ key: "agent-3", task: null });
  });

  it("持ち主は id で引く。表示名で名付けた付箋（Spec 54 の形）は孤児", () => {
    const zari = agent("ザリ");
    const c = ctx({ agents: [zari], typing: { [zari.id]: true } });
    expect(classifyNote(note(own(zari)), c).owner).toBe(zari);
    expect(classifyNote(note("ザリ - 調査.md"), c)).toEqual({
      lane: "released",
      owner: null,
      reason: "orphanUnknown",
    });
  });

  it("区切りの無い旧形式は、stem が id と同じ綴りでも孤児（コアの掃除と同じ判定）", () => {
    const zari = agent("ザリ");
    const r = classifyNote(note(`${zari.id}.md`), ctx({ agents: [zari] }));
    expect(r).toEqual({ lane: "released", owner: null, reason: "orphanUnknown" });
  });

  it("改名しても付箋は持ち主から外れない（検収 5）", () => {
    const before = agent("ザリ");
    const after = { ...before, name: "ザリ・ロブステル" };
    expect(classifyNote(note(own(before)), ctx({ agents: [after] })).owner).toBe(after);
  });

  it("見出しは「表示名 - 仕事名」。引けなければファイル名のまま", () => {
    const zari = agent("ザリ");
    expect(noteHeading(own(zari, "調査 - 続き"), zari)).toBe("ザリ - 調査 - 続き");
    expect(noteHeading("ザリ - 調査.md", null)).toBe("ザリ - 調査.md");
    expect(noteHeading(`${zari.id}.md`, zari)).toBe(`${zari.id}.md`);
  });

  it("居ても付箋の work_dir を向いていなければ孤児（moved）— 8/11 の形", () => {
    const gemmy = agent("ジェミー", { workDir: OTHER });
    const r = classifyNote(note(own(gemmy)), ctx({ agents: [gemmy], typing: { [gemmy.id]: true } }));
    expect(r.lane).toBe("released");
    expect(r.reason).toBe("orphanMoved");
    expect(r.owner).toBe(gemmy);
  });

  it("dir は持ち主を決めない — 同じ work_dir の別個体は id で分かれる", () => {
    const zari = agent("ザリ");
    const luna = agent("ルナ", { status: "idle" });
    const c = ctx({ agents: [zari, luna], typing: { [zari.id]: true } });
    expect(classifyNote(note(own(zari)), c).lane).toBe("active");
    expect(classifyNote(note(own(luna)), c).lane).toBe("released");
  });
});

describe("blackboardLanes: 手番の判定（バッジ）", () => {
  it("ターンの最中なら動いている", () => {
    const zari = agent("ザリ");
    const r = classifyNote(note(own(zari)), ctx({ agents: [zari], typing: { [zari.id]: true } }));
    expect(r).toEqual({ lane: "active", owner: zari, reason: null });
    expect(badgeOf(r)).toBe("active");
  });

  it("進行役の波が未確定なら typing で無くても動いている", () => {
    const zari = agent("ザリ");
    const r = classifyNote(note(own(zari)), ctx({ agents: [zari], waves: [wave(zari.id)] }));
    expect(r.lane).toBe("active");
  });

  it("ワーカーとして running のタスクを持つ波があれば動いている（配送待ちで typing 前）", () => {
    const zari = agent("ザリ");
    const muse = agent("ミュゼ");
    const w = wave(zari.id, { tasks: [{ to: muse.id, state: "running" }] });
    expect(classifyNote(note(own(muse)), ctx({ agents: [zari, muse], waves: [w] })).lane).toBe("active");
  });

  it("波が完了していれば（bundleChars あり）動いているとは読まない", () => {
    const zari = agent("ザリ");
    const w = wave(zari.id, { bundleChars: 1200, tasks: [{ to: "x", state: "answered" }] });
    expect(classifyNote(note(own(zari)), ctx({ agents: [zari], waves: [w] })).reason).toBe("waiting");
  });

  it("持ち主の波が pending なら、typing 中でもあなたの手番が勝つ（バッジの優先）", () => {
    const zari = agent("ザリ");
    const c = ctx({ agents: [zari], typing: { [zari.id]: true }, waves: [wave(zari.id, { state: "pending" })] });
    const r = classifyNote(note(own(zari)), c);
    expect(r.lane).toBe("yourTurn");
    expect(badgeOf(r)).toBe("yourTurn");
  });

  it("pending の波はワーカー側の付箋には効かない（手番は進行役のもの）", () => {
    const zari = agent("ザリ");
    const muse = agent("ミュゼ", { status: "idle" });
    const w = wave(zari.id, { state: "pending", tasks: [{ to: muse.id, state: "running" }] });
    expect(classifyNote(note(own(muse)), ctx({ agents: [zari, muse], waves: [w] })).lane).toBe("released");
  });

  it("discarded の波は動いているにも手番にも数えない", () => {
    const zari = agent("ザリ", { status: "idle" });
    const w = wave(zari.id, { state: "discarded" });
    expect(classifyNote(note(own(zari)), ctx({ agents: [zari], waves: [w] }))).toEqual({
      lane: "released",
      owner: zari,
      reason: "stopped",
    });
  });

  it("手が離れているときのバッジは稼働状態の内訳で分かれる", () => {
    const badge = (status: LaneAgent["status"]) => {
      const a = agent("A", { status });
      return badgeOf(classifyNote(note(own(a)), ctx({ agents: [a] })));
    };
    expect(badge("idle")).toBe("stopped");
    expect(badge("stopping")).toBe("stopped");
    expect(badge("failed")).toBe("failed");
    expect(badge("running")).toBe("waiting");
    expect(badge("starting")).toBe("waiting");
  });
});

describe("blackboardLanes: 列（仕事の状態）", () => {
  it("列順は STATES の順で固定 — コアの返却順（state の文字列順）ではない（凍結 6）", () => {
    // コアは state の文字列順で返す: doing, done, on-hold。
    const fromCore = ["doing", "done", "on-hold"].map((s) =>
      note(`ザリ - ${s}.md`, s),
    );
    const board = kanbanNotes(fromCore, ctx({ agents: [agent("ザリ")] }));
    expect(board.columns.map((c) => c.state)).toEqual([...STATES]);
    expect(STATES).toEqual(["doing", "on-hold", "done"]);
  });

  it("3 つの列は付箋が 0 枚でも出る。状態なし・その他は付箋があるときだけ", () => {
    const empty = kanbanNotes([], ctx());
    expect(empty.columns.map((c) => c.kind)).toEqual(["state", "state", "state"]);

    const withUnfiled = kanbanNotes([note("agent_1 - x.md")], ctx());
    expect(withUnfiled.columns.map((c) => c.kind)).toEqual(["state", "state", "state", "unfiled"]);
    // 直下の付箋は「状態なし」にだけ入り、どの状態の列にも漏れない（`doing` へ倒さない）。
    expect(withUnfiled.columns.map((c) => c.notes.length)).toEqual([0, 0, 0, 1]);
  });

  it("3 値の外のフォルダはフォルダごとに 1 列（フォルダ名順）。見出しは名前を運ぶ", () => {
    const board = kanbanNotes(
      [note("x.md", "foo"), note("y.md", "bar"), note("z.md", "foo"), note("w.md")],
      ctx(),
    );
    const tail = board.columns.slice(STATES.length);
    expect(tail.map((c) => [c.kind, c.state, c.notes.length])).toEqual([
      ["unfiled", null, 1],
      ["other", "bar", 1],
      ["other", "foo", 2],
    ]);
  });

  it("まとめ.md に特別な扱いは無い — 直下は「状態なし」の列に入る孤児（Spec 55 D5）", () => {
    const board = kanbanNotes([note("まとめ.md"), note("まとめ.md", "doing")], ctx({ agents: [agent("ザリ")] }));
    expect(Object.keys(board)).toEqual(["columns"]);
    const where = board.columns.map((c) => [c.kind, c.state, c.notes.map((n) => n.marks.badge)]);
    expect(where).toEqual([
      ["state", "doing", ["orphanUnknown"]],
      ["state", "on-hold", []],
      ["state", "done", []],
      ["unfiled", null, ["orphanUnknown"]],
    ]);
  });

  it("列の中では孤児を先頭へ寄せ、それ以外はコアの並び。バッジの種類では並べ替えない（凍結 5）", () => {
    const a = agent("A", { status: "idle" });
    const c = agent("C");
    const moved = agent("M", { workDir: OTHER });
    const board = kanbanNotes(
      [
        note(own(a, "x"), "doing"),
        note("agent_B - x.md", "doing"),
        note(own(c, "x"), "doing"),
        note(own(moved, "x"), "doing"),
      ],
      ctx({ agents: [a, c, moved], typing: { [c.id]: true } }),
    );
    expect(board.columns[0].notes.map((n) => `${n.name}:${n.marks.badge}`)).toEqual([
      "agent_B - x.md:orphanUnknown",
      "agent_M - x.md:orphanMoved",
      "agent_A - x.md:stopped",
      "agent_C - x.md:active",
    ]);
  });

  it("同じ name が複数の場所に並ぶと両方に重複（凍結 7）。仕事名違いは重複ではない", () => {
    const board = kanbanNotes(
      [
        note("ザリ - A.md", "doing"),
        note("ザリ - A.md", "done"),
        note("ザリ - B.md", "doing"),
        note("ルナ - A.md"),
        note("ルナ - A.md", "on-hold"),
      ],
      ctx(),
    );
    const dup = new Map<string, boolean>();
    for (const c of board.columns) for (const n of c.notes) dup.set(`${n.state ?? ""}/${n.name}`, n.marks.duplicate);
    expect(dup.get("doing/ザリ - A.md")).toBe(true);
    expect(dup.get("done/ザリ - A.md")).toBe(true);
    expect(dup.get("doing/ザリ - B.md")).toBe(false);
    // 直下も 1 つの場所として数える。
    expect(dup.get("/ルナ - A.md")).toBe(true);
    expect(dup.get("on-hold/ルナ - A.md")).toBe(true);
  });

  it("別の work_dir の同名は重複ではない（dir が違えば別の黒板）", () => {
    const board = kanbanNotes(
      [note("ザリ - A.md", "doing"), note("ザリ - A.md", "done", OTHER)],
      ctx(),
    );
    for (const c of board.columns) for (const n of c.notes) expect(n.marks.duplicate).toBe(false);
  });

  it("辞書の鍵はフォルダ名の `-` を camelCase へ写す", () => {
    expect(STATES.map(stateDictKey)).toEqual(["doing", "onHold", "done"]);
    expect(stateDictKey("unfiled")).toBe("unfiled");
  });
});
