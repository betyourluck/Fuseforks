/**
 * 作業状況タブの**承認待ちの印**（2026-09-17）。
 *
 * 計画の確認（Spec 43）で止まっている波は、作業状況タブを開いていないと気づけなかった。
 * 黒板タブを見ているあいだに進行役が `plan` を提示すると、編集パネルは隠れたタブの中に
 * 出て、村は黙って待つ。処方はタブ自身に件数のバッジを出し、ラベルを accent にすること —
 * タイトルバーの「コマンド承認」のバッジ（Spec 20）と同じ形で、**丸めない**
 * （溜まりすぎが数で見えること自体に意味がある）。
 *
 * 数えるのは `state === "pending"` の波だけ。`useWaveClear` は終わった波しか隠さないので、
 * 表示クリアと独立に数えてよい（隠れた承認待ちは存在しない）。
 * `PlanWavePane` の編集パネルの件数もここから引く — 2 箇所で数えると
 * 「タブは 1 なのにパネルは 2」の形が緑のまま入る。
 */

import type { BottomTab, PlanWaveState } from "../types";

export function pendingWaveCount(waves: readonly { state: PlanWaveState }[]): number {
  return waves.filter((w) => w.state === "pending").length;
}

/**
 * 印（accent の色・件数のバッジ・ホバーの文言）を付けるタブか。`waves` だけ —
 * 黒板に承認の概念は無い。件数 0 では付けない（印が常時あると印の意味が消える。
 * ステータスバーの MCP の待ち受け表示と同じ判断）。
 */
export function tabAttention(tab: BottomTab, pendingCount: number): boolean {
  return tab === "waves" && pendingCount > 0;
}
