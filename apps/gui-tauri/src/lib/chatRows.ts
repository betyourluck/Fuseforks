/**
 * 会話表示の行構成。同報・fan-out で複製された発話を表示上 1 つに畳む。
 *
 * 同報はログ上「同じ内容 × 宛先数」として記録される（配送の実体が宛先ごとに
 * 独立なため。これは正しい）。しかし表示でそのまま並べると、同じ文面の
 * 吹き出しが人数分連続して**壊れているように見える**。ユーザー同報も
 * エージェント発の fan-out も同じ形なので、同じ規則で畳む。
 *
 * ChatPanel から切り出した純関数。表示規則はコンポーネントの外でテストする。
 */

import type { AgentMessage, Endpoint } from "../types";

/** 表示の 1 行。同報では 1 通の代表 + 残りの宛先を束ねる。 */
export interface ChatRow {
  message: AgentMessage;
  /** 代表の宛先以外。同報でなければ空。 */
  extraTargets: Endpoint[];
}

/** エンドポイントの同一性。 */
function sameEndpoint(a: Endpoint, b: Endpoint): boolean {
  if (a.kind !== b.kind) return false;
  return a.kind !== "agent" || (b.kind === "agent" && a.id === b.id);
}

/**
 * 直前の行に畳み込めるか。
 *
 * 条件は「同じ送り手・同じ内容・同じ hop・**まだ束ねていない宛先**」の連続。
 * - hop を見るのは、後のターンでたまたま同じ文を言い直した発話を
 *   別の発話として残すため（fan-out の兄弟は必ず同じ hop を持つ）
 * - 宛先の重複を弾くのは、同じ相手への**送り直し**を畳まないため。
 *   同報・fan-out の兄弟で宛先が重複することはない
 */
function foldsInto(row: ChatRow, message: AgentMessage): boolean {
  const head = row.message;
  return (
    sameEndpoint(head.from, message.from) &&
    head.content === message.content &&
    head.hop === message.hop &&
    ![head.to, ...row.extraTargets].some((t) => sameEndpoint(t, message.to))
  );
}

/**
 * 発話列を表示行へ畳む。
 *
 * 畳むのは**連続する**複製だけ。間に別の発話が挟まったものは
 * 独立した行として残る（時系列の事実を並べ替えない）。
 */
export function collapseRows(messages: readonly AgentMessage[]): ChatRow[] {
  const rows: ChatRow[] = [];
  for (const message of messages) {
    const previous = rows[rows.length - 1];
    if (previous && foldsInto(previous, message)) {
      previous.extraTargets.push(message.to);
      continue;
    }
    rows.push({ message, extraTargets: [] });
  }
  return rows;
}
