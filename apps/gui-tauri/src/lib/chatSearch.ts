/**
 * 会話ログの検索（2026-10-15 利用者要望。Spec 不要と判断 — フロントだけで閉じ、
 * コアも IPC も 1 行も変えない）。
 *
 * # 置き場
 *
 * 規則はここの純関数、配線は `ChatPanel.vue`。**鍵は Ctrl+F**（mac は Cmd+F）で、
 * エディタの検索パネル（`CodeEditor.vue` の `Mod-f`）と同じ意味に揃える。
 * 窓ごと聴くのは Alt+↑↓ / 「/」と同じだが、奪う条件は会話ペインが見えていて
 * 覆いの層（ダイアログ・確認・案内）が出ていないときだけ。エディタの中の `Ctrl+F` は
 * CodeMirror が先に受けて `defaultPrevented` を立てるので、ここでは拾わない。
 *
 * # 何を探すか
 *
 * **いま画面に出ている発話の本文**（絞り込みと表示クリアを通った後）。ツール行は
 * 対象にしない — 本文は開いたときにだけ引く中身で、画面には名前と理由しか無い。
 * 照合は大文字小文字を区別しない部分一致。正規表現は受けない（打った文字が
 * そのまま探される、が入力欄の約束）。
 *
 * # 現在位置
 *
 * 一致の列は画面の並び（古い順）。新しい問いでは**末尾 = 最も新しい一致**から始める —
 * 会話は末尾へ伸びるので、探したいものは新しい側にあることが多い。
 * Enter は次（新しい側）、Shift+Enter は前（古い側）で、端で巻き戻る。
 */

/** 判定に要る鍵の状態だけ。`KeyboardEvent` をそのまま渡せる。 */
export interface SearchKeyEvent {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  isComposing: boolean;
}

/**
 * その打鍵が「会話を検索」か。Ctrl+F または Cmd+F（どちらか片方だけ）。
 * Shift / Alt が付いたもの（Ctrl+Shift+F など）は別のショートカットなので拾わない。
 * IME の変換中は拾わない。
 */
export function isChatSearchKey(event: SearchKeyEvent): boolean {
  if (event.isComposing) return false;
  if (event.altKey || event.shiftKey) return false;
  if (event.ctrlKey === event.metaKey) return false;
  return event.key.toLowerCase() === "f";
}

/** 検索の対象 1 件。`id` は発話 ID（行の `data-chat-row` と同じ値）。 */
export interface Searchable {
  id: string;
  text: string;
}

/** 照合用に正規化する。空白だけの問いは「問い無し」。 */
export function normalizeQuery(query: string): string {
  return query.trim().toLowerCase();
}

/**
 * 問いに一致する発話 ID を、渡された順（画面の並び）で返す。
 * 問いが空なら一致なし。
 */
export function findMatches(items: readonly Searchable[], query: string): string[] {
  const needle = normalizeQuery(query);
  if (!needle) return [];
  return items.filter((item) => item.text.toLowerCase().includes(needle)).map((item) => item.id);
}

/**
 * 1 本のテキストの中で問いが現れる区間（`[start, end)`、コードポイントではなく
 * UTF-16 の添字 — `Range.setStart` に渡す単位）。ハイライトの材料。
 *
 * 小文字化で長さが変わる文字（トルコ語の İ など）が混ざると添字がずれるので、
 * そのときは大文字小文字を区別する照合へ落とす — ずれた位置を光らせるより、
 * 一致を取りこぼすほうが害が小さい。
 */
export function matchRanges(text: string, query: string): Array<[number, number]> {
  const trimmed = query.trim();
  if (!trimmed) return [];
  const lowered = text.toLowerCase();
  // 長さが保たれるときだけ両方を小文字で比べる。保たれないときは**原文どうし**
  // （問いも小文字化しない — 片方だけ小文字にすると大文字の問いが永遠に外れる）。
  const [haystack, needle] =
    lowered.length === text.length ? [lowered, trimmed.toLowerCase()] : [text, trimmed];
  const ranges: Array<[number, number]> = [];
  let from = 0;
  for (;;) {
    const at = haystack.indexOf(needle, from);
    if (at < 0) break;
    ranges.push([at, at + needle.length]);
    from = at + needle.length;
  }
  return ranges;
}

/**
 * 新しい一致の列に対する現在位置。**末尾（最も新しい一致）から始める。**
 * 一致が無ければ `-1`。
 */
export function initialCurrent(total: number): number {
  return total > 0 ? total - 1 : -1;
}

/**
 * 現在位置を 1 つ動かす。`delta` は +1（次 = 新しい側）か -1（前 = 古い側）で、
 * 端で巻き戻る。一致が無ければ `-1` のまま。
 */
export function stepCurrent(current: number, total: number, delta: 1 | -1): number {
  if (total <= 0) return -1;
  if (current < 0 || current >= total) return initialCurrent(total);
  return (current + delta + total) % total;
}
