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
 *
 * 画像の添付（Spec 23 P4）。入口は**貼り付けと「参照…」の 2 つ**（D4 —
 * ドラッグ&ドロップは Tauri の `dragDropEnabled` が drop を横取りするので
 * 入れない）。変換（縮小 + WebP 化）は WebWorker で行い、この画面は
 * チップの表示と送信への同乗だけを持つ。上限は 1 発話 1 枚（D5）で、
 * 2 枚目を選んだら**置き換える**（チップが 1 枚しか出ない画面で「2 枚目は
 * 拒否」にすると、置き換えたつもりの操作が黙って無視される）。
 */
import { computed, nextTick, ref } from "vue";
import { useI18n } from "vue-i18n";

import {
  AttachmentError,
  MAX_EDGE_PX,
  convertImageFile,
  type PendingAttachment,
} from "../lib/attachment";
import { useOrchestrator } from "../composables/useOrchestrator";

const props = defineProps<{
  /** 送信できない状態か。 */
  disabled: boolean;
  /** 入力欄のプレースホルダ。 */
  placeholder: string;
  /** 送信できないときの理由。ボタンの `title` に出す。 */
  blockedReason?: string;
}>();

const emit = defineEmits<{
  (e: "send", text: string, attachments: PendingAttachment[]): void;
}>();

const { t } = useI18n();
const orchestrator = useOrchestrator();

/** これを超えたら伸びるのをやめて内部スクロールにする。 */
const MAX_HEIGHT_PX = 220;

const text = ref("");
const area = ref<HTMLTextAreaElement | null>(null);
const filePicker = ref<HTMLInputElement | null>(null);

/** 送信待ちの添付（D5: 1 枚まで）。 */
const attachment = ref<PendingAttachment | null>(null);
/** 変換中か。中は送信もチップの × も待たせる。 */
const converting = ref(false);

const canSend = computed(
  () =>
    (!!text.value.trim() || !!attachment.value) &&
    !props.disabled &&
    !converting.value,
);

/** チップに出すサイズ表記。 */
const attachmentSize = computed(() => {
  if (!attachment.value) return "";
  return `${Math.max(1, Math.round(attachment.value.bytes / 1024))} KB`;
});

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
  const files = attachment.value ? [attachment.value] : [];
  text.value = "";
  attachment.value = null;
  // 空にした後で高さを最小へ戻す。
  await nextTick();
  autoGrow();
  emit("send", payload, files);
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

/** 変換の失敗を辞書の文言で通知する。生の例外文字列は画面に出さない。 */
function notifyAttachError(error: unknown): void {
  const kind = error instanceof AttachmentError ? error.kind : "convertFailed";
  const key =
    kind === "tooLarge"
      ? "chatInput.attachTooLarge"
      : kind === "convertedTooLarge"
        ? "chatInput.attachConvertedTooLarge"
        : "chatInput.attachFailed";
  orchestrator.notify("error", t(key));
}

/** ファイルを 1 枚受け取り、変換してチップに載せる。 */
async function attach(file: File): Promise<void> {
  if (props.disabled || converting.value) return;
  converting.value = true;
  try {
    attachment.value = await convertImageFile(file);
  } catch (error) {
    notifyAttachError(error);
  } finally {
    converting.value = false;
  }
}

/**
 * 貼り付け。クリップボードに画像があるときだけ横取りする —
 * テキストの貼り付けは textarea の既定動作のまま。
 */
function onPaste(event: ClipboardEvent): void {
  const items = event.clipboardData?.items ?? [];
  for (const item of items) {
    if (item.kind === "file" && item.type.startsWith("image/")) {
      const file = item.getAsFile();
      if (file) {
        event.preventDefault();
        void attach(file);
      }
      return;
    }
  }
}

/** 「参照…」。同じファイルを選び直せるよう、読んだら value を戻す。 */
function onFilePicked(event: Event): void {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  input.value = "";
  if (file) void attach(file);
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
    <!-- 添付チップ（Spec 23）。× で外せる。S4 の注記をすぐ下に置く —
         「1 ターン限り」は設定ではなく仕様なので、添付のたびに見える場所で言う。 -->
    <div v-if="attachment || converting" class="mb-2 px-1">
      <div
        class="inline-flex max-w-full items-center gap-2 rounded-lg bg-surface-1 px-2.5 py-1.5 ring-1 ring-line"
      >
        <!-- 画像アイコン（SVG。絵文字は恒久要素に使わない）。 -->
        <svg
          viewBox="0 0 24 24"
          class="size-4 shrink-0 text-ink-dim"
          fill="none"
          stroke="currentColor"
          stroke-width="1.8"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <rect x="3" y="4" width="18" height="16" rx="2" />
          <circle cx="9" cy="10" r="1.6" />
          <path d="m3 17 5-5 4 4 3-3 6 6" />
        </svg>
        <template v-if="converting">
          <span class="text-[11px] text-ink-dim">{{ $t("chatInput.attachConverting") }}</span>
        </template>
        <template v-else-if="attachment">
          <span class="truncate text-[11px] text-ink" :title="attachment.fileName">
            {{ attachment.fileName }}
          </span>
          <span class="shrink-0 text-[10px] text-ink-dim tabular-nums">
            {{ attachment.width }}×{{ attachment.height }} / {{ attachmentSize }}
          </span>
          <button
            type="button"
            class="shrink-0 rounded px-0.5 text-[12px] leading-none text-ink-dim transition hover:text-warn"
            :title="$t('chatInput.attachRemove')"
            :aria-label="$t('chatInput.attachRemove')"
            @click="attachment = null"
          >
            ×
          </button>
        </template>
      </div>
      <p class="mt-1 text-[10px] text-ink-dim">
        {{ $t("chatInput.attachNote") }}
        <template v-if="attachment?.scaled">
          {{ $t("chatInput.attachScaled", { px: MAX_EDGE_PX }) }}
        </template>
      </p>
    </div>

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
        class="selectable block w-full resize-none bg-transparent py-2.5 pr-12 pl-11 text-[12px] leading-relaxed text-ink outline-none placeholder:text-ink-dim disabled:cursor-not-allowed"
        :style="{ maxHeight: `${MAX_HEIGHT_PX}px`, overflowY: 'auto' }"
        @input="autoGrow"
        @keydown.enter.exact="onEnter"
        @paste="onPaste"
      />

      <!-- 参照…（画像の添付）。送信ボタンと対称の位置。伸びる方向は下なので
           下端に留まる（送信ボタンの bottom-1 と同じ判断）。 -->
      <button
        type="button"
        :disabled="disabled || converting"
        :aria-label="$t('chatInput.attach')"
        :title="$t('chatInput.attach')"
        class="absolute bottom-1 left-1.5 grid size-8 place-items-center rounded-lg text-ink-dim transition hover:bg-surface-2 hover:text-ink disabled:cursor-not-allowed disabled:opacity-40"
        @click="filePicker?.click()"
      >
        <!-- クリップ -->
        <svg
          viewBox="0 0 24 24"
          class="size-4"
          fill="none"
          stroke="currentColor"
          stroke-width="1.8"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path
            d="m21.4 11.05-9.19 9.19a6 6 0 0 1-8.49-8.49l8.57-8.57A4 4 0 1 1 18 8.84l-8.59 8.57a2 2 0 0 1-2.83-2.83l8.49-8.48"
          />
        </svg>
      </button>
      <input
        ref="filePicker"
        type="file"
        accept="image/*"
        class="hidden"
        @change="onFilePicked"
      />

      <!-- 送信。中身（本文か添付）があるときだけ現れる。

           下の余白は `bottom-1`（4px）。1 行のときの入力欄は
           `py-2.5`（10px×2）+ 行の高さ 19.5px ≈ 40px で、ボタンは 32px なので
           上下に 4px ずつ残ると中央に来る。`bottom-2`（8px）だとボタンの上端が
           入力欄の上端に接し、**下だけ余って見えた**（実機の指摘）。

           複数行へ伸びたときは下端に寄る — 伸びる方向は下なので、
           **書いている行の隣にボタンが残る**（中央に置くと文章の途中を指す）。 -->
      <button
        v-show="text.trim() || attachment"
        type="button"
        :disabled="!canSend"
        :aria-label="$t('chatInput.send')"
        :title="canSend ? $t('chatInput.sendEnter') : (blockedReason ?? $t('chatInput.cannotSend'))"
        class="absolute right-2 bottom-1 grid size-8 place-items-center rounded-lg bg-accent text-surface-0 transition hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
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
      {{ $t("chatInput.hint") }}
    </p>
  </div>
</template>
