<script setup lang="ts">
/**
 * 右ペイン: 選択中エージェントの設定とインラインエディタ。
 *
 * 既定は**読み取り専用のプレビュー**で、右上の ✏️ を押すと編集モードに入る。
 * この二段構えは、稼働中のエージェントの設定を誤クリックで書き換えないための保険。
 *
 * 編集モードでは下書きを別に持ち、保存するまでコアへ送らない。
 * 1 フィールド変更するたびに IPC を飛ばすと、途中の不整合な状態
 * （接続先を差し替える最中の空配列など）がコアの検査に引っかかる。
 */
import { computed, ref, watch } from "vue";
import { openPath } from "@tauri-apps/plugin-opener";

import MarkdownEditor from "./MarkdownEditor.vue";
import { useOrchestrator } from "../composables/useOrchestrator";
import { STATUS_LABELS, type AgentId, type AgentSpec } from "../types";

const orchestrator = useOrchestrator();
const { state } = orchestrator;

const editing = ref(false);
const draft = ref<AgentSpec | null>(null);

const selected = computed(
  () => state.agents.find((a) => a.id === state.selectedAgentId) ?? null,
);

/** 自分以外のエージェント。接続先の候補。 */
const others = computed(() =>
  state.agents.filter((a) => a.id !== selected.value?.id),
);

/** スナップショットから編集用の下書きを起こす。 */
function toDraft(): AgentSpec | null {
  const agent = selected.value;
  if (!agent) return null;
  return {
    id: agent.id,
    name: agent.name,
    modelTemplateId: agent.modelTemplateId,
    ragSources: [...agent.ragSources],
    connectedAgents: [...agent.connectedAgents],
    order: agent.order,
  };
}

// 選択が変わったら編集モードを抜ける。別のエージェントの下書きを
// 引きずったまま保存すると、意図しない相手の設定を上書きすることになる。
watch(
  () => state.selectedAgentId,
  () => {
    editing.value = false;
    draft.value = null;
  },
);

function beginEdit(): void {
  draft.value = toDraft();
  editing.value = true;
}

function cancelEdit(): void {
  editing.value = false;
  draft.value = null;
}

async function save(): Promise<void> {
  if (!draft.value) return;
  await orchestrator.updateAgent(draft.value);
  cancelEdit();
}

/** 接続先チェックボックスの切り替え。 */
function toggleConnection(targetId: AgentId, connected: boolean): void {
  if (!draft.value) return;
  draft.value.connectedAgents = connected
    ? [...draft.value.connectedAgents, targetId]
    : draft.value.connectedAgents.filter((id) => id !== targetId);
}

/** RAG ソースの切り替え。 */
function toggleRag(source: string, enabled: boolean): void {
  if (!draft.value) return;
  draft.value.ragSources = enabled
    ? [...draft.value.ragSources, source]
    : draft.value.ragSources.filter((s) => s !== source);
}

async function remove(): Promise<void> {
  const agent = selected.value;
  if (!agent) return;
  if (!confirm(`${agent.name} を削除します。設定ファイルも消えます。よろしいですか？`))
    return;
  await orchestrator.deleteAgent(agent.id);
}

/** 設定ファイルの置き場をエクスプローラで開く。 */
async function openFolder(): Promise<void> {
  const agent = selected.value;
  if (!agent || !state.workspace) return;
  await openPath(`${state.workspace}\\agents\\${agent.id}`);
}
</script>

<template>
  <div class="flex h-full flex-col">
    <header class="flex items-center gap-2 border-b border-line px-3 py-2.5 text-xs">
      <h2 class="flex-1 truncate font-semibold tracking-wide">
        {{ selected ? selected.name : "設定" }}
      </h2>

      <template v-if="selected">
        <button
          class="rounded border border-line px-1.5 py-0.5 hover:border-accent hover:text-accent"
          title="設定フォルダを開く"
          @click="openFolder"
        >
          📁
        </button>
        <button
          class="rounded border px-1.5 py-0.5"
          :class="
            editing
              ? 'border-accent text-accent'
              : 'border-line hover:border-accent hover:text-accent'
          "
          :title="editing ? '編集モードを終了' : '編集する'"
          @click="editing ? cancelEdit() : beginEdit()"
        >
          ✏️
        </button>
      </template>
    </header>

    <p v-if="!selected" class="p-6 text-center text-[11px] text-ink-dim">
      左の一覧からエージェントを選択してください。
    </p>

    <template v-else>
      <div class="max-h-[52%] shrink-0 overflow-y-auto border-b border-line p-3">
        <!-- 概況。編集モードでも変わらない実測値なので常に出す。 -->
        <dl class="grid grid-cols-[80px_1fr] gap-x-2 gap-y-1 text-[11px]">
          <dt class="text-ink-dim">ID</dt>
          <dd class="selectable font-mono">{{ selected.id }}</dd>
          <dt class="text-ink-dim">状態</dt>
          <dd>{{ STATUS_LABELS[selected.status] }}</dd>
          <dt class="text-ink-dim">トークン</dt>
          <dd class="tabular-nums">
            {{ selected.totalTokens.toLocaleString("ja-JP") }}
          </dd>
        </dl>

        <hr class="my-3 border-line" />

        <!-- 名前 -->
        <label class="mb-1 block text-[11px] text-ink-dim">名前</label>
        <input
          v-if="editing && draft"
          v-model="draft.name"
          class="mb-3 w-full rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent"
        />
        <p v-else class="mb-3">{{ selected.name }}</p>

        <!-- 使用モデル -->
        <label class="mb-1 block text-[11px] text-ink-dim">使用モデル</label>
        <select
          v-if="editing && draft"
          v-model="draft.modelTemplateId"
          class="mb-3 w-full rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent"
        >
          <option v-for="t in state.templates" :key="t.id" :value="t.id">
            {{ t.name }}（{{ t.model }}）
          </option>
        </select>
        <p v-else class="mb-3">{{ selected.model }}</p>

        <!-- 参照 RAG -->
        <label class="mb-1 block text-[11px] text-ink-dim">参照 RAG</label>
        <div v-if="editing && draft" class="mb-3 space-y-1">
          <label
            v-for="source in state.ragSources"
            :key="source"
            class="flex items-center gap-2"
          >
            <input
              type="checkbox"
              :checked="draft.ragSources.includes(source)"
              @change="
                toggleRag(source, ($event.target as HTMLInputElement).checked)
              "
            />
            <span>{{ source }}</span>
          </label>
          <p v-if="!state.ragSources.length" class="text-[11px] text-ink-dim">
            索引済みの RAG ソースがありません。
          </p>
        </div>
        <p v-else class="mb-3">
          {{ selected.ragSources.length ? selected.ragSources.join(", ") : "—" }}
        </p>

        <!-- 接続先 -->
        <label class="mb-1 block text-[11px] text-ink-dim">接続先エージェント</label>
        <div v-if="editing && draft" class="space-y-1">
          <label v-for="other in others" :key="other.id" class="flex items-center gap-2">
            <input
              type="checkbox"
              :checked="draft.connectedAgents.includes(other.id)"
              @change="
                toggleConnection(
                  other.id,
                  ($event.target as HTMLInputElement).checked,
                )
              "
            />
            <span>{{ other.name }}</span>
          </label>
          <p v-if="!others.length" class="text-[11px] text-ink-dim">
            他のエージェントがいません。
          </p>
        </div>
        <p v-else>
          {{
            selected.connectedAgents.length
              ? selected.connectedAgents
                  .map(
                    (id) => state.agents.find((a) => a.id === id)?.name ?? id,
                  )
                  .join(", ")
              : "—"
          }}
        </p>

        <div v-if="editing" class="mt-4 flex items-center gap-2">
          <button
            class="rounded border border-fail/60 px-2 py-1 text-[11px] text-fail hover:bg-fail/10"
            @click="remove"
          >
            削除
          </button>
          <button
            class="ml-auto rounded px-2 py-1 text-[11px] text-ink-dim hover:text-ink"
            @click="cancelEdit"
          >
            取消
          </button>
          <button
            class="rounded bg-accent px-3 py-1 text-[11px] font-medium text-surface-0"
            @click="save"
          >
            保存
          </button>
        </div>
      </div>

      <MarkdownEditor :agent-id="selected.id" :editable="editing" />
    </template>
  </div>
</template>
