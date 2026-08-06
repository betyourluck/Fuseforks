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

import type { AgentId, AgentMessage, Endpoint } from "../types";

/** 表示の 1 行。同報では 1 通の代表 + 残りの宛先を束ねる。 */
export interface ChatRow {
  message: AgentMessage;
  /** 代表の宛先以外。同報でなければ空。 */
  extraTargets: Endpoint[];
}

/**
 * ツール実行 1 件（表示用）。
 *
 * コアの `toolInvoked` イベントを受けて組む、フロント側だけの投影。
 * Rust に対応する型は無いので `types.ts`（ミラー契約）には置かない。
 */
export interface ToolRun {
  /** 表示のキー。イベントに ID が無いので受け手側で振る。 */
  id: string;
  agentId: AgentId;
  tool: string;
  ok: boolean;
  /** 受信時刻。コアは時刻を載せないので、受け取った側で打つ。 */
  tsMs: number;
}

/**
 * 会話ペインに並べる 1 項目。発話とツール実行が時系列で混ざる。
 *
 * **混ぜるのは、両者が因果で繋がっているから。** 「調べて」と頼まれた
 * エージェントが `grep` を 3 回叩いてから答えた、という流れは、発話だけ
 * 見ていても分からないし、ツール実行だけ見ていても分からない。
 */
export type TimelineEntry =
  | { kind: "message"; key: string; row: ChatRow }
  | { kind: "tool"; key: string; run: ToolRun };

/**
 * 場からの告知か（吹き出しではなく細い 1 行で出す対象）。
 *
 * **判定は宛先で決まる。** `AgentMessage` に種別の欄は無いが、**宛先が既に
 * その区別を持っている**:
 *
 * - **System → User** は `record` だけで**配送されない告知**（起動・停止・
 *   役職変更・打ち切り・予算切れ・要約・予定のスキップ）。出来事の記録であって
 *   誰かの発言ではない
 * - **System → Agent** は**予定の発火**で、実際に配送されてターンを起こす。
 *   これは依頼そのものなので吹き出しのまま
 *
 * 文面で見分けようとすると 12 箇所の書式に依存して壊れる。**宛先は構造なので
 * 文面を変えても壊れない。**
 */
export function isSystemNotice(message: AgentMessage): boolean {
  return message.from.kind === "system" && message.to.kind === "user";
}

/**
 * エンドポイントの同一性。
 *
 * **中身を持つ種別は中身まで見る**（`agent` の id / `external` の client）。
 * 種別だけで畳むと、別のクライアントからの依頼が同じ送り手として束ねられる。
 */
export function sameEndpoint(a: Endpoint, b: Endpoint): boolean {
  if (a.kind !== b.kind) return false;
  if (a.kind === "agent") return b.kind === "agent" && a.id === b.id;
  if (a.kind === "external") return b.kind === "external" && a.client === b.client;
  return true;
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

/**
 * 発話行とツール実行を時系列に 1 本へ畳む。
 *
 * # 同時刻の並び
 *
 * ツール実行はその結果を含む発話より**先**に置く。時刻が同じミリ秒に
 * なったときも同じ — 因果の順序（呼んでから答える）を時刻の丸めで
 * ひっくり返さない。
 *
 * # 時刻の出所が違うことについて
 *
 * 発話の `tsMs` はコアが打ち、ツール実行の `tsMs` はイベントを受けた
 * フロントが打つ。同一プロセスではないが**同じ機械の壁時計**なので、
 * 会話ペインの粒度（秒）では並べ替えに耐える。厳密な因果順が要る用途が
 * 出たら、コア側で採番して載せること。
 */
export function buildTimeline(
  rows: readonly ChatRow[],
  runs: readonly ToolRun[],
): TimelineEntry[] {
  /** 並べ替えの鍵を項目の外に持つ（表示用の型を汚さない）。 */
  interface Sortable {
    at: number;
    /** 同時刻の優先度。小さいほど前。 */
    tie: number;
    entry: TimelineEntry;
  }

  const sortable: Sortable[] = [
    ...rows.map((row) => ({
      at: row.message.tsMs,
      // 同時刻なら発話を後ろへ（ツールを呼んでから答える）。
      tie: 1,
      entry: { kind: "message", key: row.message.id, row } as TimelineEntry,
    })),
    ...runs.map((run) => ({
      at: run.tsMs,
      tie: 0,
      entry: { kind: "tool", key: run.id, run } as TimelineEntry,
    })),
  ];

  sortable.sort((a, b) => a.at - b.at || a.tie - b.tie);
  return sortable.map(({ entry }) => entry);
}
