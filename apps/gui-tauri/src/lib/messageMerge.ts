/**
 * 会話を読み直した結果と、読んでいる間に届いた発話を合わせる（Spec 77 N13）。
 *
 * `sessionSwitched` と起動時は `listMessages` でコアの会話ログを読み直す。読んでいる間にも
 * `messageSent` は届いて投影へ積まれるので、読んだ結果で丸ごと置き換えると、その間の発話が
 * 画面から消える（コアには残っている）。読んだ結果の後ろに、まだ無い発話だけを足す。
 *
 * 画面の状態は持たない。呼ぶのは `useOrchestrator.ts`。
 */
import type { AgentMessage } from "../types";

/**
 * 読んだ結果（`fetched`）の後ろに、読んでいる間に積まれた発話（`live`）のうち `fetched` に無いものを
 * 届いた順に足し、上限 `limit` を超えたら古いほうから落とす。
 */
export function mergeFetchedMessages(
  fetched: readonly AgentMessage[],
  live: readonly AgentMessage[],
  limit: number,
): AgentMessage[] {
  const seen = new Set(fetched.map((message) => message.id));
  const merged = [...fetched, ...live.filter((message) => !seen.has(message.id))];
  return merged.length > limit ? merged.slice(merged.length - limit) : merged;
}
