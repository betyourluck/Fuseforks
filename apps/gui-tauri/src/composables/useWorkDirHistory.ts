/**
 * 作業フォルダの履歴（Spec 29 の追加・2026-08-08 利用者要望）。
 *
 * 起点は「毎回、探して選ばなくて済むので」。**取り除くのはネイティブの
 * フォルダ選択ダイアログを開いて辿る手間**であって、パスを覚えておく機構が
 * 欲しいわけではない — だから記録するのは**適用した瞬間だけ**で、
 * 入力しただけ・参照…で選んだだけでは積まない（履歴の意味を
 * 「使ったことがあるパス」に保つ）。
 *
 * 保存先は `localStorage`（`useUiSettings` / `useChatClear` と同じ棚）。
 * **村には持たせない** — `work_dir` は絶対パスで端末ごとに違い、村を配ると
 * 壊れる欄なので、履歴はなおさら端末の都合。
 */

import { reactive, watch } from "vue";

const STORAGE_KEY = "fuseforks.workDirHistory.v1";

/**
 * 覚えておく件数。**消すのではなく順序で表す**ので、同じ場所を往復する
 * 使い方（これが主）では上限に当たらない。
 */
export const WORK_DIR_HISTORY_MAX = 8;

/** 新しい順。`reactive` にするため配列そのものを保持する。 */
const history = reactive<string[]>(load());

/** 保存済みの履歴を読む。壊れていたら・文字列でない要素は捨てる。 */
function load(): string[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter((entry): entry is string => typeof entry === "string" && entry.length > 0)
      .slice(0, WORK_DIR_HISTORY_MAX);
  } catch {
    // 壊れた保存値でダイアログが開けなくなるほうが害が大きい。
    return [];
  }
}

watch(history, (next) => {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
  } catch {
    // 保存できなくてもその場の操作は続けられる。
  }
});

/**
 * 履歴へ積む純関数。**同じパスは消さずに先頭へ寄せる** — 同じ場所を何度も
 * 往復する使い方が主なので、重複は「消す対象」ではなく「順序で表すもの」。
 */
export function pushHistory(current: readonly string[], path: string): string[] {
  const trimmed = path.trim();
  if (!trimmed) return [...current];
  return [trimmed, ...current.filter((entry) => entry !== trimmed)].slice(
    0,
    WORK_DIR_HISTORY_MAX,
  );
}

export function useWorkDirHistory() {
  return {
    history,

    /** 適用が成功した後に呼ぶ。**押しただけでは積まない。** */
    remember(path: string): void {
      history.splice(0, history.length, ...pushHistory(history, path));
    },
  };
}
