/**
 * キーボードショートカットの一覧（チートシート。2026-10-27 利用者要望 — 外部レビューの
 * 「操作体系 ① ショートカット一覧」。Spec 不要と判断 — フロントだけで閉じ、コアも IPC も 0 行）。
 *
 * # これは表示のための写しで、鍵の実装ではない
 *
 * 鍵を実際に受けているのは各部品の handler（`agentNav.ts` / `chatFocus.ts` /
 * `chatSearch.ts` / `ChatInput.vue` / `CodeEditor.vue` …）。ここはそれを人に見せる写しなので、
 * **handler の鍵を変えると、ここは黙って嘘になる**。`shortcutsWiring.test.ts` が項目ごとに
 * 「その鍵を受けている実装の字面」を突き合わせて留める（項目を足して根拠を書かないと赤、
 * handler の鍵を変えると赤）。
 *
 * 割り当てを変えられるようにする（利用者 —「将来的には変えられたら」）のは後日。
 * そのときはここが写しから正本へ変わる。
 *
 * # 置き場
 *
 * 一覧はシステム設定 ＞ ユーザーインターフェース ＞ キーボードショートカット。「?」で
 * そのページを開く（配線は `App.vue`。奪う条件は「/」と同じ）。
 */

import type { FocusKeyEvent } from "./chatFocus";

/**
 * 一覧に出す鍵の部品。表示は [`keyLabel`] が OS ごとに決める。
 *
 * `Mod` は CodeMirror の記法と同じ意味（mac は Cmd、他は Ctrl）。`UpDown` / `LeftRight` は
 * 「どちらか」を 1 つの札で出すための組（`Alt + ↑↓` を 2 行に割らない）。
 */
export type KeyToken =
  | "Mod"
  | "Alt"
  | "Shift"
  | "Enter"
  | "Esc"
  | "Tab"
  | "Up"
  | "Down"
  | "Left"
  | "Right"
  | "UpDown"
  | "LeftRight"
  | "/"
  | "?"
  | "@"
  | "@@"
  | "F"
  | "R"
  | "S"
  | "V";

/** 同時に押す鍵の並び。 */
export type KeyCombo = readonly KeyToken[];

export interface Shortcut {
  /** 辞書 `settings.shortcuts.item.<id>` と、走査テストの根拠の表の鍵。 */
  id: string;
  /** どれか 1 つを押す（画面では「または」で並べる）。 */
  keys: readonly KeyCombo[];
}

export interface ShortcutSection {
  /** 辞書 `settings.shortcuts.section.<id>`。 */
  id: string;
  items: readonly Shortcut[];
}

/**
 * 一覧の中身。**並びは画面の並び**で、よく使う順（どこでも効く → 会話 → 一覧と地図 →
 * エディタ → ダイアログ）。
 *
 * マウスの操作（地図の右クリック・仕切りのダブルクリック）は載せない — キーの一覧なので。
 * IME の変換中はどの鍵も奪わない（各 handler が `isComposing` を見る）ことは、ページの
 * 説明文に 1 度だけ書く。
 */
export const SHORTCUT_SECTIONS: readonly ShortcutSection[] = [
  {
    id: "global",
    items: [
      { id: "showShortcuts", keys: [["?"]] },
      { id: "focusChat", keys: [["/"]] },
      { id: "selectAgent", keys: [["Alt", "UpDown"]] },
      { id: "searchChat", keys: [["Mod", "F"]] },
    ],
  },
  {
    id: "chatInput",
    items: [
      { id: "send", keys: [["Enter"]] },
      { id: "newline", keys: [["Shift", "Enter"]] },
      { id: "pasteImage", keys: [["Mod", "V"]] },
      { id: "completePath", keys: [["@"]] },
      { id: "quoteMessage", keys: [["@@"]] },
      { id: "pickCandidate", keys: [["UpDown"]] },
      { id: "confirmCandidate", keys: [["Enter"], ["Tab"]] },
      { id: "closeCompletion", keys: [["Esc"]] },
    ],
  },
  {
    id: "chatSearch",
    items: [
      { id: "nextMatch", keys: [["Enter"]] },
      { id: "prevMatch", keys: [["Shift", "Enter"]] },
      { id: "closeSearch", keys: [["Esc"]] },
    ],
  },
  {
    id: "village",
    items: [
      { id: "clearAgentSearch", keys: [["Esc"]] },
      { id: "cancelLink", keys: [["Esc"]] },
      { id: "resizePane", keys: [["LeftRight"], ["UpDown"]] },
    ],
  },
  {
    id: "editor",
    items: [
      { id: "saveFile", keys: [["Mod", "S"]] },
      { id: "findInFile", keys: [["Mod", "F"]] },
      { id: "noReload", keys: [["Mod", "R"]] },
      { id: "sendAssist", keys: [["Mod", "Enter"]] },
    ],
  },
  {
    id: "dialogs",
    items: [
      { id: "cancelConfirm", keys: [["Esc"]] },
      { id: "tourNext", keys: [["Right"], ["Enter"]] },
      { id: "tourBack", keys: [["Left"]] },
      { id: "tourClose", keys: [["Esc"]] },
    ],
  },
];

/** 鍵の表記を分ける OS の区別。分かれるのは修飾キーの名前だけ。 */
export type Platform = "mac" | "other";

/**
 * UA から OS を読む。Tauri の WebView は mac で `Macintosh`、Windows で `Windows NT`、
 * Linux で `X11; Linux` を名乗る。**判定に使うのは表示だけ** — 鍵を受ける側は
 * `ctrlKey` / `metaKey` を両方見ているので、ここを外しても動作は変わらない。
 */
export function platformOf(userAgent: string): Platform {
  return /Mac|iPhone|iPad/.test(userAgent) ? "mac" : "other";
}

/** 鍵 1 つの表記。mac は記号（⌘ ⌥ ⇧）、他は名前（Ctrl / Alt / Shift）。 */
export function keyLabel(token: KeyToken, platform: Platform): string {
  const mac = platform === "mac";
  switch (token) {
    case "Mod":
      return mac ? "⌘" : "Ctrl";
    case "Alt":
      return mac ? "⌥" : "Alt";
    case "Shift":
      return mac ? "⇧" : "Shift";
    case "Enter":
      return "Enter";
    case "Esc":
      return "Esc";
    case "Tab":
      return "Tab";
    case "Up":
      return "↑";
    case "Down":
      return "↓";
    case "Left":
      return "←";
    case "Right":
      return "→";
    case "UpDown":
      return "↑↓";
    case "LeftRight":
      return "←→";
    case "/":
    case "?":
    case "@":
    case "@@":
    case "F":
    case "R":
    case "S":
    case "V":
      return token;
  }
}

/**
 * その打鍵が「ショートカット一覧を開く」か。
 *
 * **Shift は見ない** — 「?」は多くの配列で Shift と一緒に打つ文字で、`key` が実際に出た文字を
 * 運ぶ（「/」と同じ扱い）。Ctrl / Alt / Meta が付いたものは他のショートカットなので拾わない。
 * IME の変換中は拾わない。**どこで奪わないか**（入力欄・覆いの層・統計画面）は配線側
 * （`App.vue`）が「/」と同じ条件で決める。
 */
export function isShortcutsKey(event: FocusKeyEvent): boolean {
  if (event.isComposing) return false;
  if (event.ctrlKey || event.altKey || event.metaKey) return false;
  return event.key === "?";
}
