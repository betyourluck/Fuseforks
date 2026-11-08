/**
 * 会話ペインの「末尾が見えているか」（2026-11-08）。
 *
 * 起点は利用者 —「会話ペインの一番最新まで飛ぶボタン。一番下が見えないスクロール位置だと
 * ↓ の FAB が出てくる。処理中は横三点のプログレス効果」。
 *
 * **判定は 1 本。** 新しい行への追従（末尾を見ているときだけ送る）と、最新へ飛ぶボタンの
 * 出し入れが同じ関数を読む — ボタンが出ていることと追従が止まっていることを常に一致させる。
 * 2 箇所で別々に書くと、「ボタンは消えたのに新しい行が来ても送られない」帯が生まれる。
 *
 * 読むのは**同じ要素のスクロール量だけ**。会話ペインは `zoom` の場所で、要素の座標
 * （画面上の位置）を測る API は持ち込まない。`scrollHeight` / `scrollTop` / `clientHeight` は
 * 同じ要素の同じ座標系で閉じているので、倍率が掛かっても比が保たれる。
 */

/** 末尾からこの距離（CSS px）以内なら「末尾を見ている」とする。 */
export const NEAR_BOTTOM_PX = 80;

/** スクロールの量だけを持つ形。テストで要素を作らずに渡せるよう、要素の型に縛らない。 */
export interface ScrollMetrics {
  scrollHeight: number;
  scrollTop: number;
  clientHeight: number;
}

export function isNearBottom(el: ScrollMetrics): boolean {
  return el.scrollHeight - el.scrollTop - el.clientHeight < NEAR_BOTTOM_PX;
}
