/**
 * 会話の参照（`@@`）— 候補の組み立て・順位付け・送信に失敗したときの下書きの復元
 * （Spec 58 / `quote_reference_contract` 凍結 8）。
 *
 * **純関数だけを置く。** IPC も DOM も知らない（`lib/pathComplete.ts` と同じ形）。
 * 写しを作るのはコアで、ここが運ぶのは**発話 ID だけ** — 本文をフロントから送らせると、
 * コアが検証できない文字列が「サーヴァントの発話」という名札でプロンプトへ入る（凍結 1）。
 */
import type { AgentId, AgentMessage, Endpoint } from "../types";

import { MAX_SUGGESTIONS } from "./pathComplete";

/**
 * 1 発話に添えられる参照の件数。**コアの `quote::MAX_QUOTES` と同じ値**
 * （`quoteRefWiring.test.ts` が Rust のソースを読んで突き合わせる）。
 * ここは表示のための上限で、検査するのはコア。
 */
export const MAX_QUOTES = 3;

/** 候補の行に出す本文の先頭の字数。 */
export const QUOTE_HEAD_CHARS = 60;

/** 会話の参照の候補 1 件。 */
export interface QuoteCandidate {
  /** 発話 ID（完全形）。コアへ送るのはこれだけ。 */
  id: string;
  /** 送り手のサーヴァントの id。 */
  fromId: AgentId;
  /** 送り手の表示名。削除済みなら id。 */
  fromName: string;
  /** 宛先の表示（利用者 / サーヴァントの表示名 / 外部）。辞書の鍵か、そのままの名前。 */
  to: Endpoint;
  /** 発話の時刻。 */
  tsMs: number;
  /** 本文の先頭（1 行へ潰してある）。 */
  head: string;
  /** 本文の字数（code point）。 */
  chars: number;
  /** 照合用の本文（小文字）。表示には使わない。 */
  haystack: string;
}

/** 入力欄に付いている参照のチップ 1 件。 */
export interface QuoteChip {
  id: string;
  fromName: string;
  tsMs: number;
  chars: number;
}

/**
 * チップと候補の行に出す時刻（時:分）。**会話ペインの発話の時刻と同じ書式** —
 * 「12:03 の回答」を会話の中で目で探せるようにするため。
 */
export function shortTime(tsMs: number): string {
  return new Date(tsMs).toLocaleTimeString("ja-JP", { hour: "2-digit", minute: "2-digit" });
}

/** 字数は code point で数える（コアの `chars().count()` と同じ数え方）。 */
export function countChars(text: string): number {
  return Array.from(text).length;
}

/**
 * この会話の発話から候補を組む。**サーヴァントが書いた発話だけ**（凍結 2 の (b)）。
 *
 * - 宛先は問わない — 利用者宛の回答も、委譲の答え（サーヴァント宛）も出す
 * - **今の宛先の個体が書いた発話も出す**（滑る窓の外へ落ちた自分の回答を読み直させる）
 * - **表示クリアで隠した発話も出す** — 引数は `state.messages` そのもので、
 *   表示クリアの境界をここへ渡さない。隠すのは画面を静かにする操作で、会話から
 *   消す操作ではない
 *
 * 並びは**新しい順**。
 */
export function quoteCandidates(
  messages: readonly AgentMessage[],
  nameOf: (id: AgentId) => string | null,
): QuoteCandidate[] {
  const out: QuoteCandidate[] = [];
  for (let i = messages.length - 1; i >= 0; i--) {
    const message = messages[i];
    if (message.from.kind !== "agent") continue;
    const chars = Array.from(message.content);
    out.push({
      id: message.id,
      fromId: message.from.id,
      fromName: nameOf(message.from.id) ?? message.from.id,
      to: message.to,
      tsMs: message.tsMs,
      head: chars.slice(0, QUOTE_HEAD_CHARS).join("").replace(/\s+/g, " ").trim(),
      chars: chars.length,
      haystack: message.content.toLowerCase(),
    });
  }
  return out;
}

/**
 * 候補を絞り込んで順位付けする。
 *
 * | 段 | 条件 |
 * |---|---|
 * | 1 | 送り手の表示名が前方一致 |
 * | 2 | 送り手の表示名が部分一致 |
 * | 3 | 本文が部分一致 |
 * | 4 | 一致しない → 落とす |
 *
 * **同点は新しい順**（= 入力の並び）。大文字小文字は無視する。
 * クエリが空なら、先頭（新しい側）から `limit` 件。
 *
 * クエリに空白は来ない（空白を打った時点で補完が閉じる）ので、空白区切りの
 * AND は持たない。
 */
export function rankQuotes(
  candidates: readonly QuoteCandidate[],
  query: string,
  limit: number = MAX_SUGGESTIONS,
): QuoteCandidate[] {
  const needle = query.toLowerCase();
  if (!needle) return candidates.slice(0, limit);

  const ranked: { rank: number; order: number; candidate: QuoteCandidate }[] = [];
  candidates.forEach((candidate, order) => {
    const name = candidate.fromName.toLowerCase();
    const inName = name.indexOf(needle);
    if (inName === 0) ranked.push({ rank: 1, order, candidate });
    else if (inName > 0) ranked.push({ rank: 2, order, candidate });
    else if (candidate.haystack.includes(needle)) ranked.push({ rank: 3, order, candidate });
  });
  ranked.sort((a, b) => a.rank - b.rank || a.order - b.order);
  return ranked.slice(0, limit).map((r) => r.candidate);
}

/** 候補をチップへ写す。 */
export function chipOf(candidate: QuoteCandidate): QuoteChip {
  return {
    id: candidate.id,
    fromName: candidate.fromName,
    tsMs: candidate.tsMs,
    chars: candidate.chars,
  };
}

/**
 * チップを 1 件足す。**同じ発話は 1 件**、上限は [`MAX_QUOTES`]。
 * 足せなかったら同じ配列をそのまま返す（呼び手は参照の一致で「足せたか」を見られる）。
 */
export function addChip(chips: readonly QuoteChip[], chip: QuoteChip): readonly QuoteChip[] {
  if (chips.length >= MAX_QUOTES) return chips;
  if (chips.some((c) => c.id === chip.id)) return chips;
  return [...chips, chip];
}

/** 入力欄の下書き — 文面・添付・参照の 3 つ。 */
export interface Draft<A> {
  text: string;
  attachment: A | null;
  quotes: readonly QuoteChip[];
}

/**
 * 送信に失敗した下書きを、入力欄へ戻す（Spec 58 D3）。
 *
 * 送信は「消してから送る」順なので、失敗すると依頼文ごと失われる。コアが参照を
 * 拒む経路（`INVALID_QUOTE`）を新しく作ったので、拒まれたときに書いた文を失わせない。
 * **戻す機構は 1 つで、文面・添付・参照の 3 つに同じく効かせる。**
 *
 * **待っている間に打たれたものを上書きしない** — 送信は数百 ms で返るが、その間に
 * 次の文を打ち始める人は居る。失敗した側を先頭に、今ある側を後ろへ繋ぐ。
 */
export function restoreDraft<A>(current: Draft<A>, failed: Draft<A>): Draft<A> {
  const text =
    current.text && failed.text
      ? `${failed.text}\n${current.text}`
      : failed.text || current.text;
  let quotes: readonly QuoteChip[] = failed.quotes;
  for (const chip of current.quotes) quotes = addChip(quotes, chip);
  return {
    text,
    // 添付は 1 枚。待っている間に貼り直されていたら、新しいほうが利用者の意図。
    attachment: current.attachment ?? failed.attachment,
    quotes,
  };
}
