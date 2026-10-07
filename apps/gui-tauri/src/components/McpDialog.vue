<script setup lang="ts">
/**
 * MCP サーバーの管理ダイアログ。タイトルバーの 🔌 から開く。
 *
 * 設定は `{workspace}/mcp.json` に **Claude Desktop と同じ形**で保存される。
 * 既に持っている設定をそのまま貼れることに実用上の価値があるので、
 * フォームではなく JSON をそのまま編集させる。
 *
 * 保存すると即座に接続し直し、各サーバーの結果（繋がったか / 何のツールが
 * 見えたか / なぜ失敗したか）を下に出す。**繋がらなかった理由が見えないと、
 * 利用者は直しようがない。**
 *
 * `headers` の値に書いた `${secret:NAME}` は「秘密の値」の欄に出て、値を OS の
 * 資格情報ストアへ入れられる（Spec 65 P4）。**値は画面へ戻さない** — 欄は保存済みか
 * どうかだけを見せる（モデルの API キー・Jev のトークンと同じ扱い）。拾うのは
 * **保存済み**の mcp.json（共通と全個体）からで、編集中の本文からは拾わない
 * （コアが接続に使うのと同じ範囲を見せる）。
 */
import { computed, onMounted, ref } from "vue";
import { Translation as I18nT, useI18n } from "vue-i18n";

import CodeEditor from "./CodeEditor.vue";
import * as ipc from "../lib/ipc";
import { askConfirm } from "../composables/useConfirm";
import { useOrchestrator } from "../composables/useOrchestrator";
import { formatError } from "../lib/errorText";
import type { McpSecretView, McpServerStatus } from "../types";

const emit = defineEmits<{ (e: "close"): void }>();

const { t } = useI18n();
const orchestrator = useOrchestrator();

const text = ref("");
/** コアから読み出した最新の保存内容。dirty 判定の基準。 */
const saved = ref("");
const statuses = ref<McpServerStatus[]>([]);
/** `${secret:NAME}` の一覧。値は持たない。 */
const secrets = ref<McpSecretView[]>([]);
/** 名前ごとの入力欄。保存したら空に戻す（値をメモリに残しておかない）。 */
const secretInputs = ref<Record<string, string>>({});
const secretBusy = ref(false);
const secretNotice = ref("");
const loading = ref(true);
const busy = ref(false);
const loadError = ref("");
/** JSON として読めるか。読めないまま保存させない。 */
const parseError = computed(() => {
  if (!text.value.trim()) return "";
  try {
    JSON.parse(text.value);
    return "";
  } catch (error) {
    return error instanceof Error ? error.message : t("mcp.invalidJson");
  }
});

const dirty = computed(() => text.value !== saved.value);

/** 貼り付けの出発点。空のファイルに何を書けばいいか分からない状態を作らない。 */
const TEMPLATE = `{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "D:\\\\work"],
      "env": {},
      "enabled": true
    }
  }
}`;

async function load(): Promise<void> {
  loading.value = true;
  try {
    const config = await ipc.readMcpConfig();
    const rendered = JSON.stringify(config, null, 2);
    text.value = rendered;
    saved.value = rendered;
    statuses.value = await ipc.listMcpServers();
    secrets.value = await ipc.listMcpSecrets();
  } catch (error) {
    // 読めない状態で空のまま保存させると、既存の宣言を消しうる。
    loadError.value = t("mcp.loadError", {
      error: String((error as { message?: string }).message ?? error),
    });
  } finally {
    loading.value = false;
  }
}

onMounted(load);

/**
 * 保存できるか。**保存ボタンと `Ctrl+S` が同じ述語を見る。**
 *
 * 条件を 2 箇所に書くと、片方だけがすり抜ける形が生まれる（`Ctrl+S` だけが
 * 保存中に二重で走る / 壊れた JSON を通す）。ボタンの `:disabled` はこれの否定。
 */
const canSave = computed(
  () =>
    !loading.value && !loadError.value && !parseError.value && !busy.value && dirty.value,
);

/** `Ctrl+S`。押せないときは何もしない（ボタンが `disabled` のときと同じ）。 */
function saveFromEditor(): void {
  if (canSave.value) void save();
}

async function save(): Promise<void> {
  if (parseError.value) return;
  busy.value = true;
  try {
    const ok = await orchestrator.saveMcpConfig(JSON.parse(text.value));
    if (ok) {
      saved.value = text.value;
      statuses.value = await ipc.listMcpServers();
      secrets.value = await ipc.listMcpSecrets();
    }
  } finally {
    busy.value = false;
  }
}

/** 設定を変えずに繋ぎ直す。サーバーを直した後の再試行に使う。 */
async function reconnect(): Promise<void> {
  busy.value = true;
  try {
    await orchestrator.reloadMcp();
    statuses.value = await ipc.listMcpServers();
  } finally {
    busy.value = false;
  }
}

/** 値を保存して繋ぎ直す。繋ぎ直さないと、保存した値を使う接続が次の再起動まで起きない。 */
async function saveSecret(name: string): Promise<void> {
  const value = (secretInputs.value[name] ?? "").trim();
  if (!value || secretBusy.value) return;
  secretBusy.value = true;
  secretNotice.value = "";
  try {
    await ipc.setMcpSecret(name, value);
    secretInputs.value[name] = "";
    await orchestrator.reloadMcp();
    statuses.value = await ipc.listMcpServers();
    secrets.value = await ipc.listMcpSecrets();
    secretNotice.value = t("mcp.secretSaved", { name });
  } catch (err) {
    secretNotice.value = formatError(ipc.toErrorPayload(err));
  } finally {
    secretBusy.value = false;
  }
}

async function clearSecret(name: string): Promise<void> {
  if (secretBusy.value) return;
  const ok = await askConfirm({
    title: t("mcp.secretClear"),
    message: t("mcp.secretClearConfirm", { name }),
    danger: true,
  });
  if (!ok) return;
  secretBusy.value = true;
  secretNotice.value = "";
  try {
    await ipc.clearMcpSecret(name);
    await orchestrator.reloadMcp();
    statuses.value = await ipc.listMcpServers();
    secrets.value = await ipc.listMcpSecrets();
    secretNotice.value = t("mcp.secretCleared", { name });
  } catch (err) {
    secretNotice.value = formatError(ipc.toErrorPayload(err));
  } finally {
    secretBusy.value = false;
  }
}

async function requestClose(): Promise<void> {
  if (
    dirty.value &&
    !(await askConfirm({
      title: t("mcp.discardCloseTitle"),
      message: t("mcp.discardCloseMessage"),
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
    class="fixed inset-0 z-40 flex items-center justify-center bg-scrim"
    @click.self="requestClose"
  >
    <div
      class="flex h-[640px] w-[760px] flex-col overflow-hidden rounded-lg border border-line bg-surface-1 shadow-2xl"
    >
      <header class="flex shrink-0 items-center gap-2 border-b border-line px-3 py-2.5 text-xs">
        <h2 class="flex-1 font-semibold">{{ $t("mcp.title") }}</h2>
        <button
          class="rounded border border-line px-2 py-1 hover:border-accent hover:text-accent disabled:opacity-40"
          :disabled="busy || loading"
          :title="$t('mcp.reconnectTitle')"
          @click="reconnect"
        >
          {{ busy ? $t("mcp.connecting") : $t("mcp.reconnect") }}
        </button>
        <button class="px-1 text-ink-dim hover:text-ink" @click="requestClose">✕</button>
      </header>

      <I18nT
        keypath="mcp.description"
        tag="p"
        class="shrink-0 border-b border-line bg-surface-0 px-3 py-2 text-[11px] text-ink-dim"
        scope="global"
      >
        <template #mcp>
          <strong class="text-ink">MCP</strong>
        </template>
        <template #sameAs>
          <strong class="text-ink">{{ $t("mcp.descriptionSame") }}</strong>
        </template>
        <template #asIs>
          <strong class="text-ink">{{ $t("mcp.descriptionAsIs") }}</strong>
        </template>
        <template #secrets>
          <strong class="text-warn">{{ $t("mcp.descriptionSecrets") }}</strong>
        </template>
      </I18nT>

      <div class="min-h-0 flex-1 overflow-y-auto p-3">
        <p v-if="loading" class="py-8 text-center text-[11px] text-ink-dim">{{ $t("mcp.loading") }}</p>
        <p v-else-if="loadError" class="py-8 text-center text-[11px] text-fail">
          {{ loadError }}
        </p>

        <template v-else>
          <div class="mb-1 flex items-center gap-2 text-[11px]">
            <span class="text-ink-dim">mcp.json</span>
            <button
              class="rounded border border-line px-1.5 py-0.5 text-ink-dim hover:border-accent hover:text-accent"
              :title="$t('mcp.templateTitle')"
              @click="text = TEMPLATE"
            >
              {{ $t("mcp.template") }}
            </button>
            <span v-if="parseError" class="ml-auto text-fail">{{ $t("mcp.jsonError", { error: parseError }) }}</span>
            <span v-else-if="dirty" class="ml-auto text-warn">{{ $t("mcp.unsaved") }}</span>
          </div>

          <CodeEditor
            v-model="text"
            class="h-64"
            language="json"
            :placeholder="$t('mcp.editorPlaceholder')"
            @save="saveFromEditor"
          />

          <!-- 秘密の値（Spec 65 P4）。値は画面に戻さない。 -->
          <h3 class="mt-4 mb-1 text-[11px] font-semibold text-ink-dim">{{ $t("mcp.secretsHeading") }}</h3>
          <p class="mb-1.5 text-[11px] text-ink-dim">{{ $t("mcp.secretsHint") }}</p>
          <p v-if="!secrets.length" class="text-[11px] text-ink-dim">{{ $t("mcp.secretsEmpty") }}</p>
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
                  v-model="secretInputs[secret.name]"
                  type="password"
                  spellcheck="false"
                  autocomplete="off"
                  :placeholder="
                    secret.stored ? $t('mcp.secretPlaceholderStored') : $t('mcp.secretPlaceholder')
                  "
                  class="flex-1 rounded border border-line bg-surface-1 px-2 py-1 font-mono text-[11px] outline-none focus:border-accent"
                  @keydown.enter="saveSecret(secret.name)"
                />
                <button
                  type="button"
                  class="rounded border border-line px-2 py-1 hover:border-accent hover:text-accent disabled:opacity-40"
                  :disabled="!(secretInputs[secret.name] ?? '').trim() || secretBusy"
                  @click="saveSecret(secret.name)"
                >
                  {{ $t("mcp.secretSave") }}
                </button>
                <button
                  v-if="secret.stored"
                  type="button"
                  class="rounded border border-line px-2 py-1 text-warn hover:border-warn disabled:opacity-40"
                  :disabled="secretBusy"
                  @click="clearSecret(secret.name)"
                >
                  {{ $t("mcp.secretClear") }}
                </button>
              </div>
            </li>
          </ul>
          <p v-if="secretNotice" class="mt-1 text-[11px] text-ink-dim">{{ secretNotice }}</p>

          <!-- 接続結果。繋がらなかった理由が見えないと利用者は直しようがない。 -->
          <h3 class="mt-4 mb-1 text-[11px] font-semibold text-ink-dim">{{ $t("mcp.statusHeading") }}</h3>
          <p v-if="!statuses.length" class="text-[11px] text-ink-dim">
            {{ $t("mcp.noServers") }}
          </p>
          <ul v-else class="space-y-2">
            <li
              v-for="status in statuses"
              :key="status.name"
              class="rounded border border-line bg-surface-0 p-2 text-[11px]"
            >
              <div class="flex items-center gap-2">
                <span
                  class="size-2 shrink-0 rounded-full"
                  :class="status.connected ? 'bg-run' : 'bg-fail'"
                />
                <span class="font-medium text-ink">{{ status.name }}</span>
                <span class="text-ink-dim">
                  {{
                    status.connected
                      ? $t("mcp.toolCount", { count: status.tools.length })
                      : $t("mcp.notConnected")
                  }}
                </span>
              </div>
              <p v-if="status.error" class="selectable mt-1 pl-4 text-fail">
                {{ status.error }}
              </p>
              <p v-if="status.tools.length" class="mt-1 pl-4 font-mono text-ink-dim">
                {{ status.tools.join(", ") }}
              </p>
            </li>
          </ul>
        </template>
      </div>

      <div class="flex shrink-0 items-center gap-2 border-t border-line px-3 py-2.5">
        <span class="text-[11px] text-ink-dim">
          {{ $t("mcp.saveNote") }}
        </span>
        <button
          class="ml-auto rounded bg-accent px-3 py-1 text-[11px] font-medium text-surface-0 disabled:opacity-40"
          :disabled="!canSave"
          @click="save"
        >
          {{ busy ? $t("mcp.saving") : $t("mcp.saveAndConnect") }}
        </button>
      </div>
    </div>
  </div>
</template>
