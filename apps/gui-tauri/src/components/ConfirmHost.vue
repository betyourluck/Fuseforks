<script setup lang="ts">
/**
 * 確認ダイアログの実体。**画面に 1 つだけ置く**（App.vue）。選択ダイアログ（`askChoice`）も
 * 同じ 1 つで出す — 実行ボタンの代わりに選択肢を並べるだけ。
 *
 * 呼び出し側は {@link askConfirm} を `await` するだけで、この部品を知らない。
 * ブラウザの `confirm()` を置き換えるためのもの — 経緯は useConfirm.ts を見よ。
 */
import { nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";

import { useConfirmHost } from "../composables/useConfirm";

const { current, answer, choose } = useConfirmHost();

const confirmButton = ref<HTMLButtonElement | null>(null);
const cancelButton = ref<HTMLButtonElement | null>(null);
/**
 * ダイアログの枠。選択肢の最初のボタンはここから引く — `v-for` の `ref` 配列は並びを保証しない
 * （Vue の文書）ので、「最初」を取るのに使えない。
 */
const dialog = ref<HTMLElement | null>(null);

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
    // 選択ダイアログは、選択肢に元に戻せない操作が 1 つでもあれば `danger` が立つ（askChoice）。
    const target = current.value?.danger
      ? cancelButton.value
      : (dialog.value?.querySelector<HTMLButtonElement>("[data-confirm-choice]") ?? confirmButton.value);
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
    class="fixed inset-0 z-[60] flex items-center justify-center bg-scrim"
    @click.self="answer(false)"
  >
    <div
      ref="dialog"
      class="rounded-lg border border-line bg-surface-1 p-4 shadow-2xl"
      :class="current.choices ? 'w-[30rem]' : 'w-[26rem]'"
      role="alertdialog"
      aria-modal="true"
    >
      <h2 class="text-[13px] font-semibold text-ink">{{ current.title }}</h2>
      <!-- 見出しの下の絵（askChoice の lead）。あれば文の代わりに出す。 -->
      <div v-if="current.lead" class="mt-2">
        <component :is="current.lead.component" v-bind="current.lead.props" />
      </div>
      <p
        v-else-if="current.message"
        class="selectable mt-2 text-[12px] leading-relaxed whitespace-pre-line text-ink-dim"
      >
        {{ current.message }}
      </p>

      <!--
        選択ダイアログ（askChoice）。実行ボタンの代わりに、選択肢を 1 行ずつの札で並べる —
        左に絵、右に見出しと補足。色はトークンだけ（元に戻せない選択肢は fail、最も影響の大きい
        1 つは地を薄く塗る）。取り消しは下の段の、確認ダイアログと同じボタン。
      -->
      <div v-if="current.choices" class="mt-3 flex flex-col gap-2" role="group">
        <button
          v-for="choice in current.choices"
          :key="choice.key"
          data-confirm-choice
          class="confirm-choice flex w-full items-stretch overflow-hidden rounded border text-left transition focus-visible:outline-2 focus-visible:outline-offset-1"
          :class="[
            choice.danger
              ? 'is-danger border-fail/40 hover:border-fail/80 focus-visible:outline-fail'
              : 'border-line hover:border-accent focus-visible:outline-accent',
            choice.emphasis ? 'bg-fail/10' : 'bg-surface-0',
          ]"
          @click="choose(choice.key)"
        >
          <span
            v-if="choice.visual"
            class="flex w-16 shrink-0 items-center justify-center border-r border-line"
          >
            <component :is="choice.visual.component" v-bind="choice.visual.props" />
          </span>
          <span class="flex min-w-0 flex-col justify-center gap-0.5 px-3 py-2">
            <span class="text-[12px] font-semibold" :class="choice.danger ? 'text-fail' : 'text-ink'">
              {{ choice.label }}
            </span>
            <span v-if="choice.description" class="text-[11px] text-ink-dim">
              {{ choice.description }}
            </span>
          </span>
        </button>
      </div>

      <div class="mt-4 flex justify-end gap-2 text-[12px]">
        <button
          ref="cancelButton"
          class="rounded border border-line px-3 py-1 text-ink-dim transition hover:border-accent hover:text-accent focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-accent"
          @click="answer(false)"
        >
          {{ current.cancelLabel ?? (current.choices ? $t("confirm.cancel") : $t("confirm.no")) }}
        </button>
        <button
          v-if="!current.choices"
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

<style scoped>
/*
 * 元に戻せない選択肢は、触れている間だけ fail の光をまとう（絆の地図の HUD が accent の光を
 * まとうのと同じ作法）。色はトークンから引く — 生の色を書かない。
 */
.confirm-choice.is-danger:hover,
.confirm-choice.is-danger:focus-visible {
  box-shadow: 0 0 12px color-mix(in oklab, var(--color-fail) 28%, transparent);
}
</style>
