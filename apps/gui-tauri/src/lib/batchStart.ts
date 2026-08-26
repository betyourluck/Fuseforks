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

/**
 * 止められる状態か。**`starting` は含めない**（2026-08-27 に反転。利用者判断）。
 *
 * 旧規則は「起動を取り消したい場面がある」として含めていたが、実機では
 * **一括起動の直後にもう一度押して、起きた直後の個体を止める二度押し**の
 * ほうが多発した。取り消しの利便より誤停止の事故が重い。`starting` は
 * MCP 接続のタイムアウト（30 秒）で必ず `running` / `failed` へ抜けるので、
 * 触れない時間は有界 — 閉じ込めにはならない。
 */
function isStoppable(status: AgentStatus): boolean {
  return status === "running";
}

/** 遷移中か。この状態の対象がいる間、停止側の役は封じる（二度押し対策）。 */
function isTransitioning(status: AgentStatus): boolean {
  return status === "starting" || status === "stopping";
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

  // 遷移中の対象がいる間は停止へ役を変えない。一括起動の直後は全員が
  // `starting` で、ここで `stop` に変わると二度押しがそのまま誤停止になる
  // （個体が `running` へ抜けた順に停止対象へ入るので、窓は縮むだけで
  // 消えない — だから「startable が空で遷移中がいる」は一律 `none`）。
  if (eligible.some((agent) => isTransitioning(agent.status))) {
    return { mode: "none", targets: [] };
  }

  const stoppable = eligible.filter((agent) => isStoppable(agent.status));
  if (stoppable.length) {
    return { mode: "stop", targets: stoppable.map((agent) => agent.id) };
  }

  // 対象が 0 体。押せる操作が無い。
  return { mode: "none", targets: [] };
}

/**
 * ボタンの見た目（記号と説明の辞書キー）。`none` は無効表示の理由も返す。
 *
 * 文字列そのものではなく辞書キー + 引数を返す（Spec 13 P3）。純関数の層は
 * i18n インスタンスを知らないままにし、翻訳は表示側（`$t`）の責務に置く。
 */
export function batchLabel(
  action: BatchAction,
  eligibleCount: number,
): { icon: string; titleKey: string; titleParams?: Record<string, unknown> } {
  switch (action.mode) {
    case "start":
      return {
        icon: "▶",
        titleKey: "agentList.batchStart",
        titleParams: { count: action.targets.length },
      };
    case "stop":
      return {
        icon: "■",
        titleKey: "agentList.batchStop",
        titleParams: { count: action.targets.length },
      };
    case "none":
      return {
        icon: "▶",
        titleKey: eligibleCount
          ? "agentList.batchTransitioning"
          : "agentList.batchNoTargets",
      };
  }
}
