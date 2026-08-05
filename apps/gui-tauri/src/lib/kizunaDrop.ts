/**
 * カードから絆を引く（Spec 21）— 絆の追加規則と drop 座標の取り出し。
 *
 * 追加の規則は 2 経路が共有する: 地図のハンドルドラッグ（TopologyMap の
 * `@connect`）と、サーヴァントリストからのカード drop。規則を 2 箇所に
 * 書かないための置き場がこのファイル。
 */
import type { AgentId } from "../types";

export interface TieHolder {
  id: AgentId;
  connectedAgents: AgentId[];
}

/**
 * source → target の絆を足したあとの接続先一覧。張れないときは `null`。
 *
 * 「接続済み」は**方向付き**で判定する — `A→B` があっても `B→A` は張れる
 * （既存の「逆も引く」と同じ規則。地図では両端矢の 1 本になる）。
 *
 * 自己接続は張らない。Vue Flow のハンドル経路は既定で自己接続の
 * `@connect` を発火させない（Spec 21 P0 実測 6）ので、この検査が実際に
 * 効くのは drop 経路だけだが、ライブラリ更新で変わりうるので規則側に置く。
 */
export function tieAddition(
  agents: readonly TieHolder[],
  sourceId: AgentId,
  targetId: AgentId,
): AgentId[] | null {
  if (sourceId === targetId) return null;

  const source = agents.find((a) => a.id === sourceId);
  if (!source) return null;
  if (!agents.some((a) => a.id === targetId)) return null;
  if (source.connectedAgents.includes(targetId)) return null;

  return [...source.connectedAgents, targetId];
}

export interface DropPoint {
  x: number;
  y: number;
}

/**
 * ドラッグ終端イベントから座標を取り出す。取れなければ `null`。
 *
 * Sortable の `originalEvent` は TouchEvent のことがある（Spec 21 rev2）。
 * TouchEvent に clientX は無いので、構造で見分ける — instanceof にしないのは
 * テストが node 環境（DOM 無し）で走るため。
 */
export function dropPoint(
  event: { clientX?: number; clientY?: number } | { changedTouches?: ArrayLike<{ clientX: number; clientY: number }> } | undefined,
): DropPoint | null {
  if (!event) return null;

  const mouse = event as { clientX?: number; clientY?: number };
  if (typeof mouse.clientX === "number" && typeof mouse.clientY === "number") {
    return { x: mouse.clientX, y: mouse.clientY };
  }

  const touches = (event as { changedTouches?: ArrayLike<{ clientX: number; clientY: number }> }).changedTouches;
  if (touches && touches.length > 0) {
    return { x: touches[0].clientX, y: touches[0].clientY };
  }

  return null;
}
