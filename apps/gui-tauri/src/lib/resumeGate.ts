/**
 * 「承認して続けさせる」を押せるか（Spec 56 D5）。
 *
 * **これは導線であって保証ではない。** 配送できるかを実際に決めているのは
 * コア側の**受信箱の有無**で、画面は受信箱を見られないので `status` で近似する。
 * 画面の状態が古いまま押された競合は、コアが `NOT_RUNNING` で断る
 * （Spec 20 の「fail closed は提示ではなく `decide` が守る」と同じ線）。
 *
 * **部品へ直書きしない理由**: 判定を `.vue` の中に書くと、走査テストは
 * その字面しか見られず、条件を `true` へ倒しても緑のまま通る（2026-09-17 の
 * 作業状況タブの印で実際に踏んだ）。純関数へ出せば単体で留まる。
 */
import type { AgentStatus } from "../types";

/**
 * 続行を配送できる見込みがあるか。
 *
 * `undefined` は**一覧に居ない個体**（削除された・まだ読めていない）。
 * 押せない側へ倒す — 押したところでコアが `AGENT_NOT_FOUND` を返す。
 */
export function canResume(status: AgentStatus | undefined): boolean {
  return status === "starting" || status === "running";
}
