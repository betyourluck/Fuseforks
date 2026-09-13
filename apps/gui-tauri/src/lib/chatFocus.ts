/**
 * 「/」キーで会話の入力欄へフォーカスを移す規則（2026-09-14 利用者要望）。
 *
 * **窓ごと聴く**のは Alt+↑↓（`agentNav.ts`）と同じだが、効かせる範囲は逆向きに決まる。
 * Alt+↑↓ は入力欄の中でも効かせたいので「通す場所」を列挙した。「/」は文字なので、
 * **入力できる要素の中では必ず文字として打たれる**べきで、奪ってよいのは
 * 「どこにも打ち込めない状態」のときだけ。
 *
 * 規則はここの純関数 3 本、配線は `App.vue`。
 */

/** 判定に要る鍵の状態だけ。`KeyboardEvent` をそのまま渡せる。 */
export interface FocusKeyEvent {
  key: string;
  altKey: boolean;
  ctrlKey: boolean;
  metaKey: boolean;
  isComposing: boolean;
}

/**
 * その打鍵が「入力欄へ移る」か。
 *
 * **Shift は見ない** — 「/」を Shift で打つ配列がある（独 Shift+7 など）。`key` が
 * 実際に出る文字なので、配列の差は `key` が吸う。Ctrl / Alt / Meta が付いたものは
 * 他のショートカットなので拾わない。IME の変換中は拾わない（確定前の打鍵を奪わない）。
 */
export function isFocusChatKey(event: FocusKeyEvent): boolean {
  if (event.isComposing) return false;
  if (event.ctrlKey || event.altKey || event.metaKey) return false;
  return event.key === "/";
}

/** 判定に要る要素の性質だけ。`Element` をそのまま渡せる。 */
export interface FocusableLike {
  tagName: string;
  isContentEditable?: boolean;
}

/**
 * 今のフォーカスが文字を打ち込める要素か。**ここにフォーカスがあれば「/」は奪わない。**
 *
 * `contenteditable` を数えるのは CodeMirror（`CodeEditor.vue`。条例・役職・設定の本文）の
 * 編集領域が `textarea` ではなく `contenteditable` の `div` だから。外すと本文に
 * 「/」を打てなくなる。
 */
export function isEditableElement(active: FocusableLike | null): boolean {
  if (active === null) return false;
  const tag = active.tagName.toUpperCase();
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
  return active.isContentEditable === true;
}

/**
 * 画面全体を覆う層（ダイアログ・確認・案内）が出ているかを見る選択子。
 *
 * ダイアログは覆いに **`bg-scrim`** を持つ約束（`dialogShell.test.ts` / `chatFocus.test.ts` が
 * 留める）、案内と確認は **`aria-modal="true"`** を持つ。覆いが出ている間に「/」で
 * 裏の入力欄へ移ると、見えない欄へ文字が入る。
 */
export const OVERLAY_SELECTOR = '.bg-scrim, [aria-modal="true"]';

/** `document` をそのまま渡せる。 */
export interface OverlayRoot {
  querySelector(selectors: string): unknown;
}

export function hasOpenOverlay(root: OverlayRoot): boolean {
  return root.querySelector(OVERLAY_SELECTOR) !== null;
}
