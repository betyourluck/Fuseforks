/**
 * ステータスバーの 2 つのスイッチを端末に覚える（2026-09-27 利用者裁定）。
 *
 * - 計画の確認を飛ばす（Spec 53）
 * - コマンドの承認モード（Spec 61）
 *
 * **コアはどちらもメモリだけに持ち、起動時は必ず既定（確認あり / 承認が必要）で始まる。**
 * 覚えるのは画面側で、起動時にこの値をコアへ設定し直す。Spec 53 は「保存しない・起動時は
 * 必ず OFF」だったが、利用者が「ブラウザのストレージには記憶されるように」と裁定した。
 * `world.json` へは入れない — 村を配った先で、受け取った人の確認が黙って外れないように
 * （端末の見え方と同じ棚）。
 *
 * **書くのはコアが受け入れた値だけ**（設定の成功とイベント）。起動時にコアから読んだ既定を
 * 書くと、覚えていた値を読む前に消してしまう。
 */

import type { RunApproval } from "../types";

export const SWITCH_STORAGE_KEY = "fuseforks.switches.v1";

export interface SwitchMemory {
  planReviewBypass?: boolean;
  runApproval?: RunApproval;
}

const MODES: readonly RunApproval[] = ["required", "auto_approve", "no_approval"];

/** 保存値を読む。壊れていたら・知らない値はその欄だけ捨てる（既定へ倒す）。 */
export function parseSwitchMemory(raw: string | null): SwitchMemory {
  if (!raw) return {};
  try {
    const parsed = JSON.parse(raw) as Record<string, unknown>;
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return {};
    const out: SwitchMemory = {};
    if (typeof parsed.planReviewBypass === "boolean") out.planReviewBypass = parsed.planReviewBypass;
    if (MODES.includes(parsed.runApproval as RunApproval)) {
      out.runApproval = parsed.runApproval as RunApproval;
    }
    return out;
  } catch {
    return {};
  }
}

export function loadSwitchMemory(): SwitchMemory {
  try {
    return parseSwitchMemory(localStorage.getItem(SWITCH_STORAGE_KEY));
  } catch {
    return {};
  }
}

/** 1 つの欄だけ書き換える（もう片方は保存値のまま）。保存できなくても黙って続ける。 */
export function rememberSwitch(patch: SwitchMemory): void {
  try {
    const next = { ...loadSwitchMemory(), ...patch };
    localStorage.setItem(SWITCH_STORAGE_KEY, JSON.stringify(next));
  } catch {
    // 保存できなくてもその場の切り替えは効いている。
  }
}
