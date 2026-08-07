/**
 * 絆の描画方向（2026-08-08）。
 *
 * 起点は利用者 —「絆ペインでは id の順序で辺が上につくか下につくかになるため、
 * それを左ペインの順序にしたい」。
 *
 * Vue Flow は **`source` 側の下端から出て `target` 側の上端へ入る**ので、
 * **どちらが `source` かが「上につくか下につくか」を決める**。正規化しないと
 * 決めるのは `state.edges` に先に現れたほう＝**左ペインの順序と無関係な順**になる。
 *
 * **「村人に序列は作らない」との整合**: ここで使う `order` は**左ペインの並び
 * そのもの**で、偉さではなく画面の並び。地図と一覧で同じ並びを見せるためだけに
 * 使い、配送にも権限にも一切効かない。
 */

/**
 * 描画上の `source` / `target` を決める。
 *
 * **双方向のときだけ並べ替える。** 一方向の絆は `A→B` という**事実そのもの**で、
 * 向きを都合で入れ替えると**意味が変わる**（矢印が逆を指す）。
 *
 * @param orderOf 左ペインでの並び順。引けない id は末尾へ倒すこと
 *   （`NaN` を作ると比較が不定になり、描画順が呼ぶたびに揺れる）。
 */
export function drawDirection(
  source: string,
  target: string,
  bidirectional: boolean,
  orderOf: (id: string) => number,
): [source: string, target: string] {
  if (!bidirectional) return [source, target];
  return orderOf(source) <= orderOf(target) ? [source, target] : [target, source];
}
