<script setup lang="ts">
/**
 * 「秘密の値」の入口（鍵のアイコン + ラベル）。押すと [`McpSecretsDialog`] を開く。
 *
 * 置き場は 2 つ — 共通 MCP ダイアログの mcp.json の行と、サーヴァントの編集ダイアログの
 * mcp.json タブ（「AI で作成」と同じ位置）。**入口を 1 部品にしてあるのは、2 か所で見た目と
 * 開き方がずれないため。** アイコンは SVG（絵文字は字形がフォントに依存し、配色に追従しない）。
 */
import { ref } from "vue";

import McpSecretsDialog from "./McpSecretsDialog.vue";

// 根が 2 つ（ボタンとダイアログ）なので、親が渡す `class`（`ml-auto` など置き場の指定）はボタンへ付ける。
defineOptions({ inheritAttrs: false });

const emit = defineEmits<{
  /** ダイアログで値を保存した・消した。開いた側が接続状態を読み直すのに使う。 */
  (e: "changed"): void;
}>();

const open = ref(false);
</script>

<template>
  <button
    type="button"
    class="flex items-center gap-1 rounded border border-line px-2 py-0.5 text-[11px] text-ink-dim hover:border-accent hover:text-accent"
    :title="$t('mcp.secretsOpenTitle')"
    data-mcp-secrets-open
    v-bind="$attrs"
    @click="open = true"
  >
    <svg
      class="size-3 shrink-0"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <circle cx="7.5" cy="15.5" r="5.5" />
      <path d="m21 2-9.6 9.6" />
      <path d="m15.5 7.5 3 3L22 7l-3-3" />
    </svg>
    {{ $t("mcp.secretsHeading") }}
  </button>

  <McpSecretsDialog v-if="open" @close="open = false" @changed="emit('changed')" />
</template>
