<script setup lang="ts">
/**
 * 区画単位のエラー境界。
 *
 * Vue は描画中に例外が出るとその部分木を破棄する。境界が無いと、
 * 1 つの区画の失敗がアプリ全体を白紙にし、再起動しないと戻らない。
 * 会話ログが消えて再起動が要る、という形で実際に発生した。
 *
 * ここで捕まえた区画だけを差し替え、**残りの画面は生かす**。
 * 例外は握り潰さず、理由を表示して再描画の導線を出す。
 */
import { onErrorCaptured, ref } from "vue";

const props = defineProps<{
  /** 表示名。どの区画が落ちたのかを人が判別するために使う。 */
  label: string;
}>();

const failure = ref<string | null>(null);
/** 再描画のたびに増やして `key` に使う。子を作り直すために必要。 */
const attempt = ref(0);

onErrorCaptured((error) => {
  failure.value = error instanceof Error ? error.message : String(error);
  // 開発中に原因を追えるよう、コンソールへは素の例外を残す。
  console.error(`[${props.label}] 描画に失敗しました`, error);
  // 親へ伝播させない。伝播すると結局アプリ全体が落ちる。
  return false;
});

function retry(): void {
  failure.value = null;
  attempt.value += 1;
}
</script>

<template>
  <div v-if="failure" class="flex h-full flex-col items-center justify-center gap-2 p-6 text-center">
    <p class="text-[12px] font-medium text-fail">
      {{ $t("errorBoundary.renderFailed", { label }) }}
    </p>
    <p class="selectable max-w-md text-[11px] break-words text-ink-dim">{{ failure }}</p>
    <button
      class="mt-1 rounded border border-line px-3 py-1 text-[11px] hover:border-accent hover:text-accent"
      @click="retry"
    >
      {{ $t("errorBoundary.retry") }}
    </button>
  </div>
  <slot v-else :key="attempt" />
</template>
