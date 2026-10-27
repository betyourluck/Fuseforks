/**
 * サーヴァント一覧の絞り込みの問い（2026-10-27）。
 *
 * **保存しない**（黒板の持ち主の絞り込みと同じ）。再起動したときに一覧の一部だけが
 * 見える状態から始めない。一覧（`AgentList`）と Alt+↑↓（`App.vue`）が同じ問いを読む —
 * 別々に持つと、画面に出ていない個体へ Alt+↓ で飛ぶ。
 */

import { computed, ref } from "vue";

import { searchTokens } from "../lib/agentSearch";

const query = ref("");
const active = computed(() => searchTokens(query.value).length > 0);

export function useAgentSearch() {
  return {
    query,
    /** 語が 1 つでもあるか（空白だけの問いは絞り込んでいない）。 */
    active,
    clear(): void {
      query.value = "";
    },
  };
}
