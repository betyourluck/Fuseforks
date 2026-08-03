<script setup lang="ts">
/**
 * 中央下段ペインのタブ（黒板 | 作業状況）。
 *
 * タブの状態は App.vue が持ち、各ペイン（BlackboardPane / PlanWavePane）は
 * ヘッダにこれを差して select を親へ中継するだけ。ペイン自身がタブ状態を
 * 持たないのは、非表示側のペインは v-if で丸ごと居なくなるため。
 */
import type { BottomTab } from "../types";

defineProps<{ active: BottomTab }>();

const emit = defineEmits<{ (e: "select", tab: BottomTab): void }>();

/** 読み方の説明はラベルのホバーへ（村の地図と同じ規則）。 */
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
      :class="
        tab.id === active
          ? 'cursor-help bg-surface-2 font-semibold text-ink'
          : 'text-ink-dim hover:text-ink'
      "
      :title="$t(tab.titleKey)"
      :aria-current="tab.id === active ? 'page' : undefined"
      @click="emit('select', tab.id)"
    >
      {{ $t(tab.labelKey) }}
    </button>
  </nav>
</template>
