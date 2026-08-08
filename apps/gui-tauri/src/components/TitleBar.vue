<script setup lang="ts">
/**
 * カスタムタイトルバー（Kataribe / SomniumTextor の TitleBar.vue 同型)。
 * OS ネイティブ装飾（decorations:false）を使わず Vue 側で描画する。
 *
 * - ドラッグ移動は data-tauri-drag-region 属性で Tauri に委任（ボタンには付けない）。
 * - 最小化 / 最大化トグル / 閉じるは @tauri-apps/api/window を動的 import で叩く
 *   （ブラウザ環境 = Tauri 外でも crash しない）。
 */

import { computed } from "vue";

import { useOrchestrator } from "../composables/useOrchestrator";

const { state } = useOrchestrator();

const emit = defineEmits<{
  (e: "open-ordinance"): void;
  (e: "open-roles"): void;
  (e: "open-mcp"): void;
  (e: "open-command-approval"): void;
  (e: "open-schedules"): void;
  (e: "open-settings"): void;
}>();

/**
 * 判断待ちの**村の合計**（Spec 20 D3）。
 *
 * 個体ごとの最大値ではなく合計。承認画面が村全体を 1 画面で扱うので、
 * 入口の数も村全体の判断待ち件数を指すのが素直。**丸めない** — 溜まりすぎが
 * 数で見えること自体に意味がある。
 */
const pendingCount = computed(() =>
  state.commandRequests.reduce((sum, view) => sum + view.pending.length, 0),
);

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
      <span>Fuseforks</span>
    </div>

    <div data-tauri-drag-region class="h-full flex-1"></div>

    <!-- 村の条例。全エージェント共通の規則をここから編集する。 -->
    <button
      class="tb-btn"
      :title="$t('titleBar.ordinanceTitle')"
      :aria-label="$t('titleBar.ordinanceAria')"
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
      <span>{{ $t("titleBar.ordinance") }}</span>
    </button>

    <!--
      役職（Spec 14）。**条例の右**に置く — どちらも「この村がどういう村か」を
      決めるもので、システム設定（アプリがどう振る舞うか）とは棚が違う。
      初版はシステム設定の左メニューへ入れたが、それは `world.json` に住むこと
      （= 保存先）で分類した誤りで、実機で差し戻した（D1・2026-08-04）。
    -->
    <button
      class="tb-btn"
      :title="$t('titleBar.rolesTitle')"
      :aria-label="$t('titleBar.rolesAria')"
      @click="emit('open-roles')"
    >
      <!-- 名札（バッジ）。役職はサーヴァントに付く札そのもの。 -->
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
        <rect x="3" y="6" width="18" height="14" rx="2" />
        <path d="M9 3h6v3H9z" />
        <path d="M7 12h4M7 16h7" />
      </svg>
      <span>{{ $t("titleBar.roles") }}</span>
    </button>

    <!-- コマンド承認（Spec 20）。判断待ちの件数は**村の合計**を出す。
         0 件でも入口は消さない — 消えると「機能が無い」と読める。 -->
    <button
      class="tb-btn"
      :title="$t('titleBar.commandApprovalTitle')"
      :aria-label="$t('titleBar.commandApprovalAria')"
      @click="emit('open-command-approval')"
    >
      <!-- チェック付きの四角。「見て、決める」ものであることを形で示す。 -->
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
        <rect x="3" y="4" width="18" height="16" rx="2" />
        <path d="m8 12 3 3 5-6" />
      </svg>
      <span>{{ $t("titleBar.commandApproval") }}</span>
      <span
        v-if="pendingCount > 0"
        class="ml-1 rounded-full bg-accent px-1.5 text-[10px] font-semibold text-surface-0"
      >{{ pendingCount }}</span>
    </button>
    <!-- MCP サーバー。外部ツールの接続をここから管理する。 -->
    <button
      class="tb-btn"
      :title="$t('titleBar.mcpTitle')"
      :aria-label="$t('titleBar.mcpAria')"
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
      <span>{{ $t("titleBar.mcp") }}</span>
    </button>

    <!-- 予定。時刻で発火する依頼をここから管理する（Spec 07）。 -->
    <button
      class="tb-btn"
      :title="$t('titleBar.schedulesTitle')"
      :aria-label="$t('titleBar.schedulesAria')"
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
      <span>{{ $t("titleBar.schedules") }}</span>
    </button>

    <!-- システム設定。村の設定（天井など）をここから開く（Spec 13）。
         COG はカードの設定ボタンが鉛筆へ変わって空いた（rev3 D8）。 -->
    <button
      class="tb-btn"
      :title="$t('titleBar.settingsTitle')"
      :aria-label="$t('titleBar.settings')"
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
      <span>{{ $t("titleBar.settings") }}</span>
    </button>

    <div class="mx-1 h-4 w-px bg-line"></div>

    <!-- ウィンドウ操作 -->
    <button
      class="tb-btn"
      :title="$t('titleBar.minimize')"
      :aria-label="$t('titleBar.minimize')"
      @click="win('minimize')"
    >
      <svg width="11" height="11" viewBox="0 0 10 10">
        <line x1="0" y1="5" x2="10" y2="5" stroke="currentColor" stroke-width="1.2" />
      </svg>
    </button>
    <button
      class="tb-btn"
      :title="$t('titleBar.maximize')"
      :aria-label="$t('titleBar.maximize')"
      @click="win('toggleMaximize')"
    >
      <svg width="11" height="11" viewBox="0 0 10 10">
        <rect x="0.6" y="0.6" width="8.8" height="8.8" fill="none" stroke="currentColor" stroke-width="1.2" />
      </svg>
    </button>
    <button
      class="tb-btn tb-close"
      :title="$t('titleBar.close')"
      :aria-label="$t('titleBar.close')"
      @click="win('close')"
    >
      <svg width="11" height="11" viewBox="0 0 10 10">
        <line x1="0" y1="0" x2="10" y2="10" stroke="currentColor" stroke-width="1.2" />
        <line x1="10" y1="0" x2="0" y2="10" stroke="currentColor" stroke-width="1.2" />
      </svg>
    </button>
  </div>
</template>

<style scoped>
/*
 * ワードマーク。色も光も `style.css` のトークンから引く（テーマで変わる）。
 * ライトでは光をほぼ消す — 明るい地の発光は「光」ではなく「にじみ」に見える。
 */
.outcasts-word {
  color: var(--color-wordmark);
  font-family: "DotGothic16", sans-serif;
  text-shadow:
    0 0 6px var(--wordmark-glow-near),
    0 0 18px var(--wordmark-glow-far);
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
  color: var(--color-ink-dim);
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
  color: var(--color-ink);
}
.tb-close:hover {
  background: #e53935;
  color: #fff;
}
</style>
