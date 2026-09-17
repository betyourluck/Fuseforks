<script setup lang="ts">
/**
 * 中央下段ペインのタブ（黒板 | 作業状況）。
 *
 * タブの状態は App.vue が持ち、各ペイン（BlackboardPane / PlanWavePane）は
 * ヘッダにこれを差して select を親へ中継するだけ。ペイン自身がタブ状態を
 * 持たないのは、非表示側のペインは v-if で丸ごと居なくなるため。
 *
 * **承認待ちの印**（2026-09-17）: 計画の確認（Spec 43）で止まっている波があるあいだ、
 * 「作業状況」のラベルを accent にして件数のバッジを付ける。黒板タブを見ている人に
 * 編集パネルは見えないので、タブ自身が知らせる（タイトルバーの「コマンド承認」と同じ形）。
 * 件数は `lib/wavesBadge.ts` の 1 実装で、編集パネルの見出しと同じ数を出す。
 */
import { computed } from "vue";
import { useI18n } from "vue-i18n";

import { useOrchestrator } from "../composables/useOrchestrator";
import { pendingWaveCount, tabAttention } from "../lib/wavesBadge";
import type { BottomTab } from "../types";

defineProps<{ active: BottomTab }>();

const emit = defineEmits<{ (e: "select", tab: BottomTab): void }>();

const { t } = useI18n();
const { state } = useOrchestrator();

const pendingCount = computed(() => pendingWaveCount(state.planWaves));

/** 判定は `lib/wavesBadge.ts` の純関数（`waves` だけ・0 件では付けない）。 */
function attention(tab: { id: BottomTab }): boolean {
  return tabAttention(tab.id, pendingCount.value);
}

function titleOf(tab: { id: BottomTab; titleKey: string }): string {
  return attention(tab)
    ? t("bottomTabs.wavesPending", { count: pendingCount.value })
    : t(tab.titleKey);
}

/** 読み方の説明はラベルのホバーへ（サーヴァントの絆と同じ規則）。 */
const TABS: Array<{ id: BottomTab; labelKey: string; titleKey: string }> = [
  {
    id: "blackboard",
    labelKey: "bottomTabs.blackboard",
    titleKey: "bottomTabs.blackboardTitle",
  },
  {
    id: "waves",
    labelKey: "bottomTabs.waves",
    titleKey: "bottomTabs.wavesTitle",
  },
];
</script>

<template>
  <nav class="flex items-center gap-1" :aria-label="$t('bottomTabs.switcher')">
    <button
      v-for="tab in TABS"
      :key="tab.id"
      class="rounded px-2 py-0.5 text-[12px] tracking-wide"
      :class="[
        tab.id === active ? 'cursor-help bg-surface-2 font-semibold' : '',
        attention(tab)
          ? 'text-accent'
          : tab.id === active
            ? 'text-ink'
            : 'text-ink-dim hover:text-ink',
      ]"
      :title="titleOf(tab)"
      :data-pending="attention(tab) ? pendingCount : undefined"
      :aria-current="tab.id === active ? 'page' : undefined"
      @click="emit('select', tab.id)"
    >
      {{ $t(tab.labelKey) }}
      <!-- 承認待ちの件数。丸めない（タイトルバーの「コマンド承認」と同じ）。 -->
      <span
        v-if="attention(tab)"
        data-waves-pending
        class="ml-1 rounded-full bg-accent px-1.5 text-[10px] font-semibold text-surface-0"
      >{{ pendingCount }}</span>
    </button>
  </nav>
</template>
