/**
 * 会話ペインのツール行を開いたときの中身（Spec 57 D7）。
 *
 * **開閉の状態と、引いた中身は別に持つ** — 1 つに畳むと「一度引いたら
 * 閉じられない」になる。閉じても中身は捨てない（開き直すたびに IPC を撃たない）。
 *
 * **`state`（コアの投影）には入れない。** 500 件ぶんの本文を投影に持たない。
 * 中身はコアのメモリにだけ在り、ここにあるのは開いた行の写しだけ。
 */

import { reactive } from "vue";

import type { ToolCallDetail } from "../types";

/**
 * 1 行ぶんの中身の状態。4 つ:
 *
 * - `loading` — 引いている最中。**もう一度押しても 2 本目の IPC を撃たない**
 * - `ready` — 引けた
 * - `gone` — コアにもう無い（500 件の押し出し / 会話の切り替え）。エラーではない。
 *   `callId` は再利用されないので、一度無ければ以後も無い = 引き直さない
 * - `failed` — IPC そのものが落ちた。押し直せば引き直す
 */
export type ToolCallDetailState =
  | { status: "loading" }
  | { status: "ready"; detail: ToolCallDetail }
  | { status: "gone" }
  | { status: "failed" };

export interface ToolCallDetails {
  /** 開いている行の `callId`。 */
  readonly expanded: ReadonlySet<number>;
  /** 引いた中身。閉じても残る。 */
  readonly details: ReadonlyMap<number, ToolCallDetailState>;
  /** 行を開く / 閉じる。開いたときだけ、必要なら中身を引く。 */
  toggle(callId: number): Promise<void>;
  /** 全部捨てる（会話を切り替えたとき）。 */
  reset(): void;
}

/**
 * @param fetchDetail 中身を引く口。部品は `getToolCall` を渡す
 *   （引数にするのは、IPC を立てずに 4 状態と二重呼び出しの防止を試すため）。
 */
export function useToolCallDetails(
  fetchDetail: (callId: number) => Promise<ToolCallDetail | null>,
): ToolCallDetails {
  const expanded = reactive(new Set<number>());
  const details = reactive(new Map<number, ToolCallDetailState>());

  async function toggle(callId: number): Promise<void> {
    if (expanded.has(callId)) {
      expanded.delete(callId);
      return;
    }
    expanded.add(callId);

    const known = details.get(callId);
    // 引いている最中・引けた・もう無い、のどれでも撃ち直さない。失敗だけ引き直す。
    if (known && known.status !== "failed") return;

    details.set(callId, { status: "loading" });
    try {
      const detail = await fetchDetail(callId);
      details.set(callId, detail ? { status: "ready", detail } : { status: "gone" });
    } catch {
      // トーストは出さない — 押した本人の目の前の行に出るので、通知を重ねない。
      details.set(callId, { status: "failed" });
    }
  }

  function reset(): void {
    expanded.clear();
    details.clear();
  }

  return { expanded, details, toggle, reset };
}
