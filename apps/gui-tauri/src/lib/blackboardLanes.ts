/**
 * 黒板の付箋を「持ち主の手番」で 3 列に分ける（2026-09-16）。
 *
 * 付箋は仕事のカードではなく**サーヴァント 1 体の作業ログ**（条例の 1 人 1 ファイル
 * `blackboard/<表示名>.md`）なので、列の状態は付箋ではなく**持ち主の状態から派生**させる。
 * 付箋には状態の欄を 1 つも書かせない — 人待ちは既に機構（Spec 43 の窓・Spec 20 の
 * pending）で決まっており、サーヴァントに申告させると真実が 2 つになる。
 *
 * 判定材料はフロントの投影に元からある 4 つだけ:
 * - `typing[agentId]` — ターンの最中か
 * - `AgentSnapshot.status` — 起動しているか
 * - `PlanWaveRecord.state === "pending"` — 人の手番の波があるか
 * - `AgentSnapshot.workDir` と付箋の `dir` — 持ち主が**この** work_dir を向いているか
 *
 * 持ち主の同定は表示名の一致に寄る（条例がそう決めているだけで、機構の保証ではない）。
 * `dir` は持ち主を決められない — 実機では 10 体中 9 体が同じ work_dir を向いている。
 * `dir` が担うのは「その名前の個体がこの work_dir を向いているか」の判定だけで、
 * 向いていなければ**孤児**（work_dir を移した個体の付箋。本人はもう消せない）。
 */

import type { AgentStatus, PlanTaskState, PlanWaveState } from "../types";

/** 進行役が束ねる付箋。列には入れず、常に上に固定する（条例の `まとめ.md`）。 */
export const SUMMARY_NOTE = "まとめ.md";

/**
 * 3 列。並びは画面の並びそのもの（上から）。
 * - `active`   — 持ち主がターンの最中、または未確定の波に参加している
 * - `yourTurn` — 持ち主の波が人の確認待ち（Spec 43）
 * - `released` — 持ち主の手が離れている（停止中 / 失敗 / 起動しているが待機 / 孤児）
 */
export type BlackboardLane = "active" | "yourTurn" | "released";
export const LANES: readonly BlackboardLane[] = ["active", "yourTurn", "released"];

export interface LaneAgent {
  id: string;
  name: string;
  status: AgentStatus;
  workDir: string | null;
}

export interface LaneWave {
  agentId: string;
  state: PlanWaveState;
  bundleChars: number | null;
  tasks: readonly { to: string; state: PlanTaskState }[];
}

export interface LaneContext {
  agents: readonly LaneAgent[];
  typing: Readonly<Record<string, true>>;
  waves: readonly LaneWave[];
}

/**
 * 「手が離れている」の内訳。列の中でバッジになる。
 * - `orphanUnknown` — 表示名の一致する個体が村に居ない
 * - `orphanMoved`   — 居るが、この付箋の work_dir を向いていない（消せるのは人だけ）
 * - `failed` / `stopped` / `waiting` — 持ち主の稼働状態
 */
export type ReleasedReason = "orphanUnknown" | "orphanMoved" | "failed" | "stopped" | "waiting";

export interface NoteLane {
  lane: BlackboardLane;
  /** 表示名で引けた持ち主。孤児（unknown）なら null。 */
  owner: LaneAgent | null;
  /** `released` のときだけ埋まる。 */
  reason: ReleasedReason | null;
}

/** 付箋のファイル名から持ち主の表示名を取る（`ザリ.md` → `ザリ`）。 */
export function ownerNameOf(noteName: string): string {
  return noteName.endsWith(".md") ? noteName.slice(0, -3) : noteName;
}

function releasedReason(status: AgentStatus): ReleasedReason {
  switch (status) {
    case "failed":
      return "failed";
    case "running":
    case "starting":
      return "waiting";
    case "idle":
    case "stopping":
      return "stopped";
  }
}

/** 付箋 1 枚の列を決める。優先は `yourTurn` > `active` > `released`。 */
export function classifyNote(note: { dir: string; name: string }, ctx: LaneContext): NoteLane {
  const stem = ownerNameOf(note.name);
  const owner = ctx.agents.find((a) => a.name === stem) ?? null;
  if (owner === null) return { lane: "released", owner: null, reason: "orphanUnknown" };
  if (owner.workDir !== note.dir) return { lane: "released", owner, reason: "orphanMoved" };

  const ownWaves = ctx.waves.filter((w) => w.agentId === owner.id);
  if (ownWaves.some((w) => w.state === "pending")) {
    return { lane: "yourTurn", owner, reason: null };
  }

  const inFlight =
    ctx.typing[owner.id] === true ||
    ctx.waves.some(
      (w) =>
        w.state === "dispatched" &&
        w.bundleChars === null &&
        (w.agentId === owner.id ||
          w.tasks.some((t) => t.to === owner.id && t.state === "running")),
    );
  if (inFlight) return { lane: "active", owner, reason: null };

  return { lane: "released", owner, reason: releasedReason(owner.status) };
}

export type LanedNote<T> = T & { info: NoteLane };

export interface LanedBoard<T> {
  /** `まとめ.md`（work_dir ごとに最大 1 枚）。列の外で先頭に固定する。 */
  summary: T[];
  lanes: Record<BlackboardLane, LanedNote<T>[]>;
}

/**
 * 一覧を 3 列へ振り分ける。コアの並び（`まとめ.md` → 名前順）は列の中でも保つ。
 * 例外は `released` だけで、**孤児を先頭へ**寄せる — 消す判断が最も軽い付箋を
 * 最初に見せる（8/11 に人が手で全消しした経路そのもの）。
 */
export function laneNotes<T extends { dir: string; name: string }>(
  notes: readonly T[],
  ctx: LaneContext,
): LanedBoard<T> {
  const board: LanedBoard<T> = {
    summary: [],
    lanes: { active: [], yourTurn: [], released: [] },
  };
  for (const note of notes) {
    if (note.name === SUMMARY_NOTE) {
      board.summary.push(note);
      continue;
    }
    const info = classifyNote(note, ctx);
    board.lanes[info.lane].push({ ...note, info });
  }
  const orphan = (n: LanedNote<T>) =>
    n.info.reason === "orphanUnknown" || n.info.reason === "orphanMoved";
  board.lanes.released = [
    ...board.lanes.released.filter(orphan),
    ...board.lanes.released.filter((n) => !orphan(n)),
  ];
  return board;
}
