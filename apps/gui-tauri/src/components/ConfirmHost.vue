<script setup lang="ts">
/**
 * 確認ダイアログの実体。**画面に 1 つだけ置く**（App.vue）。
 *
 * 呼び出し側は {@link askConfirm} を `await` するだけで、この部品を知らない。
 * ブラウザの `confirm()` を置き換えるためのもの — 経緯は useConfirm.ts を見よ。
 */
import { nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";

import { useConfirmHost } from "../composables/useConfirm";

const { current, answer } = useConfirmHost();

const confirmButton = ref<HTMLButtonElement | null>(null);
const cancelButton = ref<HTMLButtonElement | null>(null);

/**
 * 開いたら片方のボタンへフォーカスを置く。
 *
 * **元に戻せない操作では取り消し側**へ置く。Enter はフォーカスされたボタンを
 * 押すので、初期位置が実行側だと「Enter を続けて押していたら消えていた」が起きる。
 * ダイアログ全体で Enter を実行へ束ねないのはこのため — フォーカスが唯一の
 * 「いま Enter で何が起きるか」の表示になる。
 */
watch(
  () => current.value?.id,
  async (id) => {
    if (id === undefined) return;
    await nextTick();
    const target = current.value?.danger ? cancelButton.value : confirmButton.value;
    target?.focus();
  },
  { immediate: true },
);

/**
 * Escape で取り消す。**window で拾う** — 覆いの上で拾う形にすると、
 * 背景をクリックした直後（フォーカスが body へ抜けた状態）で効かなくなる。
 */
function onKeydown(event: KeyboardEvent): void {
  if (event.key === "Escape" && current.value) {
    event.preventDefault();
    answer(false);
  }
}

onMounted(() => window.addEventListener("keydown", onKeydown));
onBeforeUnmount(() => window.removeEventListener("keydown", onKeydown));
</script>

<template>
  <!--
    z-[60] は既存のダイアログ（z-40）とトースト（z-50）より上。確認は
    **ダイアログの中から呼ばれる**（設定を閉じる・予定を消す）ので、
    下に潜ると押せないダイアログが出来上がる。
  -->
  <div
    v-if="current"
    class="fixed inset-0 z-[60] flex items-center justify-center bg-black/60"
    @click.self="answer(false)"
  >
    <div
      class="w-[26rem] rounded-lg border border-line bg-surface-1 p-4 shadow-2xl"
      role="alertdialog"
      aria-modal="true"
    >
      <h2 class="text-[13px] font-semibold text-ink">{{ current.title }}</h2>
      <p
        v-if="current.message"
        class="selectable mt-2 text-[12px] leading-relaxed whitespace-pre-line text-ink-dim"
      >
        {{ current.message }}
      </p>

      <div class="mt-4 flex justify-end gap-2 text-[12px]">
        <button
          ref="cancelButton"
          class="rounded border border-line px-3 py-1 text-ink-dim transition hover:border-accent hover:text-accent focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-accent"
          @click="answer(false)"
        >
          {{ current.cancelLabel ?? $t("confirm.no") }}
        </button>
        <button
          ref="confirmButton"
          class="rounded border px-3 py-1 transition focus-visible:outline-2 focus-visible:outline-offset-1"
          :class="
            current.danger
              ? 'border-fail/60 text-fail hover:bg-fail/10 focus-visible:outline-fail'
              : 'border-accent text-accent hover:bg-accent/10 focus-visible:outline-accent'
          "
          @click="answer(true)"
        >
          {{ current.confirmLabel ?? $t("confirm.yes") }}
        </button>
      </div>
    </div>
  </div>
</template>
