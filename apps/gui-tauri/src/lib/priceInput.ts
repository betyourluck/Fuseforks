/**
 * 単価の入力欄の値を `ModelTemplate` の欄へ写す純関数（Spec 41 の手入力）。
 *
 * # なぜ `string | number` を受けるか（2026-08-19 の実バグ）
 *
 * `<input type="number">` に `v-model` を付けると、**Vue が値を数値へ自動変換する**
 * （`runtime-dom` の `vModelText`: `castToNumber = number || vnode.props.type === "number"`）。
 * `.number` 修飾子を付けていなくても同じ。初版の setter は `raw.trim()` と文字列前提で
 * 書かれており、数字を打った瞬間に `TypeError` で setter が死んで `draft` へ書かれなかった。
 * **空欄だけは通る**（`""` は数値化されず文字列のまま）ので、「取得」ボタンは効いて
 * 手入力だけが効かない、という形で実機に出た。
 *
 * # 返り値の 3 値
 *
 * - `null` … 空欄 = **未設定へ戻す**（0 ではない。未設定は「無料」ではなく「単価未登録」）
 * - `number` … 0 以上の有限値
 * - `undefined` … 受け付けない入力（負数・NaN・通貨記号など）。**欄を触らない**
 */
export function parsePriceInput(raw: string | number): number | null | undefined {
  const text = String(raw).trim();
  if (text === "") return null;
  const parsed = Number(text);
  return Number.isFinite(parsed) && parsed >= 0 ? parsed : undefined;
}
