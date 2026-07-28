<script setup lang="ts">
/**
 * 右上の通知スタック。
 *
 * 失敗通知は自動で消さない（{@link useOrchestrator} 側の方針）。
 * 数秒で消える失敗表示は、席を外している間に起きた問題を丸ごと隠す。
 */
import { useOrchestrator } from "../composables/useOrchestrator";

const orchestrator = useOrchestrator();
const { state } = orchestrator;

const TONE = {
  error: "border-fail/60 bg-fail/15 text-fail",
  warn: "border-warn/60 bg-warn/15 text-warn",
  info: "border-line bg-surface-2 text-ink",
} as const;
</script>

<template>
  <div class="pointer-events-none fixed top-3 right-3 z-50 w-80 space-y-2">
    <div
      v-for="toast in state.toasts"
      :key="toast.id"
      class="pointer-events-auto rounded-md border px-3 py-2 text-[12px] shadow-lg"
      :class="TONE[toast.level]"
    >
      <div class="flex items-start gap-2">
        <span class="flex-1 font-medium">{{ toast.title }}</span>
        <button
          class="opacity-60 hover:opacity-100"
          title="閉じる"
          @click="orchestrator.dismissToast(toast.id)"
        >
          ✕
        </button>
      </div>
      <p v-if="toast.detail" class="selectable mt-1 break-words opacity-90">
        {{ toast.detail }}
      </p>
    </div>
  </div>
</template>
