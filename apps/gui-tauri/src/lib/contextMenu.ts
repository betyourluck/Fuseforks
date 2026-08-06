/**
 * WebView の既定右クリックメニューを配布ビルドで抑止するかの判定。
 *
 * # なぜ抑止するか
 *
 * 既定メニューは WebView（ブラウザ）由来で、「最新の情報に更新」「名前を付けて
 * 保存」「印刷」が乗っている。**配布したアプリの利用者がここから再読み込みを
 * 押すと画面が作り直される** — 入力途中の依頼文・開いているダイアログ・
 * スクロール位置・選択中のサーヴァントは、どれも画面側の状態なので消える。
 *
 * ただし**村そのものは消えない**（実測 2026-08-06）。コアは Tauri の Rust 側に
 * 居て WebView の再読み込みでは落ちず、`initialize()` が `refreshAll()` /
 * `listMessages` / `list_plan_waves` で投影を張り直す（Spec 08 の実機確認
 * 「GUI を再読み込みしても波が残る」がまさにこれ）。稼働中のサーヴァントは
 * 走り続け、会話ログも戻る。**壊れるのは画面の状態だけで、村の状態ではない。**
 *
 * # なぜ dev では抑止しないか
 *
 * 右クリック →「検証」が開発者ツールの入口だから。`import.meta.env.DEV` は
 * Vite が dev サーバーと配布ビルドで切り替える定数で、判定はビルド時に畳まれる。
 *
 * # なぜ入力欄と選択テキストでは残すか
 *
 * コピー / 貼り付けの導線がそこにしか無い。**`[contenteditable]` を選択子に
 * 含めるのは CodeMirror のため** — 役職ダイアログとシステム設定の
 * `CodeEditor.vue` は CodeMirror 6 で、編集領域は `<textarea>` ではなく
 * `contenteditable` を持つ `div.cm-content`。ここを外すと**役職の本文を
 * 貼り付けられなくなる**（型検査にもテストにも掛からない種類の退行）。
 *
 * # なぜ F5 / Ctrl+R は塞がないか
 *
 * Kataribe は右クリックと同時に塞いでいるが、あちらは**再読み込みがゲーム
 * セッションの表示を吹き飛ばす**（コアが画面側に居る）ため。この村では上記の
 * とおりコアが別に居るので、再読み込みは復帰する操作であって破壊ではない。
 * むしろ Spec 08 の実機確認は再読み込みで投影が張り直ることを**確かめる手順**
 * なので、塞ぐとその確認が配布ビルドでできなくなる。
 * **同じ処方でも、コアがどちら側に居るかで要否が変わる。**
 */

/**
 * ネイティブメニューを残す場所の選択子。
 *
 * `[contenteditable]` は**属性の有無**で一致する（値は見ない）ので、
 * readonly な `CodeEditor`（CodeMirror が `contenteditable="false"` を置く）でも
 * 一致する。読めるものはコピーさせてよいので、これは意図どおり。
 */
const EDITABLE_SELECTOR = "input, textarea, [contenteditable]";

/** `closest` だけを使う最小の受け口。DOM 型に依存しない（テストは node 環境）。 */
interface ClosestTarget {
  closest(selectors: string): unknown;
}

/**
 * この右クリックでネイティブメニューを残すか。
 *
 * `instanceof HTMLElement` で判定しないのは、テストが node 環境（DOM 無し）で
 * 走るため（`kizunaDrop.ts` の `dropPoint` と同じ理由）。`closest` を持たない
 * 対象（テキストノード・`document`・null）は入力欄ではないので抑止側へ倒す。
 */
export function keepsNativeMenu(target: unknown, hasSelection: boolean): boolean {
  // 選択したうえでの右クリックは「これをコピーしたい」以外にほぼ無い。
  // 場所を問わず残す（ラベルや会話ログの本文がここに当たる）。
  if (hasSelection) return true;

  const element = target as Partial<ClosestTarget> | null | undefined;
  if (typeof element?.closest !== "function") return false;
  return Boolean(element.closest(EDITABLE_SELECTOR));
}
