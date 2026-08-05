/**
 * アバター描画の共有ロジック。
 *
 * 会話・サーヴァントの絆・エージェント一覧の 3 箇所が同じ規則で色と頭文字を出す。
 * 場所ごとに実装すると、同じエージェントの色が画面ごとに食い違い、
 * 「色 = 個体」という視覚の手がかりが壊れる。
 */

/**
 * 名前から決まるアバター背景色。
 *
 * 乱数や登録順で決めると、再起動のたびに色が入れ替わって見分けの手がかりが消える。
 * 名前のハッシュなら、同じエージェントは常に同じ色になる。
 */
export function avatarHue(name: string): string {
  let hash = 0;
  for (const char of name) hash = (hash * 31 + char.codePointAt(0)!) % 360;
  // 明度と彩度は `style.css` が持つ（テーマで変わる）。**ここに数値を書かない** —
  // アバターの上に載る文字は `text-surface-0` で、その明度はテーマで反転する。
  // 背景の明度だけが固定だと、片方のテーマで頭文字が読めなくなる。
  return `oklch(var(--avatar-l) var(--avatar-c) ${hash})`;
}

/** アバターに出す頭文字。 */
export function avatarInitial(name: string): string {
  return name.slice(0, 1);
}
