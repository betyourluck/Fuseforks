/**
 * 絆の描画方向（2026-08-08）。
 *
 * 起点は利用者 —「絆ペインでは id の順序で辺が上につくか下につくかになるため、
 * それを左ペインの順序にしたい」。
 *
 * **どちらが `source` かが辺の向きを決める。** 正規化しないと決めるのは
 * `state.edges` に先に現れたほう＝**左ペインの順序と無関係な順**になる。
 *
 * **起票時の理由は描画ライブラリ固有だった** — Vue Flow は `source` 側の下端から
 * 出て `target` 側の上端へ入るので、向きが「上につくか下につくか」に直結していた。
 * `v-network-graph` は**中心どうしを結ぶ**のでその効果は無いが、**規則は残す** —
 * 双方向の 1 本をどちらの向きで描くかは矢印の並びに出るし、決め手を
 * 「先に現れたほう」に戻すと**同じ村を開くたびに向きが揺れうる**。
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

/**
 * 辺を「流れる破線」にするか（2026-08-27）。
 *
 * **両端が稼働しているときだけ生きた辺として描く。** 旧規則は片端の稼働で
 * 発火していた（一方向は source のみ・双方向はどちらか片方）が、片方しか
 * 稼働していない辺が流れると**両方稼働していると誤認する**（利用者指摘）。
 * 破線が運ぶ意味を「この線の上で実際にやり取りが成立しうる」へ狭める —
 * 委譲（`ask_*` / `transfer_to_*` / `plan` の波）は相手が稼働していなければ
 * 届かないので、両端の稼働が成立の条件そのもの。
 *
 * 向きも `bidirectional` も判定に使わない — 描画方向は `drawDirection` の
 * 責務で、生死は端点の状態だけで決まる。
 */
export function edgeIsLive(
  edge: { source: string; target: string },
  isRunning: (id: string) => boolean,
): boolean {
  return isRunning(edge.source) && isRunning(edge.target);
}
