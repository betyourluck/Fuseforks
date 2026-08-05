/**
 * アバター描画の共有ロジック。
 *
 * 複数の画面が同じ規則で色と頭文字を出す。場所ごとに実装すると、同じ
 * エージェントの色が画面ごとに食い違い、「色 = 個体」という視覚の手がかりが壊れる。
 *
 * # ここが持つのは**フォールバックだけ**
 *
 * アバターの規則は 2 段ある:
 *
 * 1. **設定された画像があればそれを出す**（`state.icons[agentId]`）
 * 2. 無ければ、ここの色と頭文字で円を描く
 *
 * **1 段目はこのモジュールに無く、画面ごとに手で書かれている**（`state` を
 * 読む必要があるため）。使う側は必ず 2 段とも書くこと — **1 段目を落とすと、
 * その画面だけアイコンを設定しても頭文字のままになる**。実際に
 * `CommandApprovalDialog.vue` を書いたときに落とした（2026-08-06。実機の指摘）。
 *
 * 現在 2 段とも書いている場所（**総数ではなく列挙する** — 数だけ書くと、
 * 増やしたときに数が腐って grep で見つからない）:
 *
 * - `ChatPanel.vue`（会話の発話と、宛先セレクタ）
 * - `TopologyMap.vue`（サーヴァントの絆のノード）
 * - `AgentList.vue`（カード。`AgentCard.vue` へ prop で渡す）
 * - `AgentSettingsDialog.vue`（設定のプレビュー）
 * - `CommandApprovalDialog.vue`（コマンド承認の見出し）
 * - `SettingsDialog.vue`（利用者のアイコン。ここだけ `state.userIcon`）
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
