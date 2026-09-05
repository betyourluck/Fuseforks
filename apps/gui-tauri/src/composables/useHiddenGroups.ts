/**
 * 隠しているグループの集合（Spec 51 / `group_contract` 凍結 1）。
 *
 * **村には持たせない** — 地図の表示フィルタは端末の見え方で、村に入れると配った先で
 * 「なぜか半分しか見えない」になる（`autoFitOnResize` と同じ棚）。配列を持つので
 * `useUiSettings`（真偽値だけ）ではなく `useWorkDirHistory` と同じ形の棚。
 *
 * 知らない id（削除済み・別の村）は読み飛ばす — `isHidden` は村に居るグループにしか
 * 効かないので、残っていても害は無いが、保存値が膨らみ続けないよう読み込みで落とす
 * のではなく**そのまま持つ**（別の村を開き直したときに隠し方が戻る）。
 *
 * **隠すのは見え方だけ**で、配送にも一括起動にも効かない（凍結 6）。
 */

import { reactive, watch } from "vue";
import type { GroupId } from "../types";

const STORAGE_KEY = "fuseforks.hiddenGroups.v1";

/** 保存値を読む純関数。壊れていたら空。文字列でない要素は捨てる。 */
export function parseHidden(raw: string | null): GroupId[] {
  if (!raw) return [];
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed)) return [];
    return [...new Set(parsed.filter((v): v is string => typeof v === "string" && v.length > 0))];
  } catch {
    return [];
  }
}

/** 隠す / 出すの切り替え（純関数）。 */
export function toggleHidden(current: readonly GroupId[], id: GroupId): GroupId[] {
  return current.includes(id) ? current.filter((v) => v !== id) : [...current, id];
}

function load(): GroupId[] {
  try {
    return parseHidden(localStorage.getItem(STORAGE_KEY));
  } catch {
    // 壊れた保存値で一覧が開けなくなるほうが害が大きい。
    return [];
  }
}

const hidden = reactive<GroupId[]>(load());

watch(hidden, (next) => {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
  } catch {
    // 保存できなくてもその場の操作は続けられる。
  }
});

export function useHiddenGroups() {
  return {
    hidden,
    isHidden(id: GroupId): boolean {
      return hidden.includes(id);
    },
    toggle(id: GroupId): void {
      hidden.splice(0, hidden.length, ...toggleHidden(hidden, id));
    },
  };
}
