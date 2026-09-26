/**
 * 判断役の編集ダイアログを開いているか（Spec 62 D10）。
 *
 * 入口は 2 つ（左ペインの一覧 / 地図のノード）で、ダイアログは `App.vue` に 1 つだけ置く。
 * **`selectedAgentId` は使わない** — 会話ペインと `useOrchestrator` はそれがサーヴァントの
 * id であることを前提にしており、判断役の id を入れると会話ペインが壊れる。
 */
import { readonly, ref } from "vue";

import type { AgentId } from "../types";

const openId = ref<AgentId | null>(null);

export function useJudgeDialog() {
  return {
    /** 開いている判断役の id。`null` なら閉じている。 */
    openId: readonly(openId),
    open(id: AgentId): void {
      openId.value = id;
    },
    close(): void {
      openId.value = null;
    },
  };
}
