/**
 * ステータスバーのコマンド承認モード（Spec 61）の表示と巡回。
 *
 * **クリックで順番に切り替える**（2026-09-27 利用者裁定。初版の select から変更）。
 * 順番は緩める向き — 承認あり → 自動許可＋承認 → 自動許可 → 承認あり。
 * 押し間違えても、もう 2 回押せば元へ戻る（帯の字でいまのモードが読める）。
 */

import type { RunApproval } from "../types";

export const RUN_APPROVAL_ORDER: readonly RunApproval[] = ["required", "auto_approve", "no_approval"];

/** 次のモード。知らない値は既定（承認あり）へ戻す。 */
export function nextRunApproval(mode: RunApproval): RunApproval {
  const i = RUN_APPROVAL_ORDER.indexOf(mode);
  if (i < 0) return "required";
  return RUN_APPROVAL_ORDER[(i + 1) % RUN_APPROVAL_ORDER.length];
}

/** 帯に出す字の辞書の鍵。 */
export function runApprovalLabelKey(mode: RunApproval): string {
  switch (mode) {
    case "auto_approve":
      return "statusBar.runApprovalAutoApprove";
    case "no_approval":
      return "statusBar.runApprovalNoApproval";
    default:
      return "statusBar.runApprovalRequired";
  }
}
