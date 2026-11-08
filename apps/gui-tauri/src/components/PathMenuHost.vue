<script setup lang="ts">
/**
 * 発話の中のファイルの位置のメニュー（2026-11-08）。`App.vue` に 1 つだけ置く。
 *
 * 項目は 2 つ — 「エクスプローラーで表示」（opener の `revealItemInDir`。選択した状態でフォルダを開くだけで
 * 実行しない）と「エディタで開く」（システム設定の外部エディタ。コアが棚から読んだコマンドを、シェルを
 * 通さずパスを 1 つの引数として起動する）。**OS の既定のアプリでは開かない** — 押す対象はモデルが書いた
 * 文章で、`.exe` / `.bat` なら実行される。
 *
 * **画面全体を覆う層を作らない**（覆う層は「/」や Alt+↑↓ の覆いの判定に掛かる）。外を押す・
 * Esc・スクロール・窓のリサイズで閉じる。
 */
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { usePathMenu } from "../composables/usePathMenu";
import { useEditorSettings } from "../composables/useEditorSettings";
import * as ipc from "../lib/ipc";

const { t } = useI18n();
const { state, close } = usePathMenu();
const editor = useEditorSettings();
const menu = ref<HTMLElement | null>(null);

/** 窓の端からはみ出さない位置。幅と高さは描いた後に測れないので、上限で見積もる。 */
const MENU_WIDTH = 280;
const MENU_HEIGHT = 120;
const position = computed(() => ({
  left: `${Math.max(4, Math.min(state.x, window.innerWidth - MENU_WIDTH - 4))}px`,
  top: `${Math.max(4, Math.min(state.y + 6, window.innerHeight - MENU_HEIGHT - 4))}px`,
}));

const busy = ref(false);

async function reveal(): Promise<void> {
  const resolved = state.resolved;
  if (!resolved || busy.value) return;
  busy.value = true;
  try {
    await revealItemInDir(resolved.path);
    close();
  } catch (err) {
    state.error = String(err);
  } finally {
    busy.value = false;
  }
}

async function openEditor(): Promise<void> {
  const resolved = state.resolved;
  if (!resolved || busy.value || !editor.configured.value) return;
  busy.value = true;
  try {
    await ipc.openInEditor(resolved.path, state.ref?.line ?? null);
    close();
  } catch (err) {
    state.error = ipc.toErrorPayload(err).message;
  } finally {
    busy.value = false;
  }
}

function onPointerDown(event: MouseEvent): void {
  if (!state.open) return;
  if (menu.value && event.target instanceof Node && menu.value.contains(event.target)) return;
  close();
}
function onKey(event: KeyboardEvent): void {
  if (state.open && event.key === "Escape") close();
}
function onScroll(event: Event): void {
  if (!state.open) return;
  if (menu.value && event.target instanceof Node && menu.value.contains(event.target)) return;
  close();
}

// 開くたびに設定を読み直す（システム設定で変えた直後でも、押せるかが正しい）。
watch(
  () => state.open,
  (open) => {
    if (open) void editor.reload();
  },
);

onMounted(() => {
  window.addEventListener("mousedown", onPointerDown, true);
  window.addEventListener("keydown", onKey);
  window.addEventListener("scroll", onScroll, true);
  window.addEventListener("resize", close);
});
onBeforeUnmount(() => {
  window.removeEventListener("mousedown", onPointerDown, true);
  window.removeEventListener("keydown", onKey);
  window.removeEventListener("scroll", onScroll, true);
  window.removeEventListener("resize", close);
});
</script>

<template>
  <div
    v-if="state.open"
    ref="menu"
    class="fixed z-50 w-[280px] rounded-md border border-line bg-surface-1 py-1 text-[11px] shadow-xl"
    :style="position"
    role="menu"
    data-path-menu
  >
    <p class="truncate px-3 pt-1 pb-1.5 font-mono text-ink-dim" :title="state.resolved?.path ?? state.raw">
      {{ state.resolved?.path ?? state.raw }}
    </p>
    <p v-if="state.resolved === undefined" class="px-3 py-1 text-ink-dim">
      {{ t("pathMenu.resolving") }}
    </p>
    <p v-else-if="state.resolved === null" class="px-3 py-1 text-warn">
      {{ t("pathMenu.notFound") }}
    </p>
    <template v-else>
      <button class="path-menu-item" role="menuitem" :disabled="busy" @click="reveal">
        {{ t("pathMenu.reveal") }}
      </button>
      <button
        class="path-menu-item"
        role="menuitem"
        :disabled="busy || !editor.configured.value"
        :title="editor.configured.value ? '' : t('pathMenu.editorUnset')"
        @click="openEditor"
      >
        {{ t("pathMenu.openEditor") }}
        <span v-if="state.ref?.line" class="text-ink-dim">:{{ state.ref.line }}</span>
      </button>
      <p v-if="!editor.configured.value" class="px-3 pt-0.5 pb-1 text-[10px] text-ink-dim">
        {{ t("pathMenu.editorUnset") }}
      </p>
    </template>
    <p v-if="state.error" class="px-3 py-1 text-fail wrap-anywhere">{{ state.error }}</p>
  </div>
</template>

<style scoped>
.path-menu-item {
  display: flex;
  width: 100%;
  align-items: center;
  gap: 4px;
  padding: 4px 12px;
  text-align: left;
  color: var(--color-ink);
}
.path-menu-item:hover:not(:disabled) {
  background-color: var(--color-surface-2);
  color: var(--color-accent);
}
.path-menu-item:disabled {
  opacity: 0.45;
  cursor: default;
}
</style>
