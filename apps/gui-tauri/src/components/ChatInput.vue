<script setup lang="ts">
/**
 * 会話の入力欄。
 *
 * 作りは Kataribe の `ActionInput.vue` に倣う:
 * - `rows="1"` から始め、内容に合わせて**上方向へ伸びる**（下端固定のレイアウトなので）
 * - 上限まで伸びたら内部スクロールへ切り替える
 * - 送信ボタンは入力欄の**中**に浮かせ、中身があるときだけ現れる
 * - Enter で送信、Shift+Enter で改行
 *
 * 高さを CSS だけで賄えないのは、`textarea` に内容ぴったりへ縮む仕組みが無いため。
 * `height: auto` へ戻してから `scrollHeight` を読む、の 2 段が要る。
 */
import { computed, nextTick, ref } from "vue";

const props = defineProps<{
  /** 送信できない状態か。 */
  disabled: boolean;
  /** 入力欄のプレースホルダ。 */
  placeholder: string;
  /** 送信できないときの理由。ボタンの `title` に出す。 */
  blockedReason?: string;
}>();

const emit = defineEmits<{ (e: "send", text: string): void }>();

/** これを超えたら伸びるのをやめて内部スクロールにする。 */
const MAX_HEIGHT_PX = 220;

const text = ref("");
const area = ref<HTMLTextAreaElement | null>(null);

const canSend = computed(() => !!text.value.trim() && !props.disabled);

/** 内容ぴったりの高さへ合わせる。 */
function autoGrow(): void {
  const el = area.value;
  if (!el) return;
  // 一度 auto へ戻さないと scrollHeight が縮まず、削っても高さが残る。
  el.style.height = "auto";
  el.style.height = `${Math.min(el.scrollHeight, MAX_HEIGHT_PX)}px`;
}

async function send(): Promise<void> {
  if (!canSend.value) return;
  const payload = text.value;
  text.value = "";
  // 空にした後で高さを最小へ戻す。
  await nextTick();
  autoGrow();
  emit("send", payload);
}

/**
 * Enter の扱い。
 *
 * `event.isComposing` を見るのは日本語入力のため。IME で変換中の Enter は
 * 「変換の確定」であって送信ではない。ここを見ないと、変換を確定した瞬間に
 * 未完成の文が飛ぶ。
 */
function onEnter(event: KeyboardEvent): void {
  if (event.isComposing) return;
  event.preventDefault();
  void send();
}

/**
 * 外から文面を差し込む（Spec 12 — 分岐したときに、選んだ依頼を戻す）。
 *
 * **送信はしない。** 差した文面は書き換えられる状態で置くのが目的で、
 * 分岐の用途はそもそも「別の頼み方を試す」こと。カーソルは末尾へ置く。
 *
 * 下書きの所有権は入力欄に閉じたままにしたいので、`v-model` を親へ生やさず
 * この 1 メソッドだけを公開する。
 */
async function fill(payload: string): Promise<void> {
  text.value = payload;
  await nextTick();
  autoGrow();
  const el = area.value;
  if (!el) return;
  el.focus();
  el.setSelectionRange(payload.length, payload.length);
}

defineExpose({ fill });
</script>

<template>
    <div class="shrink-0 border-t border-line px-3 py-2.5">
    <div
      class="relative rounded-xl bg-surface-1 ring-1 ring-transparent transition focus-within:ring-accent/60"
      :class="{ 'opacity-40': disabled }"
    >
      <textarea
        ref="area"
        v-model="text"
        rows="1"
        :disabled="disabled"
        :placeholder="placeholder"
        class="selectable block w-full resize-none bg-transparent px-3 py-2.5 pr-12 text-[12px] leading-relaxed text-ink outline-none placeholder:text-ink-dim disabled:cursor-not-allowed"
        :style="{ maxHeight: `${MAX_HEIGHT_PX}px`, overflowY: 'auto' }"
        @input="autoGrow"
        @keydown.enter.exact="onEnter"
      />

      <!-- 送信。中身があるときだけ現れる。 -->
      <button
        v-show="text.trim()"
        type="button"
        :disabled="!canSend"
        aria-label="送信"
        :title="canSend ? '送信（Enter）' : (blockedReason ?? '送信できません')"
        class="absolute right-2 bottom-2 grid size-8 place-items-center rounded-lg bg-accent text-surface-0 transition hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
        @click="send"
      >
        <!-- ↵ -->
        <svg
          viewBox="0 0 24 24"
          class="size-4"
          fill="none"
          stroke="currentColor"
          stroke-width="2.2"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M9 10 4 15l5 5" />
          <path d="M20 4v7a4 4 0 0 1-4 4H4" />
        </svg>
      </button>
    </div>

    <p class="mt-1 px-1 text-[10px] text-ink-dim">
      Enter で送信 / Shift+Enter で改行
    </p>
  </div>
</template>
