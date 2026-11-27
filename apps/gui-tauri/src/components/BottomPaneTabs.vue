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
 *
 * **ペインが狭いときはアイコンだけ**（2026-11-27 利用者要望）。ラベルが 1 文字ずつ
 * 折り返して見出し行が 2 行になっていた。切り替えは見出し行の幅の container query で、
 * 閾値と出し分けは `style.css` の `.bottom-pane-header` の 1 箇所（黒板と作業状況の
 * 見出しが同じ閾値で動く）。**バッジはラベルの外に置く** — アイコンだけの状態でも
 * 承認待ちの印が消えない。ラベルの文字は `title` と `aria-label` に残す。
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

/**
 * ホバーの文言。1 行目はタブの名前 — アイコンだけの状態では、名前がここにしか無い。
 * 2 行目が読み方の説明（承認待ちのあいだはその件数）。
 */
function titleOf(tab: { id: BottomTab; labelKey: string; titleKey: string }): string {
  const detail = attention(tab)
    ? t("bottomTabs.wavesPending", { count: pendingCount.value })
    : t(tab.titleKey);
  return `${t(tab.labelKey)}\n${detail}`;
}

/**
 * 読み方の説明はラベルのホバーへ（サーヴァントの絆と同じ規則）。
 * アイコンは 24 の viewBox に線で描く（色は `currentColor` = ラベルと同じ色）。
 * 黒板 = 脚の付いた板にチョークの 1 本線 / 作業状況 = 列（波）と行（サーヴァント）の格子。
 */
const TABS: Array<{ id: BottomTab; labelKey: string; titleKey: string; icon: string[] }> = [
  {
    id: "blackboard",
    labelKey: "bottomTabs.blackboard",
    titleKey: "bottomTabs.blackboardTitle",
    icon: [
      "M4 4h16a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1z",
      "M7 9h6",
      "m8 20 2-5",
      "m16 20-2-5",
    ],
  },
  {
    id: "waves",
    labelKey: "bottomTabs.waves",
    titleKey: "bottomTabs.wavesTitle",
    icon: [
      "M5 4h14a1 1 0 0 1 1 1v14a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1z",
      "M4 10h16",
      "M4 15h16",
      "M10 4v16",
      "M15 4v16",
    ],
  },
];
</script>

<template>
  <nav class="flex shrink-0 items-center gap-1" :aria-label="$t('bottomTabs.switcher')">
    <button
      v-for="tab in TABS"
      :key="tab.id"
      class="inline-flex h-5 items-center rounded px-2 text-[12px] tracking-wide"
      :class="[
        tab.id === active ? 'cursor-help bg-surface-2 font-semibold' : '',
        attention(tab)
          ? 'text-accent'
          : tab.id === active
            ? 'text-ink'
            : 'text-ink-dim hover:text-ink',
      ]"
      :title="titleOf(tab)"
      :aria-label="titleOf(tab)"
      :data-pending="attention(tab) ? pendingCount : undefined"
      :aria-current="tab.id === active ? 'page' : undefined"
      @click="emit('select', tab.id)"
    >
      <!-- 狭いときだけ出る（`style.css` の `.pane-head-narrow`）。 -->
      <svg
        class="pane-head-narrow size-3.5"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
      >
        <path v-for="d in tab.icon" :key="d" :d="d" />
      </svg>
      <span class="pane-head-wide">{{ $t(tab.labelKey) }}</span>
      <!-- 承認待ちの件数。丸めない（タイトルバーの「コマンド承認」と同じ）。ラベルの外なので幅に依らず出る。 -->
      <span
        v-if="attention(tab)"
        data-waves-pending
        class="ml-1 rounded-full bg-accent px-1.5 text-[10px] font-semibold text-surface-0"
      >{{ pendingCount }}</span>
    </button>
  </nav>
</template>
