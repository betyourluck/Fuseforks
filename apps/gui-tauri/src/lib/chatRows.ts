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

import type { AgentId, AgentMessage, Endpoint, ReasonState } from "../types";

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
  /** 表示のキー。受け手側で振る（`callId` が来る前からの鍵で、動かす理由が無い）。 */
  id: string;
  /**
   * 中身（引数と、モデルへ返した本文）を引く鍵（Spec 57）。コアが振る。
   * 行を開いたときに `get_tool_call` へ渡す。
   */
  callId: number;
  agentId: AgentId;
  tool: string;
  /** **返り値が `Ok` だったか。副作用の成否ではない**（Spec 27 D11）。 */
  ok: boolean;
  /** モデルが書いた 1 行の意図（Spec 27）。**自己申告であって監査証跡ではない。** */
  reason: ReasonState;
  /** 受信時刻。コアは時刻を載せないので、受け取った側で打つ。 */
  tsMs: number;
}

/**
 * 理由の行に何を出すか。
 *
 * **純関数は i18n を知らない**ので、訳語ではなく**辞書の鍵**を返す
 * （`batchLabel` が `titleKey` を返すのと同じ形）。
 *
 * `null` は**行そのものを出さない**という意味。空文字と混同しないこと —
 * 空文字を返すと「理由が空である」という別の主張になる。
 */
export type ReasonDisplay =
  | { kind: "text"; text: string }
  | { kind: "labelKey"; key: string }
  | null;

/**
 * 理由の状態を表示へ落とす。
 *
 * **`kind` で分岐し、推測しない**（Spec 27 D10）。`unsupported` と `excluded` を
 * 同じ扱いにしないのは、**`ask_agent_3` に「外部ツール」と出すのが嘘**だから。
 */
export function reasonDisplay(reason: ReasonState): ReasonDisplay {
  switch (reason.kind) {
    case "written":
      return { kind: "text", text: reason.text };
    case "omitted":
      return { kind: "labelKey", key: "chat.reasonOmitted" };
    case "unsupported":
      return { kind: "labelKey", key: "chat.reasonUnsupported" };
    case "excluded":
      return null;
  }
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

/**
 * 1 つのまとまりに束ねる最小の本数（Spec 57 D8）。
 *
 * 実測（`fuseforks.log` の 5,181 呼び出し）で、同じ個体の連続 3 本以上が
 * 全ツール行の 76.0%。2 本以下は畳んでも 1 行しか減らず、押す手間のほうが大きい。
 */
export const TOOL_GROUP_MIN = 3;

/**
 * 会話ペインに実際に描く 1 項目。[`TimelineEntry`] に「束ねの見出し」が加わる。
 *
 * **見出しは中の行を持たない** — 開いているまとまりの行は、見出しの後ろに
 * ふつうの `tool` 項目として並ぶ。畳んでいるまとまりの行は並びに居ない。
 * 描く側が入れ子の v-for を持たずに済み、ツール行の描き方が 1 箇所のままになる。
 */
export type DisplayEntry =
  | TimelineEntry
  | {
      kind: "toolGroup";
      /** まとまりの鍵 = 先頭の行の `callId`。行は末尾にしか増えないので、伸びても動かない。 */
      key: string;
      agentId: AgentId;
      /** まとまりの本数。 */
      count: number;
      /** 開いているか（見出しの後ろに行が並んでいるか）。 */
      open: boolean;
    };

/** まとまりの鍵。開閉の状態（描く側の `Set`）はこの文字列で持つ。 */
export function toolGroupKey(first: ToolRun): string {
  return `tool-group-${first.callId}`;
}

/**
 * 連続するツール行を束ねる（Spec 57 D8）。規則は 4 つ:
 *
 * 1. **同じ個体のツール行が、間に他の項目を挟まずに [`TOOL_GROUP_MIN`] 本以上**
 *    続いたら 1 つのまとまり。発話・告知・別の個体の行で切れる
 *    （時系列を並べ替えてまで束ねない）
 * 2. **その個体が処理中のあいだは束ねない** — いま何をしているかが見える必要がある。
 *    「後ろに何が来たか」で切らないのは、波では別の個体の行が後ろに来るので、
 *    まだ答えていない個体の行が途中で畳まれるため
 * 3. **`ok = false` の行を 1 本でも含むまとまりは束ねない** — 赤いドットを
 *    見出しの裏へ隠さない
 * 4. それ以外は束ね、**既定で畳む**。`opened` に鍵があるまとまりだけ、
 *    見出しの後ろへ行を並べる
 *
 * **見出しにエラーの本数は持たせない。** 3 により束ねたまとまりは常に
 * `ok = false` が 0 本で、しかも `ok` は「返り値が Ok か」であって「成功したか」
 * ではない（同梱ツールは失敗を `Ok(<エラー文>)` で返す）。「エラー 0 件」と
 * 書くと正常に終わったと読まれる。
 */
export function groupToolRuns(
  entries: readonly TimelineEntry[],
  busy: ReadonlySet<AgentId>,
  opened: ReadonlySet<string>,
): DisplayEntry[] {
  const out: DisplayEntry[] = [];
  let i = 0;
  while (i < entries.length) {
    const head = entries[i];
    if (head.kind !== "tool") {
      out.push(head);
      i += 1;
      continue;
    }
    // 同じ個体のツール行が続く範囲 [i, j)。
    let j = i + 1;
    while (j < entries.length) {
      const next = entries[j];
      if (next.kind !== "tool" || next.run.agentId !== head.run.agentId) break;
      j += 1;
    }
    const run = entries.slice(i, j) as Extract<TimelineEntry, { kind: "tool" }>[];
    const foldable =
      run.length >= TOOL_GROUP_MIN &&
      !busy.has(head.run.agentId) &&
      run.every((e) => e.run.ok);
    if (!foldable) {
      out.push(...run);
    } else {
      const key = toolGroupKey(head.run);
      const open = opened.has(key);
      out.push({ kind: "toolGroup", key, agentId: head.run.agentId, count: run.length, open });
      if (open) out.push(...run);
    }
    i = j;
  }
  return out;
}

/**
 * 開いた行の「入力」に出す文字列（Spec 57 D5）。
 *
 * **形は旗で決める。`typeof` で推測しない** — 切っていない引数そのものが
 * 文字列の JSON 値でありうる。切った引数は詰めた形の先頭なので、整形せず
 * そのまま出す（途中で切れた JSON は parse できない）。
 */
export function formatToolArgs(args: unknown, truncated: boolean): string {
  if (truncated) return typeof args === "string" ? args : JSON.stringify(args);
  return JSON.stringify(args, null, 2);
}
