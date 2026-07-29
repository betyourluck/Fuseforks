/**
 * 表示用の数値整形。
 *
 * 桁区切り（`1,234,567`）をやめて短縮表記（`1.2M`）にするのは、
 * トークン数が 3 箇所（カード・設定ダイアログ・トポロジーのノード）に出るうえ、
 * どれも幅の狭い枠だから。桁が伸びると隣の項目を押し出すか、切り詰められて
 * 先頭だけが見える状態になる。**桁数そのものより「どの規模か」が読めればよい。**
 *
 * 整形は**フロント側の責務**にしてある。コアが返すのは生の数値で、
 * 表示の都合（ロケール・幅・単位）をワイヤの型に持ち込まない。
 */

/** 短縮表記の単位。1000 進で K → M → G → T。 */
const UNITS = ["", "K", "M", "G", "T"] as const;

/**
 * 数値を短縮表記にする。`1234` → `1.2K`、`1234567` → `1.2M`。
 *
 * - 1000 未満はそのまま整数で返す（`999` → `999`）。小数を付けても情報が増えない
 * - 1000 以上は小数第 1 位まで。`1.0M` のように末尾が 0 でも桁を落とさない —
 *   落とすと `1M` と `1.04M` が同じ幅にならず、並べたときに揃わない
 * - 負数は符号を保つ（現状の呼び出し元には無いが、丸めの実装で符号が
 *   消えるのは驚きになる）
 * - 有限でない値は `—`（未取得と同じ見た目にして、`NaN` を画面に出さない）
 */
export function compactNumber(value: number): string {
  if (!Number.isFinite(value)) return "—";

  const sign = value < 0 ? "-" : "";
  let n = Math.abs(value);
  let unit = 0;

  while (n >= 1000 && unit < UNITS.length - 1) {
    n /= 1000;
    unit += 1;
  }

  // 単位なし = 元の値が 1000 未満。小数を付けない。
  if (unit === 0) return `${sign}${Math.round(n)}`;

  return `${sign}${n.toFixed(1)}${UNITS[unit]}`;
}

/**
 * 正確な値を添えるための桁区切り表記。`title` 属性など、
 * 短縮表記だけでは足りない場所で使う。
 */
export function exactNumber(value: number): string {
  if (!Number.isFinite(value)) return "—";
  return value.toLocaleString("ja-JP");
}
