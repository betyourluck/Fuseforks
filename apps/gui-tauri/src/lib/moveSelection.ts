/**
 * 絆の地図の「移動のための複数選択」の規則（2026-11-05 利用者要望）。
 *
 * **会話の選択（`state.selectedAgentId`）とは別物。** Shift を押している間だけ生き、離すと消える。
 * 選ばれたノードは拡大せず、枠の色だけが変わる。選んだノードのどれかを Shift を押したまま
 * ドラッグすると、選んだ全員が一緒に動く（v-network-graph の「選択中のノードを掴むと選択した
 * 全員が追従する」仕組みをそのまま使う）。
 */

/**
 * 左ペインのグループ見出しを Shift で押したときの選択。**全員が既に選ばれていれば外し、
 * そうでなければ足す**（2 つのグループを続けて押すと両方が選ばれる）。地図に出ていない個体
 * （隠したグループ・消えた個体）は `members` に入れない — 呼び出し側が地図の上の個体へ絞る。
 */
export function toggleMembers(current: readonly string[], members: readonly string[]): string[] {
  if (members.length === 0) return [...current];
  const allIn = members.every((id) => current.includes(id));
  if (allIn) return current.filter((id) => !members.includes(id));
  return [...current, ...members.filter((id) => !current.includes(id))];
}

/** Shift の押下・解放を拾う鍵か。左右どちらの Shift も同じ。 */
export function isShiftKey(event: Pick<KeyboardEvent, "key">): boolean {
  return event.key === "Shift";
}
