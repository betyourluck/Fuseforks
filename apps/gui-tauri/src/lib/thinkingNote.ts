/**
 * 思考の要約の表示規則（Spec 33 P3）。
 *
 * # 接地の来歴とは別の枠に置く（D6）
 *
 * 出典は**検証できる外部の指し先**、要約は**検証できない内部の申告**である。
 * 同じ枠に並べると後者に前者の信用が乗る。だから規則も表示も別に持つ。
 *
 * # 0 件と「薄い要約」を混ぜない（D4）
 *
 * `reasoning_summary` に入るのは**1 字以上のものだけ**（0 字はコア側の decode が
 * 落とす）。ここで**長さによる足切りをしない**のは、短形（129 字 = 問いの再掲）と
 * 長形（919〜1,367 字）の間に線を引くと、**probe の産物である数字が表示規則に
 * なる**ため（`failures.md` #92）。しかも「再掲だけか」は内容の判断で、
 * 画面側には決められない。**薄いことは利用者が読めば分かる。**
 */

import type { AgentMessage } from "../types";

/** 表示に必要な形へ畳んだ思考の要約。 */
export interface ThinkingView {
  /**
   * 周ごとの要約。**重複は潰さない** — 同じ文が 2 度出るなら、それは
   * モデルが 2 周とも同じことを考えた事実であって重複ではない。
   */
  summaries: string[];
  /** 総文字数。開く前に規模が読めるように `summary` 行へ出す。 */
  chars: number;
}

/**
 * 発話から表示用の要約を作る。要約が 1 つも無ければ `null`。
 *
 * **`null` は「思考しなかった」ではない。** 要約が返らない回は普通にあり
 * （この村のターンの大半を占めるツールループの回がそれ）、そこでも思考トークンは
 * 払っている。量は `turn:` 行の `reasoning=` に出る（Spec 32）。
 * **ここで扱うのは中身だけ。**
 */
export function thinkingView(message: AgentMessage): ThinkingView | null {
  const summaries = message.reasoningSummary;
  if (!summaries || summaries.length === 0) return null;

  return {
    summaries,
    chars: summaries.reduce((total, one) => total + [...one].length, 0),
  };
}
