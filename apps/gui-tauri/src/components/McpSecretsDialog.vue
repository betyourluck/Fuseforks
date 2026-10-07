<script setup lang="ts">
/**
 * MCP の秘密の値（`${secret:NAME}`）を資格情報ストアへ入れるダイアログ（Spec 65 P4）。
 *
 * 入口は 2 つで、**どちらから開いても同じ一覧**を出す — 共通 MCP ダイアログの mcp.json の行と、
 * サーヴァントの編集ダイアログの mcp.json タブ（「AI で作成」と同じ位置）。一覧は
 * `list_mcp_secrets` が村の全個体を数えたもので、**共通の mcp.json と各サーヴァントの mcp.json の
 * 参照がまとめて並ぶ**（`bake` と起動前検査が数えるのと同じ範囲 = `village_mcp_secret_refs`）。
 * 同じ名前は複数のサーバーで 1 つの値を共有する。共通 MCP のダイアログの中に欄を置いていた頃、
 * 「共通の分しか拾わないのでは」と読まれた（2026-10-08 利用者）ので、置き場を分けて範囲を説明の帯に書いた。
 *
 * **値は画面へ戻さない** — 欄は保存済みかどうかだけを見せる（モデルの API キー・Jev のトークンと
 * 同じ扱い）。拾うのは**保存済み**の mcp.json からで、編集中の本文からは拾わない
 * （コアが接続に使うのと同じ範囲を見せる）。
 */
import { onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import * as ipc from "../lib/ipc";
import { askConfirm } from "../composables/useConfirm";
import { useOrchestrator } from "../composables/useOrchestrator";
import { formatError } from "../lib/errorText";
import type { McpSecretView } from "../types";

const emit = defineEmits<{
  (e: "close"): void;
  /** 値を保存した・消した（繋ぎ直し済み）。開いた側が接続状態を読み直すのに使う。 */
  (e: "changed"): void;
}>();

const { t } = useI18n();
const orchestrator = useOrchestrator();

/** `${secret:NAME}` の一覧。値は持たない。 */
const secrets = ref<McpSecretView[]>([]);
/** 名前ごとの入力欄。保存したら空に戻す（値をメモリに残しておかない）。 */
const inputs = ref<Record<string, string>>({});
const loading = ref(true);
const busy = ref(false);
const notice = ref("");
const loadError = ref("");

async function load(): Promise<void> {
  loading.value = true;
  try {
    secrets.value = await ipc.listMcpSecrets();
  } catch (err) {
    loadError.value = formatError(ipc.toErrorPayload(err));
  } finally {
    loading.value = false;
  }
}

onMounted(load);

/** 値を保存して繋ぎ直す。繋ぎ直さないと、保存した値を使う接続が次の再起動まで起きない。 */
async function save(name: string): Promise<void> {
  const value = (inputs.value[name] ?? "").trim();
  if (!value || busy.value) return;
  busy.value = true;
  notice.value = "";
  try {
    await ipc.setMcpSecret(name, value);
    inputs.value[name] = "";
    await orchestrator.reloadMcp();
    secrets.value = await ipc.listMcpSecrets();
    notice.value = t("mcp.secretSaved", { name });
    emit("changed");
  } catch (err) {
    notice.value = formatError(ipc.toErrorPayload(err));
  } finally {
    busy.value = false;
  }
}

async function clear(name: string): Promise<void> {
  if (busy.value) return;
  const ok = await askConfirm({
    title: t("mcp.secretClear"),
    message: t("mcp.secretClearConfirm", { name }),
    danger: true,
  });
  if (!ok) return;
  busy.value = true;
  notice.value = "";
  try {
    await ipc.clearMcpSecret(name);
    await orchestrator.reloadMcp();
    secrets.value = await ipc.listMcpSecrets();
    notice.value = t("mcp.secretCleared", { name });
    emit("changed");
  } catch (err) {
    notice.value = formatError(ipc.toErrorPayload(err));
  } finally {
    busy.value = false;
  }
}

/** 入力途中の値があれば確かめてから閉じる（貼った鍵を黙って捨てない）。 */
async function requestClose(): Promise<void> {
  const pending = Object.values(inputs.value).some((v) => v.trim() !== "");
  if (
    pending &&
    !(await askConfirm({
      title: t("mcp.secretsDiscardTitle"),
      message: t("mcp.secretsDiscardMessage"),
      confirmLabel: t("mcp.discardCloseConfirm"),
      cancelLabel: t("mcp.keepEditing"),
      danger: true,
    }))
  ) {
    return;
  }
  emit("close");
}
</script>

<template>
  <div
    class="fixed inset-0 z-50 flex items-center justify-center bg-scrim"
    data-mcp-secrets-dialog
    @click.self="requestClose"
  >
    <div
      class="flex max-h-[80vh] w-[640px] max-w-[96vw] flex-col overflow-hidden rounded-lg border border-line bg-surface-1 shadow-2xl"
    >
      <header class="flex shrink-0 items-center gap-2 border-b border-line px-3 py-2.5 text-xs">
        <svg
          class="size-3.5 shrink-0 text-accent"
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
        <h2 class="flex-1 font-semibold">{{ $t("mcp.secretsHeading") }}</h2>
        <button class="px-1 text-ink-dim hover:text-ink" @click="requestClose">✕</button>
      </header>

      <p class="shrink-0 border-b border-line bg-surface-0 px-3 py-2 text-[11px] text-ink-dim">
        {{ $t("mcp.secretsHint") }}
      </p>

      <div class="min-h-0 flex-1 overflow-y-auto p-3">
        <p v-if="loading" class="py-8 text-center text-[11px] text-ink-dim">{{ $t("mcp.loading") }}</p>
        <p v-else-if="loadError" class="py-8 text-center text-[11px] text-fail">{{ loadError }}</p>
        <p v-else-if="!secrets.length" class="text-[11px] text-ink-dim">{{ $t("mcp.secretsEmpty") }}</p>
        <ul v-else class="space-y-1.5">
          <li
            v-for="secret in secrets"
            :key="secret.name"
            class="rounded border border-line bg-surface-0 p-2 text-[11px]"
            data-mcp-secret
          >
            <div class="flex items-center gap-2">
              <span class="font-mono text-ink">{{ secret.name }}</span>
              <span :class="secret.stored ? 'text-run' : 'text-warn'">
                {{ secret.stored ? $t("mcp.secretStored") : $t("mcp.secretMissing") }}
              </span>
              <span class="ml-auto truncate text-ink-dim" :title="secret.servers.join(', ')">
                {{ $t("mcp.secretUsedBy", { servers: secret.servers.join(", ") }) }}
              </span>
            </div>
            <div class="mt-1.5 flex items-center gap-2">
              <input
                v-model="inputs[secret.name]"
                type="password"
                spellcheck="false"
                autocomplete="off"
                :placeholder="
                  secret.stored ? $t('mcp.secretPlaceholderStored') : $t('mcp.secretPlaceholder')
                "
                class="flex-1 rounded border border-line bg-surface-1 px-2 py-1 font-mono text-[11px] outline-none focus:border-accent"
                @keydown.enter="save(secret.name)"
              />
              <button
                type="button"
                class="rounded border border-line px-2 py-1 hover:border-accent hover:text-accent disabled:opacity-40"
                :disabled="!(inputs[secret.name] ?? '').trim() || busy"
                @click="save(secret.name)"
              >
                {{ $t("mcp.secretSave") }}
              </button>
              <button
                v-if="secret.stored"
                type="button"
                class="rounded border border-line px-2 py-1 text-warn hover:border-warn disabled:opacity-40"
                :disabled="busy"
                @click="clear(secret.name)"
              >
                {{ $t("mcp.secretClear") }}
              </button>
            </div>
          </li>
        </ul>
        <p v-if="notice" class="mt-2 text-[11px] text-ink-dim">{{ notice }}</p>
      </div>
    </div>
  </div>
</template>
