/**
 * サーヴァントのグループ（Spec 51）の純関数。
 *
 * 一覧の区分け・非表示の判定・全体 ▶ の門・地図の辺のフィルタ・drop の確定を
 * **ここ 1 箇所**に置く。コンポーネントは配線だけ（`batchStart.ts` と同じ規律）。
 *
 * 規則（`group_contract`）:
 * - 所属は 1 つ（`groupId`）。村に無い id は**無所属として描く**（凍結 3）
 * - **無所属はどのフィルタでも隠れない**（凍結 6）
 * - 隠すのは見え方で、配送にも一括起動にも効かない — 全体 ▶ の門は
 *   `agent.batchStart && (無所属 || group.batchStart)` で、非表示を見ない（凍結 7）
 * - drop の確定は並びと所属を 1 回で返す（凍結 8）
 */

import type { AgentGroup, AgentId, GroupId, TopologyEdge } from "../types";
import { inListOrder } from "./agentNav";

/** 区分けに要る最小限。`AgentSnapshot` をそのまま渡せる。 */
export interface GroupableAgent {
  id: AgentId;
  order: number;
  groupId: GroupId | null;
  batchStart: boolean;
}

/** 無所属の区分けの鍵。グループ id は UUID なので空文字と衝突しない。 */
export const UNASSIGNED = "";

/** 一覧の 1 区分け。`group` が `null` なら無所属。 */
export interface Section<T extends GroupableAgent> {
  key: string;
  group: AgentGroup | null;
  agents: T[];
}

/**
 * 個体の区分けの鍵。**村に無い id は無所属**（凍結 3 — 引けない id は無所属として描く）。
 */
export function sectionKeyOf(
  agent: Pick<GroupableAgent, "groupId">,
  groups: readonly AgentGroup[],
): string {
  if (agent.groupId === null) return UNASSIGNED;
  return groups.some((g) => g.id === agent.groupId) ? agent.groupId : UNASSIGNED;
}

/**
 * 一覧の区分け。**無所属 → グループの配列順（作成順）**で、各区分けの中は `order` 順。
 * 空の区分け（グループはあるが誰も居ない）も返す — 見出しは落とし場でもある。
 */
export function sectionize<T extends GroupableAgent>(
  agents: readonly T[],
  groups: readonly AgentGroup[],
): Section<T>[] {
  const ordered = inListOrder(agents);
  const sections: Section<T>[] = [
    { key: UNASSIGNED, group: null, agents: [] },
    ...groups.map((group) => ({ key: group.id, group, agents: [] as T[] })),
  ];
  const byKey = new Map(sections.map((s) => [s.key, s]));
  for (const agent of ordered) {
    byKey.get(sectionKeyOf(agent, groups))?.agents.push(agent);
  }
  return sections;
}

/** 隠れているか。**無所属（引けない id を含む）は決して隠れない。** */
export function isHidden(
  agent: Pick<GroupableAgent, "groupId">,
  groups: readonly AgentGroup[],
  hidden: readonly GroupId[],
): boolean {
  const key = sectionKeyOf(agent, groups);
  return key !== UNASSIGNED && hidden.includes(key);
}

/** 見えている個体（一覧の並び順）。地図・絞り込み・Alt+↑↓ の巡回はこれを読む。 */
export function visibleAgents<T extends GroupableAgent>(
  agents: readonly T[],
  groups: readonly AgentGroup[],
  hidden: readonly GroupId[],
): T[] {
  return inListOrder(agents).filter((agent) => !isHidden(agent, groups, hidden));
}

/**
 * 見えている集合に対する選択の落ち着き先。選択中が見えていればそのまま、
 * 隠れたら見えている先頭、**可視 0 体なら `null`**（= 空の村と同じ状態）。
 */
export function settleSelection(
  selected: AgentId | null,
  visible: readonly Pick<GroupableAgent, "id">[],
): AgentId | null {
  if (selected !== null && visible.some((a) => a.id === selected)) return selected;
  return visible[0]?.id ?? null;
}

/**
 * 全体 ▶ の対象（凍結 7）。個体のトグルと、グループのスイッチの 2 段。
 * **非表示は見ない** — 隠したグループも全体 ▶ で起きる（休ませるのはスイッチの仕事）。
 */
export function batchEligible<T extends GroupableAgent>(
  agents: readonly T[],
  groups: readonly AgentGroup[],
): T[] {
  return agents.filter((agent) => {
    if (!agent.batchStart) return false;
    const key = sectionKeyOf(agent, groups);
    if (key === UNASSIGNED) return true;
    return groups.find((g) => g.id === key)?.batchStart ?? true;
  });
}

/**
 * 地図に描く辺 = **両端が見えている辺だけ**。隠れる辺の数は
 * `edges.length - visibleEdges(...).length`（片端でも両端でも隠れていれば数える）。
 */
export function visibleEdges(
  edges: readonly TopologyEdge[],
  visibleIds: ReadonlySet<AgentId>,
): TopologyEdge[] {
  return edges.filter((e) => visibleIds.has(e.source) && visibleIds.has(e.target));
}

/** drop の確定結果。`regroup` が `null` なら所属は変わらない。 */
export interface DropCommit {
  order: AgentId[];
  regroup: { id: AgentId; groupId: GroupId | null } | null;
}

/**
 * drop の確定（凍結 8）。
 *
 * `sections` は **`state` から組んだ全区分け**（畳んだ区分けの個体も含む — DOM に
 * 無くても `state` には居る）。`pending` は Sortable が `update:model-value` で返した
 * 箱ごとの新しい並び（出た箱と入った箱の 2 つ、同じ箱の中なら 1 つ）。保留の無い箱は
 * `sections` の並びをそのまま使うので、**畳んだ区分けの個体が `order` から漏れる形は無い**。
 *
 * 所属: 落ちた箱（`toKey`）の区分けが動かしたカードの新しい所属。同じ箱の中の移動は
 * `regroup: null`。**無所属の箱へ落ちたとき、カードの `groupId` が `null` でなければ
 * `null` を書く**（引けない id の正規化 — 凍結 3）。
 */
export function commitDrop<T extends GroupableAgent>(
  sections: readonly Section<T>[],
  pending: Readonly<Record<string, readonly AgentId[]>>,
  toKey: string,
  movedId: AgentId,
): DropCommit {
  const order: AgentId[] = [];
  for (const section of sections) {
    const ids = pending[section.key] ?? section.agents.map((a) => a.id);
    for (const id of ids) if (!order.includes(id)) order.push(id);
  }

  const moved = sections.flatMap((s) => s.agents).find((a) => a.id === movedId);
  if (!moved) return { order, regroup: null };
  const fromKey = sections.find((s) => s.agents.some((a) => a.id === movedId))?.key ?? UNASSIGNED;
  const targetGroupId = toKey === UNASSIGNED ? null : toKey;
  const crossed = fromKey !== toKey;
  // 無所属の箱で動かした個体が引けない id を持っていれば、ここで null へ正規化する。
  const normalize = toKey === UNASSIGNED && moved.groupId !== null;
  if (!crossed && !normalize) return { order, regroup: null };
  return { order, regroup: { id: movedId, groupId: targetGroupId } };
}
