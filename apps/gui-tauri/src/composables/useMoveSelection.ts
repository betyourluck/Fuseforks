import { ref } from "vue";
import { toggleMembers } from "../lib/moveSelection";

/**
 * 移動のための複数選択の状態。**地図（TopologyMap）と左ペイン（AgentList）が共有する**ので
 * モジュールに 1 つだけ持つ。保存しない — Shift を離した時点で消える一時の状態。
 *
 * - `shiftHeld` — Shift を押しているか。地図の `node.selectable` がこれを読むので、Shift なしの
 *   クリックではライブラリが選択を作らない（クリックの後で消す形にすると、ライブラリの
 *   `update:selectedNodes` が後から届いて選択が戻る）
 * - `selection` — 選んだノードの id（地図の `v-model:selected-nodes`）
 */
const shiftHeld = ref(false);
const selection = ref<string[]>([]);

export function useMoveSelection() {
  return {
    shiftHeld,
    selection,
    /** 左ペインのグループ見出しを Shift で押した。地図に出ている個体だけを渡す。 */
    toggleGroup(members: readonly string[]): void {
      selection.value = toggleMembers(selection.value, members);
    },
    clear(): void {
      if (selection.value.length) selection.value = [];
    },
  };
}
