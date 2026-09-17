import { describe, expect, it } from "vitest";

import {
  badgeOf,
  classifyNote,
  kanbanNotes,
  ownerNameOf,
  stateDictKey,
  STATES,
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

const note = (name: string, state?: string, dir = DIR) =>
  state === undefined ? { dir, name } : { dir, name, state };

describe("blackboardLanes: 持ち主の同定", () => {
  it("ファイル名の .md を落とした表示名で引く（旧形式 = 仕事名なし）", () => {
    expect(ownerNameOf("ザリ.md")).toBe("ザリ");
    expect(ownerNameOf("memo")).toBe("memo");
  });

  it("区切りは最初の ` - `。仕事名の中の ` - ` は仕事名の一部（Spec 54 凍結 3）", () => {
    expect(ownerNameOf("ザリ - 調査.md")).toBe("ザリ");
    expect(ownerNameOf("ザリ - 調査 - 続き.md")).toBe("ザリ");
    // 空白の無いハイフンは区切りではない（表示名に `-` を含む個体を割らない）。
    expect(ownerNameOf("ロボット-3号.md")).toBe("ロボット-3号");
  });

  it("表示名の一致する個体が居なければ孤児（unknown）", () => {
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
    expect(classifyNote(note("ザリ - 調査.md"), c).lane).toBe("active");
    expect(classifyNote(note("ルナ - 調査.md"), c).lane).toBe("released");
  });
});

describe("blackboardLanes: 手番の判定（バッジ）", () => {
  it("ターンの最中なら動いている", () => {
    const zari = agent("ザリ");
    const r = classifyNote(note("ザリ.md"), ctx({ agents: [zari], typing: { [zari.id]: true } }));
    expect(r).toEqual({ lane: "active", owner: zari, reason: null });
    expect(badgeOf(r)).toBe("active");
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

  it("持ち主の波が pending なら、typing 中でもあなたの手番が勝つ（バッジの優先）", () => {
    const zari = agent("ザリ");
    const c = ctx({ agents: [zari], typing: { [zari.id]: true }, waves: [wave(zari.id, { state: "pending" })] });
    const r = classifyNote(note("ザリ.md"), c);
    expect(r.lane).toBe("yourTurn");
    expect(badgeOf(r)).toBe("yourTurn");
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

  it("手が離れているときのバッジは稼働状態の内訳で分かれる", () => {
    const badge = (status: LaneAgent["status"]) =>
      badgeOf(classifyNote(note("A.md"), ctx({ agents: [agent("A", { status })] })));
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
    expect(empty.summary).toEqual([]);

    const withUnfiled = kanbanNotes([note("ザリ.md")], ctx());
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

  it("直下の まとめ.md だけが summary。状態の中の まとめ.md は普通の付箋で孤児バッジ（凍結 9）", () => {
    const board = kanbanNotes(
      [note(SUMMARY_NOTE), note(SUMMARY_NOTE, "doing")],
      ctx({ agents: [agent("ザリ")] }),
    );
    expect(board.summary.map((n) => n.name)).toEqual([SUMMARY_NOTE]);
    const doing = board.columns[0];
    expect(doing.state).toBe("doing");
    expect(doing.notes.map((n) => [n.name, n.marks.badge])).toEqual([[SUMMARY_NOTE, "orphanUnknown"]]);
  });

  it("列の中では孤児を先頭へ寄せ、それ以外はコアの並び。バッジの種類では並べ替えない（凍結 5）", () => {
    const a = agent("A", { status: "idle" });
    const c = agent("C");
    const moved = agent("M", { workDir: OTHER });
    const board = kanbanNotes(
      [
        note("A - x.md", "doing"),
        note("B - x.md", "doing"),
        note("C - x.md", "doing"),
        note("M - x.md", "doing"),
      ],
      ctx({ agents: [a, c, moved], typing: { [c.id]: true } }),
    );
    expect(board.columns[0].notes.map((n) => `${n.name}:${n.marks.badge}`)).toEqual([
      "B - x.md:orphanUnknown",
      "M - x.md:orphanMoved",
      "A - x.md:stopped",
      "C - x.md:active",
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
