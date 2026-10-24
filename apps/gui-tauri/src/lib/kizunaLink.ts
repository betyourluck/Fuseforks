/**
 * 絆の地図の上で絆を張る・切る規則（2026-10-24）。
 *
 * 地図の右クリックメニュー「ここから絆を伸ばす」→ 行き先のノードをクリック、で張る。
 * 張る規則そのものはカードの drop と同じ `tieAddition`（`kizunaDrop.ts`）を使う —
 * 規則を 2 箇所に書かない。ここに置くのは地図にしかない 2 つ（起点にできるか / 双方向の線の切り方）。
 */
import type { TieHolder } from "./kizunaDrop";
import type { AgentId } from "../types";

/**
 * 起点にできるか。**サーヴァントだけ。**
 *
 * 判断役（Spec 62）から出る線の正本は `judge.toml` の `to` で、`connected_agents` を持たない。
 * 判断役は行き先にはなれる（サーヴァント → 判断役は今の線と同じ）ので、そちらは
 * `tieAddition` の `receivers` が受ける。
 */
export function canStartLink(agents: readonly TieHolder[], id: AgentId): boolean {
  return agents.some((a) => a.id === id);
}

/**
 * 線の切り方。地図の線は**描いた向き**（`source` → `target`）を持つので、それで名指しする。
 * - `both`: 双方向なら両方、片方向ならその 1 本
 * - `forward`: `source` → `target` だけ
 * - `backward`: `target` → `source` だけ
 */
export type EdgeCut = "both" | "forward" | "backward";

/** 地図が描いた 1 本の線。双方向は両端矢の 1 本にまとめてある。 */
export interface DrawnEdge {
  source: AgentId;
  target: AgentId;
  bidirectional: boolean;
}

/** 切った後の接続先（変わる個体だけ）。 */
export interface ConnectionChange {
  id: AgentId;
  connectedAgents: AgentId[];
}

/**
 * 線を切った後、どの個体の接続先がどうなるか。
 *
 * **片方向の線は `cut` を見ない** — 切れる向きが 1 つしか無い。双方向の `forward` / `backward` は
 * 描いた向きで名指しするので、画面のボタンの「A → B」と同じ向きが切れる。
 * 持っていない向きは変わる個体に含めない（呼んでも何も書かない）。
 */
export function edgeCutPlan(
  agents: readonly TieHolder[],
  edge: DrawnEdge,
  cut: EdgeCut,
): ConnectionChange[] {
  const pairs: Array<[AgentId, AgentId]> = [];
  if (!edge.bidirectional || cut === "both" || cut === "forward") {
    pairs.push([edge.source, edge.target]);
  }
  if (edge.bidirectional && (cut === "both" || cut === "backward")) {
    pairs.push([edge.target, edge.source]);
  }

  const changes: ConnectionChange[] = [];
  for (const [from, to] of pairs) {
    const holder = agents.find((a) => a.id === from);
    if (!holder || !holder.connectedAgents.includes(to)) continue;
    changes.push({ id: from, connectedAgents: holder.connectedAgents.filter((id) => id !== to) });
  }
  return changes;
}
