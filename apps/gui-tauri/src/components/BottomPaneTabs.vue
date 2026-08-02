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
const TABS: Array<{ id: BottomTab; label: string; title: string }> = [
  {
    id: "blackboard",
    label: "黒板",
    title: "村の黒板（共有作業メモ）。work_dir の 黒板/ を表示するだけで、ここからは書けない",
  },
  {
    id: "waves",
    label: "作業状況",
    title: "plan の実行痕。列 = 波 / 行 = サーヴァント",
  },
];
</script>

<template>
  <nav class="flex items-center gap-1" aria-label="下段ペインの切り替え">
    <button
      v-for="tab in TABS"
      :key="tab.id"
      class="rounded px-2 py-0.5 text-[12px] tracking-wide"
      :class="
        tab.id === active
          ? 'cursor-help bg-surface-2 font-semibold text-ink'
          : 'text-ink-dim hover:text-ink'
      "
      :title="tab.title"
      :aria-current="tab.id === active ? 'page' : undefined"
      @click="emit('select', tab.id)"
    >
      {{ tab.label }}
    </button>
  </nav>
</template>
