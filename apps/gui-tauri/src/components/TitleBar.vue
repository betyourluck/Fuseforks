<script setup lang="ts">
/**
 * カスタムタイトルバー（Kataribe / SomniumTextor の TitleBar.vue 同型)。
 * OS ネイティブ装飾（decorations:false）を使わず Vue 側で描画する。
 *
 * - ドラッグ移動は data-tauri-drag-region 属性で Tauri に委任（ボタンには付けない）。
 * - 最小化 / 最大化トグル / 閉じるは @tauri-apps/api/window を動的 import で叩く
 *   （ブラウザ環境 = Tauri 外でも crash しない）。
 */

async function win(method: "minimize" | "toggleMaximize" | "close") {
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow()[method]();
  } catch (e) {
    console.warn(`[TitleBar] window.${method} unavailable:`, e);
  }
}
</script>

<template>
  <div
    data-tauri-drag-region
    class="flex h-8 shrink-0 select-none items-center border-b border-line bg-surface-1"
  >
    <div
      data-tauri-drag-region
      class="pointer-events-none flex items-center gap-2 px-3 text-xs font-bold tracking-widest text-ink"
    >
      <!-- ブランドマーク: 3 つのノードが繋がる図。オーケストレーションの形をそのまま出す。 -->
      <svg
        width="14"
        height="14"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="1.8"
        class="text-accent"
        aria-hidden="true"
      >
        <circle cx="12" cy="5" r="2.4" />
        <circle cx="5" cy="18" r="2.4" />
        <circle cx="19" cy="18" r="2.4" />
        <path d="M10.6 7 6.4 16M13.4 7l4.2 9M7.4 18h9.2" />
      </svg>
      Concordia
    </div>

    <div data-tauri-drag-region class="h-full flex-1"></div>

    <!-- ウィンドウ操作 -->
    <button class="tb-btn" title="最小化" aria-label="最小化" @click="win('minimize')">
      <svg width="11" height="11" viewBox="0 0 10 10">
        <line x1="0" y1="5" x2="10" y2="5" stroke="currentColor" stroke-width="1.2" />
      </svg>
    </button>
    <button
      class="tb-btn"
      title="最大化 / 元に戻す"
      aria-label="最大化 / 元に戻す"
      @click="win('toggleMaximize')"
    >
      <svg width="11" height="11" viewBox="0 0 10 10">
        <rect x="0.6" y="0.6" width="8.8" height="8.8" fill="none" stroke="currentColor" stroke-width="1.2" />
      </svg>
    </button>
    <button class="tb-btn tb-close" title="閉じる" aria-label="閉じる" @click="win('close')">
      <svg width="11" height="11" viewBox="0 0 10 10">
        <line x1="0" y1="0" x2="10" y2="10" stroke="currentColor" stroke-width="1.2" />
        <line x1="10" y1="0" x2="0" y2="10" stroke="currentColor" stroke-width="1.2" />
      </svg>
    </button>
  </div>
</template>

<style scoped>
.tb-btn {
  width: 44px;
  height: 32px;
  display: flex;
  align-items: center;
  justify-content: center;
  background: transparent;
  border: none;
  color: var(--color-ink-dim, #8b93a7);
  cursor: pointer;
  transition:
    background 0.15s,
    color 0.15s;
}
.tb-btn:hover {
  background: color-mix(in oklab, currentColor 12%, transparent);
  color: var(--color-ink, #e6e9f2);
}
.tb-close:hover {
  background: #e53935;
  color: #fff;
}
</style>
