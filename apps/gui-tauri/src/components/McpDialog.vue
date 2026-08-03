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
 */
import { computed, onMounted, ref } from "vue";

import CodeEditor from "./CodeEditor.vue";
import * as ipc from "../lib/ipc";
import { askConfirm } from "../composables/useConfirm";
import { useOrchestrator } from "../composables/useOrchestrator";
import type { McpServerStatus } from "../types";

const emit = defineEmits<{ (e: "close"): void }>();

const orchestrator = useOrchestrator();

const text = ref("");
/** コアから読み出した最新の保存内容。dirty 判定の基準。 */
const saved = ref("");
const statuses = ref<McpServerStatus[]>([]);
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
    return error instanceof Error ? error.message : "JSON として読めません";
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
  } catch (error) {
    // 読めない状態で空のまま保存させると、既存の宣言を消しうる。
    loadError.value = `設定を読み込めませんでした: ${String(
      (error as { message?: string }).message ?? error,
    )}`;
  } finally {
    loading.value = false;
  }
}

onMounted(load);

async function save(): Promise<void> {
  if (parseError.value) return;
  busy.value = true;
  try {
    const ok = await orchestrator.saveMcpConfig(JSON.parse(text.value));
    if (ok) {
      saved.value = text.value;
      statuses.value = await ipc.listMcpServers();
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

async function requestClose(): Promise<void> {
  if (
    dirty.value &&
    !(await askConfirm({
      title: "未保存の変更を破棄して閉じますか？",
      message: "編集中の内容は保存されません。",
      confirmLabel: "破棄して閉じる",
      cancelLabel: "編集を続ける",
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
    class="fixed inset-0 z-40 flex items-center justify-center bg-black/60"
    @click.self="requestClose"
  >
    <div
      class="flex h-[640px] w-[760px] flex-col overflow-hidden rounded-lg border border-line bg-surface-1 shadow-2xl"
    >
      <header class="flex shrink-0 items-center gap-2 border-b border-line px-3 py-2.5 text-xs">
        <h2 class="flex-1 font-semibold">MCP サーバー</h2>
        <button
          class="rounded border border-line px-2 py-1 hover:border-accent hover:text-accent disabled:opacity-40"
          :disabled="busy || loading"
          title="設定を変えずに繋ぎ直す"
          @click="reconnect"
        >
          {{ busy ? "接続中…" : "再接続" }}
        </button>
        <button class="px-1 text-ink-dim hover:text-ink" @click="requestClose">✕</button>
      </header>

      <p class="shrink-0 border-b border-line bg-surface-0 px-3 py-2 text-[11px] text-ink-dim">
        外部ツールを <strong class="text-ink">MCP</strong>（Model Context Protocol）で繋ぎます。
        書式は <strong class="text-ink">Claude Desktop と同じ</strong>なので、既存の設定を
        <strong class="text-ink">加工せずそのまま</strong>貼れます（`npx` などのコマンドは
        PATH から解決します）。保存すると即座に接続し直し、各サーヴァントが次の発話から
        使えるようになります。
        <strong class="text-warn">API キーなどの秘密は書かないでください</strong> —
        このファイルは平文で保存されます。
      </p>

      <div class="min-h-0 flex-1 overflow-y-auto p-3">
        <p v-if="loading" class="py-8 text-center text-[11px] text-ink-dim">読み込み中…</p>
        <p v-else-if="loadError" class="py-8 text-center text-[11px] text-fail">
          {{ loadError }}
        </p>

        <template v-else>
          <div class="mb-1 flex items-center gap-2 text-[11px]">
            <span class="text-ink-dim">mcp.json</span>
            <button
              class="rounded border border-line px-1.5 py-0.5 text-ink-dim hover:border-accent hover:text-accent"
              title="記入例を貼る"
              @click="text = TEMPLATE"
            >
              記入例
            </button>
            <span v-if="parseError" class="ml-auto text-fail">JSON エラー: {{ parseError }}</span>
            <span v-else-if="dirty" class="ml-auto text-warn">未保存</span>
          </div>

          <CodeEditor
            v-model="text"
            class="h-64"
            language="json"
            placeholder="（未設定）右上の「記入例」から始められます"
          />

          <!-- 接続結果。繋がらなかった理由が見えないと利用者は直しようがない。 -->
          <h3 class="mt-4 mb-1 text-[11px] font-semibold text-ink-dim">接続状態</h3>
          <p v-if="!statuses.length" class="text-[11px] text-ink-dim">
            サーバーが登録されていません。
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
                  {{ status.connected ? `ツール ${status.tools.length} 本` : "未接続" }}
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
          保存すると接続し直します（アプリの再起動は不要）
        </span>
        <button
          class="ml-auto rounded bg-accent px-3 py-1 text-[11px] font-medium text-surface-0 disabled:opacity-40"
          :disabled="loading || !!loadError || !!parseError || busy || !dirty"
          @click="save"
        >
          {{ busy ? "保存中…" : "保存して接続" }}
        </button>
      </div>
    </div>
  </div>
</template>
