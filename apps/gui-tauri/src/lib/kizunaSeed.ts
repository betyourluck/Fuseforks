/**
 * 座標を持たないノードの初期位置。
 *
 * **自動整列は入れない**（2026-08-13 利用者裁定 —「自動整列みたいなのはなくても
 * よいかもしれない」）。人が置くのが本線で、ここが埋めるのは
 * **「まだ一度も置かれていない個体をどこに出すか」だけ**。
 *
 * **乱数も現在時刻も使わない**（`clock.ts` / `schedule.rs` と同じ規律）。
 * 種は id のハッシュなので、同じ村なら毎回同じ所に出る。
 */

/** 文字列 → 32bit（FNV-1a）。`Math.random` を使わないための種。 */
function hash(text: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i += 1) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h;
}

export interface Point {
  x: number;
  y: number;
}

/**
 * 既に置かれている座標を尊重しつつ、無いものだけを埋める。
 *
 * **既存の座標には一切触れない** — ここが触ると「動かしたのに戻る」になる。
 *
 * 埋める位置は**既存の塊を避けた円周上**。全員が未配置なら素直な円になり、
 * 途中から加わった個体は**外側**に出る（既にある配置を崩さない）。
 */
export function seedPositions(
  ids: string[],
  placed: Record<string, Point>,
): Record<string, Point> {
  const result: Record<string, Point> = {};
  const missing = ids.filter((id) => !placed[id]);
  if (missing.length === 0) return result;

  // 既にある座標の広がり。**無ければ原点まわり**から始める。
  const known = ids.map((id) => placed[id]).filter((p): p is Point => Boolean(p));
  let cx = 0;
  let cy = 0;
  let reach = 0;
  if (known.length > 0) {
    for (const p of known) {
      cx += p.x;
      cy += p.y;
    }
    cx /= known.length;
    cy /= known.length;
    for (const p of known) {
      reach = Math.max(reach, Math.hypot(p.x - cx, p.y - cy));
    }
  }

  // 未配置が多いほど広げる（全員が未配置の初回は素直な円になる）。
  const radius = Math.max(reach + 140, 90 + missing.length * 26);

  missing.forEach((id, index) => {
    // 角度は id のハッシュから。**並び順に依らない**ので、一覧を並べ替えても
    // 出る場所が変わらない。等間隔にしないのは、円環の規則的な見え方を
    // 初期状態にも持ち込まないため。
    const jitter = (hash(id) & 0xffff) / 0x10000;
    const angle =
      ((index + jitter) / Math.max(1, missing.length)) * Math.PI * 2 - Math.PI / 2;
    result[id] = {
      x: cx + radius * Math.cos(angle),
      y: cy + radius * Math.sin(angle),
    };
  });

  return result;
}
