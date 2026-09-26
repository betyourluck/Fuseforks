/**
 * 黒板タブの持ち主での絞り込みと、まとめて畳む判定（2026-09-27 利用者要望
 * 「黒板にすべて畳むを」「エージェント名でフィルタできるように」）。
 *
 * **絞り込むのは表示だけ。** 列（3 つの状態 + 状態なし + その他）の形は変えず、
 * 列の中の付箋だけを減らす — 列の形が毎回同じであることが情報、という黒板の規律
 * （2026-09-16）を絞り込み中も保つ。
 *
 * 選べる持ち主は**いま黒板に付箋がある個体だけ**（村の全員を並べると、選んでも
 * 何も出ない項目が並ぶ）。持ち主が引けない付箋（`orphanUnknown` = 区切りの無い名前 /
 * 削除された個体の id）は 1 つの項目「持ち主不明」に束ねる。
 */

import type { KanbanColumn, LaneAgent, MarkedNote } from "./blackboardLanes";

/** 絞り込みの値。`all` = 絞らない / `unowned` = 持ち主が引けない付箋 / それ以外は agent id。 */
export type OwnerFilter = "all" | "unowned" | (string & {});

export interface OwnerOptions {
  /** 付箋を持つ個体。表示名の順。 */
  owners: Pick<LaneAgent, "id" | "name">[];
  /** 持ち主が引けない付箋があるか（「持ち主不明」の項目を出すか）。 */
  hasUnowned: boolean;
}

/** 黒板にいる持ち主を列挙する。 */
export function ownerOptions<T>(columns: readonly KanbanColumn<T>[]): OwnerOptions {
  const byId = new Map<string, Pick<LaneAgent, "id" | "name">>();
  let hasUnowned = false;
  for (const column of columns) {
    for (const note of column.notes) {
      const owner = note.marks.info.owner;
      if (owner === null) hasUnowned = true;
      else if (!byId.has(owner.id)) byId.set(owner.id, { id: owner.id, name: owner.name });
    }
  }
  const owners = [...byId.values()].sort((a, b) => a.name.localeCompare(b.name));
  return { owners, hasUnowned };
}

/**
 * 選んでいる値が今も選べるか。**選べなくなったら `all` へ戻す** — 持ち主の付箋が
 * 全部消えた（または個体が削除された）あとも絞り込みが残ると、黒板が理由なく空に見える。
 */
export function effectiveFilter(filter: OwnerFilter, options: OwnerOptions): OwnerFilter {
  if (filter === "all") return "all";
  if (filter === "unowned") return options.hasUnowned ? "unowned" : "all";
  return options.owners.some((o) => o.id === filter) ? filter : "all";
}

export function matchesOwner<T>(note: MarkedNote<T>, filter: OwnerFilter): boolean {
  if (filter === "all") return true;
  const owner = note.marks.info.owner;
  if (filter === "unowned") return owner === null;
  return owner !== null && owner.id === filter;
}

/** 列の形はそのまま、列の中の付箋だけを絞る。 */
export function filterColumns<T>(
  columns: readonly KanbanColumn<T>[],
  filter: OwnerFilter,
): KanbanColumn<T>[] {
  if (filter === "all") return [...columns];
  return columns.map((column) => ({
    ...column,
    notes: column.notes.filter((note) => matchesOwner(note, filter)),
  }));
}

/**
 * 「すべて畳む」と「すべて開く」のどちらを出すか。**見えている付箋に 1 枚でも開いている
 * ものがあれば畳む側** — 押した結果が「全部畳まれている」に必ずなる向きへ倒す。
 * 付箋が 0 枚なら畳む側（押せない状態で出す）。
 */
export function foldAllAction<T>(
  notes: readonly T[],
  isCollapsed: (note: T) => boolean,
): "collapse" | "expand" {
  if (notes.length === 0) return "collapse";
  return notes.some((note) => !isCollapsed(note)) ? "collapse" : "expand";
}
