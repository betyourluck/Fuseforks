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

/**
 * 通知の配色。
 *
 * 背景は**必ず不透明**にする。半透明の色を重ねただけの実装だと、
 * 背後のパネルの文字が透けて通知本文と混ざり、どちらも読めなくなる
 * （実際に右ペインの上で発生した）。色は枠線と文字だけで表す。
 */
const TONE = {
  error: "border-fail/70 text-fail",
  warn: "border-warn/70 text-warn",
  info: "border-line text-ink",
} as const;
</script>

<template>
  <!--
    右上ではなく上部中央に出す。右上には右ペインの編集ボタンがあり、
    通知がそれを覆うと、通知を消すまで設定を直せなくなる。
    中央ペイン上部（サーヴァントの絆の見出し）なら、覆っても操作を妨げない。
  -->
  <div
    class="pointer-events-none fixed top-3 left-1/2 z-50 w-[26rem] -translate-x-1/2 space-y-2"
  >
    <div
      v-for="toast in state.toasts"
      :key="toast.id"
      class="pointer-events-auto rounded-md border bg-surface-1 px-3 py-2 text-[12px] shadow-xl"
      :class="TONE[toast.level]"
    >
      <div class="flex items-start gap-2">
        <span class="flex-1 font-medium">{{ toast.title }}</span>
        <button
          class="opacity-60 hover:opacity-100"
          :title="$t('common.close')"
          @click="orchestrator.dismissToast(toast.id)"
        >
          ✕
        </button>
      </div>
      <p
        v-if="toast.detail"
        class="selectable mt-1 break-words whitespace-pre-line text-ink-dim"
      >
        {{ toast.detail }}
      </p>
    </div>
  </div>
</template>
