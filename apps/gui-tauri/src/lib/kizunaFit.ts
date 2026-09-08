/**
 * 絆の地図の Fit を**ズームの上限の内側**で収めるための余白（`failures.md` #122）。
 *
 * v-network-graph の `fitToContents` は、内容が小さくペインが広いとき**上限を超える
 * zoom** を計算し、zoom だけを `maxZoomLevel` へ丸めて pan は丸める前の値のまま当てる
 * （`lib/index.js` の `R` → `Is` / `ro` / `ao`、zoom の setter だけが clamp）。結果、
 * 2 体だけ見えている地図が**左上へ寄る**（実機 2026-09-08。幅が広いほど要求 zoom が
 * 大きく、ずれも大きい）。
 *
 * ここではライブラリの経路を変えず、**余白を辺ごとに広げて要求 zoom を上限ちょうどに
 * する**。`fitToContents({ margin })` は辺ごとの px を受けるので、clamp が起きなければ
 * pan も正しい（再現ページで `matrix(2,0,0,2,…)`・内容の中心 = ペインの中心を実測）。
 *
 * **内容の広がりは控えめに見積もる**（円の半径だけ。名前ラベル・輪・稼働の点は含めない）。
 * 実際の描画はそれより必ず大きいので、ライブラリが出す zoom は上限**以下**に落ち、
 * 再び clamp されることがない。逆に大きく見積もると zoom が上限を超えて元の症状に戻る。
 *
 * 純関数。時計も DOM も読まない（ペインの大きさは引数）。
 */

import type { Point } from "./kizunaSeed";

/** 辺ごとの余白（px）。v-network-graph の `FitContentMargin` の object 形と同じ鍵。 */
export interface FitMargin {
  top: number;
  left: number;
  right: number;
  bottom: number;
}

export interface PaneSize {
  width: number;
  height: number;
}

/**
 * 見えている座標の外接矩形（円の半径ぶんを足す）。0〜1 点なら `null`。
 */
export function contentExtent(
  points: Point[],
  radius: number,
): { width: number; height: number } | null {
  if (points.length < 2) return null;
  let left = Infinity;
  let right = -Infinity;
  let top = Infinity;
  let bottom = -Infinity;
  for (const p of points) {
    left = Math.min(left, p.x - radius);
    right = Math.max(right, p.x + radius);
    top = Math.min(top, p.y - radius);
    bottom = Math.max(bottom, p.y + radius);
  }
  return { width: right - left, height: bottom - top };
}

/**
 * `fitToContents` へ渡す余白。
 *
 * - 内容が上限 zoom でもペインに収まらないなら、基準の余白（`baseRatio` × 各辺の長さ。
 *   ライブラリ既定の `"8%"` と同じ意味）をそのまま返す — 従来どおりの Fit
 * - 内容が小さく、上限 zoom で描いてもペインが余るなら、**余るぶんを余白へ回す**。
 *   ライブラリは `(ペイン − 余白) / 内容` で zoom を出すので、ちょうど上限になる
 * - 点が 1 つ以下なら基準の余白（ライブラリ側も 1 点以下では何もしない）
 */
export function fitMargin(
  points: Point[],
  radius: number,
  pane: PaneSize,
  maxZoom: number,
  baseRatio = 0.08,
): FitMargin {
  const baseX = pane.width * baseRatio;
  const baseY = pane.height * baseRatio;
  const extent = contentExtent(points, radius);
  if (!extent || maxZoom <= 0) {
    return { top: baseY, left: baseX, right: baseX, bottom: baseY };
  }
  const x = Math.max(baseX, (pane.width - extent.width * maxZoom) / 2);
  const y = Math.max(baseY, (pane.height - extent.height * maxZoom) / 2);
  return { top: y, left: x, right: x, bottom: y };
}
