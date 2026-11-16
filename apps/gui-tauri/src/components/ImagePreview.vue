<script setup lang="ts">
/**
 * 会話の画像を大きく見る（Spec 76 P2 の実機の要望 —「生成した画像をクリックで大きくプレビューできるほうがいい」）。
 *
 * **`body` へ出して窓いっぱいに描く。** 会話ペインの根は container query のコンテナで、その中に置いた
 * `fixed` の層は会話ペインの枠に閉じ込められる（レイアウトの封じ込めが `fixed` の基準を作る）。
 * 会話の表示倍率も掛からない。
 *
 * 閉じるのは 背景のクリック / ✕ / Esc。Esc は**ここで止める** — 窓で聴いている他の Esc
 * （会話の検索・地図の接続モード）へ届かせない。開く前にフォーカスしていた要素へ戻す。
 */
import { onBeforeUnmount, onMounted, ref } from "vue";

defineProps<{
  /** 表示中の object URL（会話ペインの投影キャッシュのもの。ここでは作らないし捨てない）。 */
  src: string;
  /** 元ファイル名（生成画像の写しは作業フォルダのファイル名）。 */
  name: string;
  width?: number;
  height?: number;
}>();
const emit = defineEmits<{ close: [] }>();

const root = ref<HTMLElement | null>(null);
let previousFocus: HTMLElement | null = null;

onMounted(() => {
  previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  root.value?.focus();
});

onBeforeUnmount(() => {
  previousFocus?.focus();
});
</script>

<template>
  <Teleport to="body">
    <div
      ref="root"
      class="fixed inset-0 z-40 flex flex-col items-center justify-center gap-2 bg-scrim p-6 outline-none"
      role="dialog"
      aria-modal="true"
      :aria-label="name"
      tabindex="-1"
      data-image-preview
      @click.self="emit('close')"
      @keydown.esc.stop.prevent="emit('close')"
    >
      <button
        type="button"
        class="absolute right-3 top-3 rounded-full bg-surface-1 p-1.5 text-ink-dim ring-1 ring-line hover:text-ink"
        :aria-label="$t('common.close')"
        :title="$t('chat.imagePreviewClose')"
        @click="emit('close')"
      >
        <svg
          viewBox="0 0 24 24"
          class="size-4"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          aria-hidden="true"
        >
          <path d="M6 6l12 12M18 6 6 18" />
        </svg>
      </button>
      <img
        :src="src"
        :alt="name"
        class="max-h-[calc(100vh-5rem)] max-w-full rounded-lg object-contain shadow-2xl ring-1 ring-line"
      />
      <p class="max-w-full truncate text-xs text-ink-dim" :title="name">
        {{ name }}<template v-if="width && height"> · {{ width }}×{{ height }}</template>
      </p>
    </div>
  </Teleport>
</template>
