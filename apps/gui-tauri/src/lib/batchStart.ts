/**
 * 一括起動（左ペインヘッダの ▶）の判定規則。
 *
 * # 2 つの軸を分ける
 *
 * このボタンが成立するには、**「起動対象に含めるか」と「今動いているか」を
 * 別の軸として持つ**必要がある。元の UI は 1 つのトグルに両方を兼ねさせて
 * いたため、「起きてはいないが、一括起動では起こしたい」という状態を
 * 表現できなかった（= 毎回 1 体ずつ押すしかなかった）。
 *
 * - 含めるか → `AgentSnapshot.batchStart`（永続。`world.json` に入る）
 * - 動いているか → `AgentSnapshot.status`（実行時。永続しない）
 *
 * # なぜトグルなのか
 *
 * 対象が全部動いていれば、次に押したい操作は「起動」ではなく「停止」である。
 * 別ボタンを 2 つ並べるより、状態に応じて 1 つが役を変えるほうが、
 * 「今押したら何が起きるか」が一意に決まる。
 *
 * 純関数としてここに置くのは、この分岐が唯一の意思決定であり、
 * コンポーネントの外でテストできるようにするため。
 */

import type { AgentId, AgentSnapshot, AgentStatus } from "../types";

/** ボタンが次に行う操作。 */
export type BatchMode = "start" | "stop" | "none";

/** 一括操作の内容。`targets` が実際に叩く相手。 */
export interface BatchAction {
  mode: BatchMode;
  targets: AgentId[];
}

/** 起こせる状態か。`stopping` は遷移中なので触らない（叩くと競合する）。 */
function isStartable(status: AgentStatus): boolean {
  return status === "idle" || status === "failed";
}

/** 止められる状態か。`starting` も含める（起動を取り消したい場面がある）。 */
function isStoppable(status: AgentStatus): boolean {
  return status === "running" || status === "starting";
}

/**
 * 一括ボタンが次に行う操作を決める。
 *
 * 規則は「起こせる対象が 1 体でもいれば起動、全部動いていれば停止」。
 * **起動側を優先する**のは、混在状態（一部だけ落ちている）で押したときに
 * 期待されるのが「揃える」方向だから — そこで全部止まると、動いていた
 * エージェントの会話を巻き添えに殺すことになる。
 */
export function batchAction(agents: readonly AgentSnapshot[]): BatchAction {
  const eligible = agents.filter((agent) => agent.batchStart);

  const startable = eligible.filter((agent) => isStartable(agent.status));
  if (startable.length) {
    return { mode: "start", targets: startable.map((agent) => agent.id) };
  }

  const stoppable = eligible.filter((agent) => isStoppable(agent.status));
  if (stoppable.length) {
    return { mode: "stop", targets: stoppable.map((agent) => agent.id) };
  }

  // 対象が 0 体、または全員が遷移中（stopping）。押せる操作が無い。
  return { mode: "none", targets: [] };
}

/** ボタンの見た目（記号と説明）。`none` は無効表示の理由も返す。 */
export function batchLabel(
  action: BatchAction,
  eligibleCount: number,
): { icon: string; title: string } {
  switch (action.mode) {
    case "start":
      return {
        icon: "▶",
        title: `対象の ${action.targets.length} 体を一括起動`,
      };
    case "stop":
      return {
        icon: "■",
        title: `稼働中の ${action.targets.length} 体を一括停止`,
      };
    case "none":
      return {
        icon: "▶",
        title: eligibleCount
          ? "対象が全員 状態の変更中です"
          : "一括起動の対象がありません（カードのトグルで選んでください）",
      };
  }
}
