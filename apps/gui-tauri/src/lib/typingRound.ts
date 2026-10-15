/**
 * 入力中バブルの周回数バッジ（2026-10-15 利用者裁定 — ミニマル・バッジ型）。
 *
 * 初版は「ツール 2 / 64 周目」と常に分母まで書いていた。**普段は数ステップで答えに
 * 辿り着くので、上限を常時見せる必要は無い** — 数だけの小さなバッジにし、上限に近づいた
 * ときだけ「N / 上限」と警告色で出す。閾値は上限の 8 割（64 なら 52 周目から）。
 * 上限が分からない個体（`maxToolIterations` が `null`。既定値はコアの設定にあり画面は
 * 知らない）では警告の段が無く、数だけを出し続ける — 知らない分母を捏造しない。
 *
 * 規則はここの純関数、見た目は `ChatPanel.vue`。
 */

/** 上限に対するこの割合から「N / 上限」と警告色にする。 */
export const NEAR_LIMIT_RATIO = 0.8;

export interface RoundBadge {
  /** バッジに出す文字。普段は数だけ、上限が近いと `N / 上限`。 */
  label: string;
  /** 上限が近いか（警告色にする）。上限が分からなければ常に偽。 */
  nearLimit: boolean;
}

/** 上限が分かっているとき、この周回から「近い」と数える（切り上げ。64 → 52、36 → 29）。 */
export function nearLimitFrom(max: number): number {
  return Math.max(1, Math.ceil(max * NEAR_LIMIT_RATIO));
}

export function roundBadge(round: number, max: number | null): RoundBadge {
  if (max !== null && max > 0 && round >= nearLimitFrom(max)) {
    return { label: `${round} / ${max}`, nearLimit: true };
  }
  return { label: String(round), nearLimit: false };
}
