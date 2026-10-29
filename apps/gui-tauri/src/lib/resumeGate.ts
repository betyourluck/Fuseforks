/**
 * 「続けさせる」を押せるか（Spec 56 D5 → Spec 72 D5）。
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
import type { AgentStatus, CommandPolicyView } from "../types";

/**
 * 続行を配送できる見込みがあるか（稼働の近似）。
 *
 * `undefined` は**一覧に居ない個体**（削除された・まだ読めていない）。
 * 押せない側へ倒す — 押したところでコアが `AGENT_NOT_FOUND` を返す。
 */
export function canResume(status: AgentStatus | undefined): boolean {
  return status === "starting" || status === "running";
}

/**
 * 「続けさせる」を押せない理由（Spec 72 D5）。`null` なら押せる。
 *
 * 順は**止まっていない → 判断待ちが残る → `run.json` が壊れている → 稼働していない**。
 * - `notStopped`: 印が無い（押す対象が無い。節に出ないので普段は見えない）
 * - `pending`: 判断待ちが残っている。押しても再開したターンが最初の `Unknown` でまた止まるだけ
 * - `broken`: `run.json` が読めない。照合の読みが既定へ落ちて全部 `Unknown` になるので、
 *   直す前に続けさせても同じ所で止まる
 * - `notRunning`: `canResume` が偽（Spec 56 D5 の文言のまま）
 */
export type ResumeBlock = "notStopped" | "pending" | "broken" | "notRunning";

export function resumeBlock(
  view: Pick<CommandPolicyView, "stoppedForApproval" | "pending" | "broken">,
  status: AgentStatus | undefined,
): ResumeBlock | null {
  if (!view.stoppedForApproval) return "notStopped";
  if (view.pending.length > 0) return "pending";
  if (view.broken) return "broken";
  if (!canResume(status)) return "notRunning";
  return null;
}

/** 「続けさせる」を押せるか = 押せない理由が無い。 */
export function canResumeNow(
  view: Pick<CommandPolicyView, "stoppedForApproval" | "pending" | "broken">,
  status: AgentStatus | undefined,
): boolean {
  return resumeBlock(view, status) === null;
}
