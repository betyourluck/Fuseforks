/**
 * サーヴァント一覧の選択をキーで動かす規則（2026-08-08 利用者要望・マイルストーン 8）。
 *
 * **鍵は Alt + ↑↓。** 3 つの候補から絞った経緯:
 * - **Shift + ↑↓ は不可** — `textarea` で行単位の**範囲選択**に使われている。
 *   入力欄で最もよく使う編集操作を奪う
 * - **Ctrl + ↑↓ は不可寄り** — macOS では Mission Control が OS 側で吸うので
 *   アプリまで届かない。3 OS で配布するので、片方だけ効かない鍵は採らない
 * - **Alt + ↑↓** は Windows / Linux の `textarea` で未使用。macOS の Option+↑↓ は
 *   段落移動だが、チャット入力で段落単位の移動はまず使わない
 *
 * **入力欄にフォーカスがあるときも効く**のが要件（利用者 —「入力欄にフォーカス
 * している状態で切り替えられないと、あまり意味がない」）。ゆえに窓ごと聴く形になり、
 * **どこで効かせないか**を閉じた許容で決める（[`isNavigableFocus`]）。
 */

import type { AgentId } from "../types";

/** 並べ替えに必要な最小限。`AgentSnapshot` をそのまま渡せる。 */
export interface NavigableAgent {
  id: AgentId;
  order: number;
}

/**
 * 一覧に出る順序。**`AgentList.vue` と同じ実装を共有する。**
 *
 * 別々に整列すると、Alt+↓ が画面の並びと違う順で飛ぶ。**同じ規則を 2 箇所に
 * 書かない**（`roleLabel` を 1 実装に閉じたのと同じ判断）。
 */
export function inListOrder<T extends NavigableAgent>(agents: readonly T[]): T[] {
  return [...agents].sort((a, b) => a.order - b.order);
}

/**
 * 次に選ぶ id。**端で巻き戻る**（入力欄のパス補完が `% count` で巻き戻るのと同じ作法）。
 *
 * 選択が無い・一覧に無い id のときは、下なら先頭・上なら末尾から入る
 * （「まだ選んでいない」状態から 1 打で入れる）。
 */
export function nextAgentId(
  agents: readonly NavigableAgent[],
  current: AgentId | null,
  delta: 1 | -1,
): AgentId | null {
  const ordered = inListOrder(agents);
  if (ordered.length === 0) return null;

  const index = current === null ? -1 : ordered.findIndex((a) => a.id === current);
  if (index < 0) return (delta === 1 ? ordered[0] : ordered[ordered.length - 1]).id;

  const next = (index + delta + ordered.length) % ordered.length;
  return ordered[next].id;
}

/** 判定に要る鍵の状態だけ。`KeyboardEvent` をそのまま渡せる。 */
export interface NavKeyEvent {
  key: string;
  altKey: boolean;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
}

/**
 * その打鍵が選択の移動か。**Alt だけが押されていること**まで見る。
 *
 * 他の修飾を許すと、`Ctrl+Alt+↑`（一部の環境で画面回転）や
 * `Shift+Alt+↑`（範囲選択との合わせ技）を奪う。**足りない条件で拾うより、
 * 余った条件で見送るほうが安全**な種類の判定。
 */
export function agentNavDelta(event: NavKeyEvent): 1 | -1 | null {
  if (!event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return null;
  if (event.key === "ArrowDown") return 1;
  if (event.key === "ArrowUp") return -1;
  return null;
}

/**
 * そのフォーカス位置で効かせてよいか。**閉じた許容**（除外リストではない）。
 *
 * 通すのは 2 つだけ — **何もフォーカスしていない**（`body` / `null`）と
 * **チャット入力欄**（`[data-chat-input]`）。
 *
 * 除外リストにしない理由は、**新しい入力面が増えるたびに黙って奪う側へ倒れる**から。
 * 特に `CodeEditor.vue`（CodeMirror）は既定の keymap で **Alt+↑↓ に行の移動**を
 * 持っており、役職ダイアログとシステム設定で使われている。許容側で書けば、
 * ダイアログが 1 つ増えても既定で安全なまま。
 */
export function isNavigableFocus(active: Element | null): boolean {
  if (active === null) return true;
  if (active.tagName === "BODY") return true;
  return active.hasAttribute("data-chat-input");
}
