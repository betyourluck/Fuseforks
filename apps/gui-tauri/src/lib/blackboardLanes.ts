/**
 * 黒板の付箋を**仕事の状態（フォルダ）で列に**分け、**持ち主の手番をバッジに**する（Spec 54）。
 *
 * 2026-09-16 の版は持ち主の手番（typing / status / 波 / workDir）から 3 列を派生させていた。
 * 2026-09-17 の Spec 54 で前提が「持ち主の状態」から「仕事の状態」へ動き、列は付箋の
 * 置き場（`blackboard/<state>/<表示名> - <仕事名>.md` の `state`）が決めるようになった。
 * 手番の判定 `classifyNote` は 1 行も変えず、返り値の使い方だけを列からバッジへ変えた —
 * 「状態は `doing` なのに持ち主は停止中」のような**宣言と観測の食い違い**を人が読むため。
 *
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

/** 進行役が束ねる付箋。列には入れず、常に上に固定する（条例の `まとめ.md`）。**直下のものだけ。** */
export const SUMMARY_NOTE = "まとめ.md";

/**
 * 表示名と仕事名の区切り（最初の 1 つ）。`ownerNameOf` が割り、条例へ挿入する節
 * （`blackboardOrdinance.ts`）が同じ綴りをサーヴァントへ伝える — 2 箇所に書かない。
 */
export const NOTE_SEPARATOR = " - ";

/**
 * 仕事の状態 = 状態フォルダの閉じた 3 値（Spec 54 凍結 1・rev3）。**並びが画面の列の並び**
 * （`doing → on-hold → done`）。コアの返却順は `state` の文字列順で
 * 別のもの — コアに列順を持たせると 3 値の順序がコアと辞書の 2 箇所に住む（凍結 6）。
 * rev2 の 5 値から `needs-you` と `waiting` を落とした（2026-09-17 実機）— 委譲は
 * ターンの中で同期的に待つので「待ちに入った」と宣言する時点が無く、人の手番は
 * Spec 43 の窓と `run.json` の pending が既に決めてバッジに出る。
 * 名前は英字（`blackboard/` を言語非依存にしたのと同じ規律）。表示は辞書が訳す。
 */
export const STATES = ["doing", "on-hold", "done"] as const;
export type BlackboardState = (typeof STATES)[number];

export function isKnownState(state: string): state is BlackboardState {
  return (STATES as readonly string[]).includes(state);
}

/**
 * 辞書の鍵（`blackboard.state.*` / `blackboard.stateTitle.*`）。フォルダ名の `-` は
 * vue-i18n の鍵に置かず camelCase へ写す（`on-hold` → `onHold`）。
 */
export function stateDictKey(state: BlackboardState | "unfiled" | "other" | "summary"): string {
  return state.replace(/-([a-z])/g, (_, c: string) => c.toUpperCase());
}

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
 * 持ち主の手番。2026-09-16 の 3 列の名前をそのまま持つ（列ではなくバッジの判定になった）。
 * - `active`   — 持ち主がターンの最中、または未確定の波に参加している
 * - `yourTurn` — 持ち主の波が人の確認待ち（Spec 43）
 * - `released` — 持ち主の手が離れている（停止中 / 失敗 / 起動しているが待機 / 孤児）
 */
export type BlackboardLane = "active" | "yourTurn" | "released";

/**
 * `released` の内訳。バッジになる。
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

/**
 * 付箋のファイル名から持ち主の表示名を取る。
 * `ザリ.md` → `ザリ`（旧形式 = 仕事名なし）/ `ザリ - 調査 - 続き.md` → `ザリ`。
 * **区切りは最初の ` - `**（半角空白 + ハイフン + 半角空白）。仕事名の中の ` - ` は
 * 仕事名の一部（Spec 54 凍結 3）。
 */
export function ownerNameOf(noteName: string): string {
  const stem = noteName.endsWith(".md") ? noteName.slice(0, -3) : noteName;
  const cut = stem.indexOf(NOTE_SEPARATOR);
  return cut === -1 ? stem : stem.slice(0, cut);
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

/** 付箋 1 枚の手番を決める。優先は `yourTurn` > `active` > `released`。 */
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

/**
 * 見出し行に出すバッジ（1 枚に 1 つ）。`yourTurn` > `active` > `released` の優先は
 * **どのバッジを出すか**にだけ効き、列の中の並びには使わない（凍結 5）。
 * `released` は内訳（`reason`）で出す — 「手が離れている」という語は列ごと消えた。
 */
export type NoteBadge = "active" | "yourTurn" | ReleasedReason;

export function badgeOf(info: NoteLane): NoteBadge {
  return info.lane === "released" ? (info.reason ?? "stopped") : info.lane;
}

export function isOrphan(badge: NoteBadge): boolean {
  return badge === "orphanUnknown" || badge === "orphanMoved";
}

export interface NoteMarks {
  info: NoteLane;
  badge: NoteBadge;
  /**
   * 同じ `dir` で同じ `name`（表示名 + 仕事名の全体）が複数の場所に並んでいる
   * （凍結 7）。move ではなく write で別の状態に同名を作った事故の網。
   * `ザリ - A.md` と `ザリ - B.md` は別の仕事なので重複ではない。
   */
  duplicate: boolean;
}

export type MarkedNote<T> = T & { marks: NoteMarks };

/**
 * 画面の列。並びは `kanbanNotes` が決める:
 * 3 つの `state`（付箋が 0 枚でも出す）→ `unfiled`（直下。あるときだけ）→
 * `other`（3 値の外のフォルダ。**フォルダごとに 1 列**・フォルダ名順・あるときだけ）。
 */
export interface KanbanColumn<T> {
  kind: "state" | "unfiled" | "other";
  /** `state` / `other` のときフォルダ名。`unfiled` は null。 */
  state: string | null;
  notes: MarkedNote<T>[];
}

export interface KanbanBoard<T> {
  /** 直下の `まとめ.md`（work_dir ごとに最大 1 枚）。列の外で先頭に固定する。バッジ無し。 */
  summary: T[];
  columns: KanbanColumn<T>[];
}

interface NoteRef {
  dir: string;
  name: string;
  state?: string;
}

/** 列の中の並び: 孤児を先頭へ寄せ、残りはコアの並び（`name` 順）のまま。 */
function orphansFirst<T>(notes: MarkedNote<T>[]): MarkedNote<T>[] {
  return [
    ...notes.filter((n) => isOrphan(n.marks.badge)),
    ...notes.filter((n) => !isOrphan(n.marks.badge)),
  ];
}

/**
 * 一覧を列へ振り分け、各付箋にバッジを付ける。
 *
 * - 直下の `まとめ.md` だけが `summary`（`classifyNote` に掛けない）。`state` の中の
 *   `まとめ.md` は普通の付箋（持ち主「まとめ」= 孤児バッジ。凍結 9）
 * - 列は `state` から決まる。3 値の外のフォルダはフォルダごとに `other` の列
 * - 重複は `dir:name` が複数の場所（直下も 1 つの場所）に現れたとき
 */
export function kanbanNotes<T extends NoteRef>(
  notes: readonly T[],
  ctx: LaneContext,
): KanbanBoard<T> {
  const summary: T[] = [];
  const rest: T[] = [];
  for (const note of notes) {
    if (note.state === undefined && note.name === SUMMARY_NOTE) summary.push(note);
    else rest.push(note);
  }

  const places = new Map<string, Set<string>>();
  for (const note of rest) {
    const key = `${note.dir}:${note.name}`;
    const set = places.get(key) ?? new Set<string>();
    set.add(note.state ?? "");
    places.set(key, set);
  }

  const marked: MarkedNote<T>[] = rest.map((note) => {
    const info = classifyNote(note, ctx);
    return {
      ...note,
      marks: {
        info,
        badge: badgeOf(info),
        duplicate: (places.get(`${note.dir}:${note.name}`)?.size ?? 0) > 1,
      },
    };
  });

  const columns: KanbanColumn<T>[] = STATES.map((state) => ({
    kind: "state",
    state,
    notes: orphansFirst(marked.filter((n) => n.state === state)),
  }));

  const unfiled = orphansFirst(marked.filter((n) => n.state === undefined));
  if (unfiled.length > 0) columns.push({ kind: "unfiled", state: null, notes: unfiled });

  const others = [
    ...new Set(
      marked
        .map((n) => n.state)
        .filter((s): s is string => s !== undefined && !isKnownState(s)),
    ),
  ].sort();
  for (const state of others) {
    columns.push({
      kind: "other",
      state,
      notes: orphansFirst(marked.filter((n) => n.state === state)),
    });
  }

  return { summary, columns };
}
