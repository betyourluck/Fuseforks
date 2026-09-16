/**
 * 黒板タブの付箋の畳み（2026-09-16 利用者要望「付箋を畳めるようにしたい」）。
 *
 * **畳むのは表示だけで、付箋の中身にも並びにも触らない。** 見出し行（名前・バッジ・
 * 時刻・ごみ箱）は残し、本文だけを隠す。
 *
 * ## 部品の中に持てない理由
 *
 * 黒板タブは `App.vue` で `v-if` により切り替わるので、タブを離れた瞬間に部品ごと
 * 捨てられる。畳んだ状態を `BlackboardPane` の `ref` に置くと**作業状況タブを見て戻る
 * たびに全部開く**。作業状況の表示クリア（`useWaveClear`）と同じくモジュール単位の
 * 状態にし、保存先も同じ棚（`localStorage`）。**端末の見え方**であって村の内容物では
 * ないので `world.json` には混ぜない。
 *
 * 鍵は `dir:name`（一覧の `:key` と同じ）。付箋が消えたら鍵も落とす（`prune`）ので、
 * 保存は一覧の大きさを超えて育たない。
 */

import { reactive, watch } from "vue";

const STORAGE_KEY = "fuseforks.blackboardCollapsed.v1";

export interface NoteRef {
  dir: string;
  name: string;
}

export function noteKey(note: NoteRef): string {
  return `${note.dir}:${note.name}`;
}

/** 保存済みの鍵を読む。壊れていたら・文字列でない要素はそのぶんだけ捨てる。 */
function load(): string[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((key): key is string => typeof key === "string");
  } catch {
    // 壊れた保存値でタブが開けなくなるほうが害が大きい（useWaveClear と同じ判断）。
    return [];
  }
}

const collapsed = reactive<{ keys: string[] }>({ keys: load() });

watch(
  () => collapsed.keys,
  (next) => {
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
    } catch {
      // 保存できなくてもその場の畳みは効く。
    }
  },
);

export function useBlackboardCollapse() {
  return {
    /** 本文を隠している付箋か。 */
    isCollapsed(note: NoteRef): boolean {
      return collapsed.keys.includes(noteKey(note));
    },

    /** 畳む ⇄ 開く。 */
    toggle(note: NoteRef): void {
      const key = noteKey(note);
      collapsed.keys = collapsed.keys.includes(key)
        ? collapsed.keys.filter((k) => k !== key)
        : [...collapsed.keys, key];
    },

    /**
     * 一覧に無い付箋の鍵を落とす。読み直すたびに呼ぶ。
     * 何も変わらないときは配列を差し替えない（保存の書き込みを増やさない）。
     */
    prune(notes: readonly NoteRef[]): void {
      const live = new Set(notes.map(noteKey));
      const next = collapsed.keys.filter((k) => live.has(k));
      if (next.length !== collapsed.keys.length) collapsed.keys = next;
    },
  };
}
