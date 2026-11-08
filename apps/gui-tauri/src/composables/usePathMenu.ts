/**
 * 発話の中のファイルの位置を押したときのメニュー（2026-11-08）。会話ペインと黒板が共有する。
 *
 * 押した側は「書かれた文字列」と「相対パスの起点（作業フォルダ）」だけを渡し、解決はコアが持つ
 * （`resolve_message_path`）。メニューは `App.vue` の `PathMenuHost` が 1 つだけ描く — 会話ペインは
 * `zoom` の場所で、ここで座標を扱わないため（押した位置は画面全体の座標のまま運ぶ）。
 */
import { reactive } from "vue";
import * as ipc from "../lib/ipc";
import { parsePathRef, type PathRef } from "../lib/messagePath";
import type { ResolvedPath } from "../types";

interface PathMenuState {
  open: boolean;
  /** 押した位置（画面全体の座標）。 */
  x: number;
  y: number;
  /** 発話に書かれた文字列（表示用）。 */
  raw: string;
  ref: PathRef | null;
  /** 解決の結果。`undefined` = 解決中 / `null` = 見つからない。 */
  resolved: ResolvedPath | null | undefined;
  /** 操作の失敗（エディタが起動できなかった等）。 */
  error: string;
  /** 解決の世代。閉じた後や開き直した後に、古い解決の結果で上書きしない。 */
  seq: number;
}

const state = reactive<PathMenuState>({
  open: false,
  x: 0,
  y: 0,
  raw: "",
  ref: null,
  resolved: undefined,
  error: "",
  seq: 0,
});

/**
 * メニューを開いて解決を始める。形がパスでなければ何もしない（印は `renderMarkdown` が付けたものだけ
 * なので、ここに来るのは形の判定を通ったものだけのはずだが、もう一度見る）。
 */
function open(event: MouseEvent, raw: string, workDir: string | null): void {
  const ref = parsePathRef(raw);
  if (!ref) return;
  const seq = state.seq + 1;
  Object.assign(state, {
    open: true,
    x: event.clientX,
    y: event.clientY,
    raw,
    ref,
    resolved: undefined,
    error: "",
    seq,
  });
  ipc
    .resolveMessagePath(workDir, ref.path)
    .then((resolved) => {
      if (state.seq === seq) state.resolved = resolved;
    })
    .catch(() => {
      if (state.seq === seq) state.resolved = null;
    });
}

function close(): void {
  state.open = false;
  state.seq += 1;
}

export function usePathMenu() {
  return { state, open, close };
}
