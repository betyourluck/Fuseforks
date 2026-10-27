/**
 * 地図の「重なりをほどく」（2026-10-27 利用者裁定）。
 *
 * **全体を並べ直さない。** 円環・放射のような規則的な配置は実機で 2 度却下されている
 * （「網ではなく図表に見える」）ので、押したときだけ動く形にしても並べ直しは採らない。
 * ここがやるのは**近すぎる組を離すことだけ**で、離れている個体の座標には触れない。
 * 人が置いた配置はそのまま残る。
 *
 * 離し方は組ごとに**等しく反対向き**へ押す。だから全体の重心は動かない
 * （押した後に視点を合わせ直さなくても、地図が見えている位置から逃げない）。
 *
 * **乱数も現在時刻も使わない**（`kizunaSeed` と同じ規律）。完全に重なった組の向きは
 * id の組のハッシュで決めるので、同じ配置からは毎回同じ結果になる。
 */

import { hash, type Point } from "./kizunaSeed";

/** この差までは「離れている」とみなす（押した直後の浮動小数の誤差で再発火させない）。 */
const TOLERANCE = 0.5;

/** 完全に重なっているとみなす距離。向きが決まらないので、ハッシュで決める。 */
const COINCIDENT = 1e-6;

/** 押し合いを繰り返す上限。収まらなくても、そこまでの結果を返す（少しでも離れていれば前進）。 */
const MAX_ROUNDS = 200;

/** 中心どうしが `gap` より近い組が 1 つでもあるか。ボタンを押せるかの判定に使う。 */
export function hasOverlap(points: Record<string, Point>, gap: number): boolean {
  const list = Object.values(points);
  for (let i = 0; i < list.length; i += 1) {
    for (let j = i + 1; j < list.length; j += 1) {
      if (Math.hypot(list[j].x - list[i].x, list[j].y - list[i].y) < gap - TOLERANCE) return true;
    }
  }
  return false;
}

/**
 * 近すぎる組を `gap` まで押し離した結果。**動かした個体だけ**を返す
 * （呼び手はそれをそのまま保存する。動かなかった個体を書き直さない）。
 *
 * 押した先で別の個体に近づけば、その組も次の周で押す。動くのは重なりに連なった個体だけ。
 */
export function untangle(points: Record<string, Point>, gap: number): Record<string, Point> {
  // 入力の鍵の順に依らない（同じ配置なら同じ結果）。
  const ids = Object.keys(points).sort();
  const work: Record<string, Point> = {};
  for (const id of ids) work[id] = { ...points[id] };
  const moved = new Set<string>();

  for (let round = 0; round < MAX_ROUNDS; round += 1) {
    let pushed = false;
    for (let i = 0; i < ids.length; i += 1) {
      for (let j = i + 1; j < ids.length; j += 1) {
        const a = work[ids[i]];
        const b = work[ids[j]];
        const dx = b.x - a.x;
        const dy = b.y - a.y;
        const distance = Math.hypot(dx, dy);
        if (distance >= gap - TOLERANCE) continue;

        let ux: number;
        let uy: number;
        if (distance < COINCIDENT) {
          const angle = ((hash(`${ids[i]}|${ids[j]}`) & 0xffff) / 0x10000) * Math.PI * 2;
          ux = Math.cos(angle);
          uy = Math.sin(angle);
        } else {
          ux = dx / distance;
          uy = dy / distance;
        }
        const push = (gap - distance) / 2;
        a.x -= ux * push;
        a.y -= uy * push;
        b.x += ux * push;
        b.y += uy * push;
        moved.add(ids[i]);
        moved.add(ids[j]);
        pushed = true;
      }
    }
    if (!pushed) break;
  }

  const result: Record<string, Point> = {};
  for (const id of moved) result[id] = work[id];
  return result;
}
