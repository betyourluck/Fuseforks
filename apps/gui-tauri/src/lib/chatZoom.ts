/**
 * 会話ペインの表示倍率（2026-09-09 利用者要望「メッセージウィンドウのフォントの
 * 大きさを変えたい。他の UI は変えない」）。
 *
 * **文字だけでなくペインごと拡大する**（CSS の `zoom`）。文字サイズだけを変える形は
 * 採っていない — 会話ペインは大きさを **px で直書き**した箇所が 33 あり
 * （`ChatPanel` 16 / `ChatInput` 15 / `GroundingNote` 1 / `ThinkingNote` 1）、
 * そこを相対単位へ書き換えても `px-3 py-2` の余白は固定のまま残るので、
 * 150% で文字が窮屈になる。余白も倍率へ乗せると `zoom` を手で書いたのと同じになる。
 *
 * **`zoom` が安全なのはこのペインに座標計算が 1 つも無いから**（`getBoundingClientRect`
 * も `elementFromPoint` も 0 箇所）。絆の地図に同じ手は使えない — あちらは Spec 21 の
 * drop が `elementFromPoint` で当たりを取っており、倍率が入ると座標系がずれる。
 *
 * **代償**: アバターとアイコンも一緒に大きくなる。要望の言葉（「フォントの大きさ」）
 * とはズレる部分で、文字だけを変えたくなったら上の 33 箇所の工事に戻る。
 */

/**
 * 選べる倍率。**離散にする** — スライダーにすると保存値が連続量になり、
 * 壊れた値の検査が「範囲内か」だけになって刻みの意図が消える（`theme` と同じ形で、
 * 集合に無い値は既定へ落とす）。
 *
 * 下限は 90%。本文は 12px なので 90% で 10.8px、これ以上小さくすると読めない。
 * 上限は 200%（Discord のチャット文字スケーリングが 12〜24px = 同じ幅）。
 */
export const CHAT_ZOOM_STEPS = [0.9, 1, 1.1, 1.25, 1.5, 1.75, 2] as const;

export type ChatZoom = (typeof CHAT_ZOOM_STEPS)[number];

/** 既定は等倍 = これまでの見え方。既定を変えると既存の村の画面が黙って変わる。 */
export const DEFAULT_CHAT_ZOOM: ChatZoom = 1;

/** 保存済みの値が選べる倍率のどれかか。手編集・旧版の欠落・小数の誤差を落とす。 */
export function isChatZoom(value: unknown): value is ChatZoom {
  return (
    typeof value === "number" && (CHAT_ZOOM_STEPS as readonly number[]).includes(value)
  );
}

/**
 * 画面に出す表記。**言語に追従させない** — `100%` は語ではなく量で、
 * 読み手の国で表記が変わると設定の値と画面の表示が突き合わせられなくなる
 * （ステータスバーの時計・版番号と同じ判断）。
 */
export function formatChatZoom(zoom: ChatZoom): string {
  return `${Math.round(zoom * 100)}%`;
}
