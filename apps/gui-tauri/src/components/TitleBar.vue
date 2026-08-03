<script setup lang="ts">
/**
 * カスタムタイトルバー（Kataribe / SomniumTextor の TitleBar.vue 同型)。
 * OS ネイティブ装飾（decorations:false）を使わず Vue 側で描画する。
 *
 * - ドラッグ移動は data-tauri-drag-region 属性で Tauri に委任（ボタンには付けない）。
 * - 最小化 / 最大化トグル / 閉じるは @tauri-apps/api/window を動的 import で叩く
 *   （ブラウザ環境 = Tauri 外でも crash しない）。
 */

const emit = defineEmits<{
  (e: "open-ordinance"): void;
  (e: "open-mcp"): void;
  (e: "open-schedules"): void;
  (e: "open-settings"): void;
}>();

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
    class="flex h-[38px] shrink-0 select-none items-center border-b border-line bg-surface-1"
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
      <span class="outcasts-word">Outcasts</span>
      <span>Concordia</span>
    </div>

    <div data-tauri-drag-region class="h-full flex-1"></div>

    <!-- 村の条例。全エージェント共通の規則をここから編集する。 -->
    <button
      class="tb-btn"
      title="村の条例（全サーヴァント共通の規則）"
      aria-label="村の条例"
      @click="emit('open-ordinance')"
    >
      <!-- 巻物 -->
      <svg
        width="15"
        height="15"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="1.6"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
      >
        <path d="M8 21h9a3 3 0 0 0 3-3V5a2 2 0 0 0-2-2H8a3 3 0 0 0-3 3v12" />
        <path d="M5 21a2 2 0 0 1-2-2v-1h7" />
        <path d="M10 8h6M10 12h6" />
      </svg>
      <span>条例</span>
    </button>

    <!-- MCP サーバー。外部ツールの接続をここから管理する。 -->
    <button
      class="tb-btn"
      title="MCP サーバー（外部ツールの接続）"
      aria-label="MCP サーバー"
      @click="emit('open-mcp')"
    >
      <!-- プラグ -->
      <svg
        width="15"
        height="15"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="1.6"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
      >
        <path d="M9 2v6M15 2v6" />
        <path d="M6 8h12v3a6 6 0 0 1-12 0z" />
        <path d="M12 17v5" />
      </svg>
      <span>共通MCP</span>
    </button>

    <!-- 予定。時刻で発火する依頼をここから管理する（Spec 07）。 -->
    <button
      class="tb-btn"
      title="予定（時刻で発火する依頼）"
      aria-label="予定"
      @click="emit('open-schedules')"
    >
      <!-- カレンダー -->
      <svg
        width="15"
        height="15"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="1.6"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
      >
        <rect x="3" y="5" width="18" height="16" rx="2" />
        <path d="M16 3v4M8 3v4M3 10h18M8 14h.01M12 14h.01M16 14h.01M8 18h.01M12 18h.01" />
      </svg>
      <span>スケジュール</span>
    </button>

    <!-- システム設定。村の設定（天井など）をここから開く（Spec 13）。
         COG はカードの設定ボタンが鉛筆へ変わって空いた（rev3 D8）。 -->
    <button
      class="tb-btn"
      title="システム設定（村の設定）"
      aria-label="システム設定"
      @click="emit('open-settings')"
    >
      <!-- 歯車（COG） -->
      <svg
        width="15"
        height="15"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        stroke-width="1.6"
        stroke-linecap="round"
        stroke-linejoin="round"
        aria-hidden="true"
      >
        <circle cx="12" cy="12" r="3" />
        <path
          d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09a1.65 1.65 0 0 0-1-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09a1.65 1.65 0 0 0 1.51-1 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33h.01a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51h.01a1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82v.01a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"
        />
      </svg>
      <span>システム設定</span>
    </button>

    <div class="mx-1 h-4 w-px bg-line"></div>

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
.outcasts-word {
  color: rgb(var(--c-accent) / var(--tw-text-opacity, 1));
  font-family: "DotGothic16", sans-serif;
  text-shadow:
    0 0 6px rgba(168, 85, 247, 0.8),
    0 0 18px rgba(168, 85, 247, 0.4);
}

.tb-btn {
  min-width: 44px;
  height: 38px;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 0 10px;
  background: transparent;
  border: none;
  color: var(--color-ink-dim, #8b93a7);
  cursor: pointer;
  transition:
    background 0.15s,
    color 0.15s;
}
.tb-btn svg {
  flex: none;
}
.tb-btn span {
  font-size: 11px;
  white-space: nowrap;
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
