<script setup lang="ts">
/**
 * 中央ペイン下段: 村の黒板（共有作業メモの読み取り専用ビュー）。
 *
 * 実体はエージェントの共通 work_dir にある `黒板/` フォルダ。書くのは
 * エージェント（file ツール）と人で、**GUI からの書き込み経路は作らない**
 * （条例の「書いてよいのは自分の付箋だけ」を GUI が迂回しない）。
 *
 * 更新は pull のみ — 表示中の定期再読 + 手動更新。コアはファイル変更を
 * 監視せず、モデルへの push 注入もしない（黒板の運用と同じ形）。
 */
import { computed, onMounted, onUnmounted, ref } from "vue";

import { listBlackboard, toErrorPayload } from "../lib/ipc";
import { formatError } from "../lib/errorText";
import { renderMarkdown } from "../lib/markdown";
import type { BlackboardNote, BottomTab, ErrorPayload } from "../types";
import BottomPaneTabs from "./BottomPaneTabs.vue";

defineProps<{ activeTab: BottomTab }>();

const emit = defineEmits<{ (e: "selectTab", tab: BottomTab): void }>();

const notes = ref<BlackboardNote[]>([]);
const error = ref<ErrorPayload | null>(null);
/** 初回の読みが済むまで「空」と断定しない（一瞬の空表示のちらつき防止）。 */
const loaded = ref(false);

/** 表示中の自動再読の間隔。ローカルの一覧 + 読みだけなので軽い。 */
const REFRESH_MS = 10_000;
let timer: number | undefined;

async function refresh(): Promise<void> {
  try {
    notes.value = await listBlackboard();
    error.value = null;
  } catch (err) {
    error.value = toErrorPayload(err);
  } finally {
    loaded.value = true;
  }
}

onMounted(() => {
  void refresh();
  timer = window.setInterval(() => void refresh(), REFRESH_MS);
});
onUnmounted(() => window.clearInterval(timer));

/** 複数の work_dir が混在するときだけ由来を出す。通常は 1 つで無音。 */
const showDir = computed(() => new Set(notes.value.map((n) => n.dir)).size > 1);

function formatTime(ms: number): string {
  if (ms === 0) return "";
  const date = new Date(ms);
  const today = new Date();
  const sameDay =
    date.getFullYear() === today.getFullYear() &&
    date.getMonth() === today.getMonth() &&
    date.getDate() === today.getDate();
  return sameDay ? date.toLocaleTimeString() : date.toLocaleString();
}
</script>

<template>
  <div class="flex h-full flex-col">
    <!--
      高さは 4 ペイン共通の 38px 固定（AgentList のコメント参照）。
      下線は引かない — VS Code のパネル（問題 / 出力）と同じで、タブと中身の
      間に境界線を置かない（2026-08-02 利用者指定。下段タブの 2 ペイン共通）。
    -->
    <header
      class="flex h-[38px] shrink-0 items-center gap-3 px-3 text-xs text-ink-dim"
    >
      <BottomPaneTabs :active="activeTab" @select="emit('selectTab', $event)" />
      <span v-if="notes.length">{{ $t("blackboard.noteCount", { count: notes.length }) }}</span>
      <button
        class="ml-auto grid size-6 place-items-center rounded text-ink-dim transition-colors hover:text-ink focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-accent"
        :title="$t('blackboard.refreshTitle')"
        :aria-label="$t('blackboard.refresh')"
        @click="refresh"
      >
        <svg
          class="size-4"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M21 12a9 9 0 1 1-3-6.7" />
          <path d="M21 3v6h-6" />
        </svg>
      </button>
    </header>

    <div
      v-if="error"
      class="flex flex-1 items-center justify-center px-6 text-center text-xs text-fail"
    >
      {{ formatError(error) }}
    </div>

    <div
      v-else-if="loaded && notes.length === 0"
      class="flex flex-1 items-center justify-center px-6 text-center text-xs text-ink-dim"
    >
      {{ $t("blackboard.empty") }}
    </div>

    <!-- 付箋を縦に並べる。まとめ.md はコア側の並びで先頭に来る。 -->
    <div v-else class="min-h-0 flex-1 overflow-y-auto px-3 py-2">
      <article
        v-for="note in notes"
        :key="`${note.dir}:${note.name}`"
        class="mb-2 rounded-lg border border-line/50 bg-surface-1"
      >
        <header
          class="flex items-baseline gap-2 border-b border-line/50 px-3 py-1.5 text-[11px]"
        >
          <span class="font-semibold text-ink">{{ note.name }}</span>
          <span v-if="showDir" class="truncate text-ink-dim" :title="note.dir">{{
            note.dir
          }}</span>
          <span v-if="note.modifiedMs" class="ml-auto shrink-0 text-ink-dim">{{
            formatTime(note.modifiedMs)
          }}</span>
        </header>
        <!--
          renderMarkdown は html:false で生 HTML をエスケープするので、
          エージェントが書いた内容を v-html へ挿しても任意タグは注入されない
          （ChatPanel の会話バブルと同じ前提）。
        -->
        <div
          class="md-body selectable px-3 py-2 text-[12px] leading-relaxed wrap-anywhere text-ink"
          v-html="renderMarkdown(note.content)"
        />
      </article>
    </div>
  </div>
</template>
