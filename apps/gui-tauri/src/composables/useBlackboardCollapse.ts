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
 * 鍵は `dir:state/name`（一覧の `:key` と同じ。`state` 無し = 直下は `dir:name` のままで、
 * 2026-09-16 の保存値を壊さない — Spec 54 凍結 10）。付箋が消えたら鍵も落とす（`prune`）ので、
 * 保存は一覧の大きさを超えて育たない。**`file move` で状態が変わると鍵も変わり、畳んでいた
 * 付箋は開く。これは意図した挙動** — 状態が動いた付箋は人が見るべきもので、鍵を `dir:name` に
 * すると重複（同名が 2 状態）の片方を畳むと両方が畳まれる。
 */

import { reactive, watch } from "vue";

const STORAGE_KEY = "fuseforks.blackboardCollapsed.v1";

export interface NoteRef {
  dir: string;
  name: string;
  state?: string;
}

export function noteKey(note: NoteRef): string {
  return note.state ? `${note.dir}:${note.state}/${note.name}` : `${note.dir}:${note.name}`;
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
     * 渡した付箋をまとめて畳む（`true`）/ 開く（`false`）。**渡さなかった付箋には
     * 触らない** — 絞り込み中に押しても、見えていない付箋の畳みは変わらない。
     */
    setCollapsed(notes: readonly NoteRef[], value: boolean): void {
      const targets = new Set(notes.map(noteKey));
      const rest = collapsed.keys.filter((k) => !targets.has(k));
      collapsed.keys = value ? [...rest, ...targets] : rest;
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
