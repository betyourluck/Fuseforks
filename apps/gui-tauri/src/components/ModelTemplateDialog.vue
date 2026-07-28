<script setup lang="ts">
/**
 * モデルテンプレートの管理ダイアログ。
 *
 * API キーの入力欄が無いのは意図的。テンプレートに持たせるのは
 * **環境変数名だけ**で、実値はプロセスの環境から解決する。
 * 平文で保存される設定ファイルに秘密を書ける場所を、UI からも作らない。
 */
import { computed, ref } from "vue";

import { useOrchestrator } from "../composables/useOrchestrator";
import type { Effort, ModelTemplate, ModelTemplateId, Provider } from "../types";

const emit = defineEmits<{ (e: "close"): void }>();

const orchestrator = useOrchestrator();
const { state } = orchestrator;

const selectedId = ref<ModelTemplateId | null>(state.templates[0]?.id ?? null);
const draft = ref<ModelTemplate | null>(null);

const PROVIDERS: { value: Provider | null; label: string }[] = [
  { value: null, label: "自動判定（baseUrl から）" },
  { value: "open_ai_compat", label: "OpenAI 互換" },
  { value: "anthropic", label: "Anthropic ネイティブ" },
];

const EFFORTS: { value: Effort | null; label: string }[] = [
  { value: null, label: "指定しない（送らない）" },
  { value: "low", label: "low" },
  { value: "medium", label: "medium" },
  { value: "high", label: "high" },
  { value: "xhigh", label: "xhigh" },
  { value: "max", label: "max" },
];

const current = computed(
  () => state.templates.find((t) => t.id === selectedId.value) ?? null,
);

/** 新規テンプレートの初期値。Rust 側の `ModelTemplate::new` と揃えてある。 */
function blank(): ModelTemplate {
  const base = "template";
  let id = base;
  let n = 2;
  while (state.templates.some((t) => t.id === id)) {
    id = `${base}_${n}`;
    n += 1;
  }
  return {
    id,
    name: "新しいテンプレート",
    baseUrl: "https://api.openai.com/v1",
    model: "gpt-4o",
    contextLength: 128000,
    temperature: null,
    maxOutputTokens: 4096,
    apiKeyEnv: "OPENAI_API_KEY",
    provider: null,
    useTools: true,
    effort: null,
    requestTimeoutSecs: 120,
    maxRetries: 3,
  };
}

function edit(template: ModelTemplate): void {
  selectedId.value = template.id;
  draft.value = { ...template };
}

function create(): void {
  draft.value = blank();
  selectedId.value = null;
}

async function save(): Promise<void> {
  if (!draft.value) return;
  await orchestrator.upsertTemplate(draft.value);
  selectedId.value = draft.value.id;
  draft.value = null;
}

async function remove(template: ModelTemplate): Promise<void> {
  if (!confirm(`${template.name} を削除しますか？`)) return;
  await orchestrator.deleteTemplate(template.id);
  if (selectedId.value === template.id) selectedId.value = null;
}

/** `temperature` の入力を「空文字 = 送らない」として扱う。 */
function onTemperature(raw: string): void {
  if (!draft.value) return;
  draft.value.temperature = raw.trim() === "" ? null : Number(raw);
}
</script>

<template>
  <div
    class="fixed inset-0 z-40 flex items-center justify-center bg-black/60"
    @click.self="emit('close')"
  >
    <div
      class="flex h-[560px] w-[760px] overflow-hidden rounded-lg border border-line bg-surface-1 shadow-2xl"
    >
      <!-- 一覧 -->
      <div class="flex w-56 shrink-0 flex-col border-r border-line">
        <header class="flex items-center border-b border-line px-3 py-2 text-xs">
          <h2 class="flex-1 font-semibold">モデルテンプレート</h2>
          <button
            class="rounded border border-line px-1.5 hover:border-accent hover:text-accent"
            title="追加"
            @click="create"
          >
            ＋
          </button>
        </header>

        <ul class="flex-1 overflow-y-auto p-2">
          <li v-for="template in state.templates" :key="template.id">
            <button
              class="group flex w-full items-center gap-1 rounded px-2 py-1.5 text-left text-[12px]"
              :class="
                selectedId === template.id ? 'bg-surface-2' : 'hover:bg-surface-2'
              "
              @click="edit(template)"
            >
              <span class="min-w-0 flex-1">
                <span class="block truncate">{{ template.name }}</span>
                <span class="block truncate text-[10px] text-ink-dim">
                  {{ template.model }}
                </span>
              </span>
              <span
                class="hidden px-1 text-fail group-hover:inline"
                title="削除"
                @click.stop="remove(template)"
              >
                ✕
              </span>
            </button>
          </li>
          <li
            v-if="!state.templates.length"
            class="px-2 py-6 text-center text-[11px] text-ink-dim"
          >
            未登録です。
          </li>
        </ul>
      </div>

      <!-- 編集フォーム -->
      <div class="flex min-w-0 flex-1 flex-col">
        <header class="flex items-center border-b border-line px-3 py-2 text-xs">
          <h3 class="flex-1 truncate font-semibold">
            {{ draft ? draft.name : (current?.name ?? "選択してください") }}
          </h3>
          <button class="px-1 text-ink-dim hover:text-ink" @click="emit('close')">
            ✕
          </button>
        </header>

        <div v-if="draft" class="flex-1 space-y-3 overflow-y-auto p-4 text-[12px]">
          <div class="grid grid-cols-[128px_1fr] items-center gap-x-3 gap-y-2.5">
            <label class="text-ink-dim">識別子</label>
            <input
              v-model="draft.id"
              class="rounded border border-line bg-surface-0 px-2 py-1 font-mono outline-none focus:border-accent"
            />

            <label class="text-ink-dim">表示名</label>
            <input
              v-model="draft.name"
              class="rounded border border-line bg-surface-0 px-2 py-1 outline-none focus:border-accent"
            />

            <label class="text-ink-dim">base URL</label>
            <input
              v-model="draft.baseUrl"
              placeholder="https://api.openai.com/v1"
              class="rounded border border-line bg-surface-0 px-2 py-1 font-mono outline-none focus:border-accent"
            />

            <label class="text-ink-dim">モデル名</label>
            <input
              v-model="draft.model"
              class="rounded border border-line bg-surface-0 px-2 py-1 font-mono outline-none focus:border-accent"
            />

            <label class="text-ink-dim">プロトコル</label>
            <select
              v-model="draft.provider"
              class="rounded border border-line bg-surface-0 px-2 py-1 outline-none focus:border-accent"
            >
              <option v-for="p in PROVIDERS" :key="String(p.value)" :value="p.value">
                {{ p.label }}
              </option>
            </select>

            <label class="text-ink-dim">API キー変数</label>
            <input
              v-model="draft.apiKeyEnv"
              placeholder="OPENAI_API_KEY"
              class="rounded border border-line bg-surface-0 px-2 py-1 font-mono outline-none focus:border-accent"
            />

            <label class="text-ink-dim">コンテキスト長</label>
            <input
              v-model.number="draft.contextLength"
              type="number"
              class="rounded border border-line bg-surface-0 px-2 py-1 tabular-nums outline-none focus:border-accent"
            />

            <label class="text-ink-dim">最大出力</label>
            <input
              v-model.number="draft.maxOutputTokens"
              type="number"
              class="rounded border border-line bg-surface-0 px-2 py-1 tabular-nums outline-none focus:border-accent"
            />

            <label class="text-ink-dim">temperature</label>
            <input
              :value="draft.temperature ?? ''"
              type="number"
              step="0.1"
              placeholder="空欄 = 送らない"
              class="rounded border border-line bg-surface-0 px-2 py-1 tabular-nums outline-none focus:border-accent"
              @input="onTemperature(($event.target as HTMLInputElement).value)"
            />

            <label class="text-ink-dim">推論の深さ</label>
            <select
              v-model="draft.effort"
              class="rounded border border-line bg-surface-0 px-2 py-1 outline-none focus:border-accent"
            >
              <option v-for="e in EFFORTS" :key="String(e.value)" :value="e.value">
                {{ e.label }}
              </option>
            </select>

            <label class="text-ink-dim">タイムアウト(秒)</label>
            <input
              v-model.number="draft.requestTimeoutSecs"
              type="number"
              class="rounded border border-line bg-surface-0 px-2 py-1 tabular-nums outline-none focus:border-accent"
            />

            <label class="text-ink-dim">最大試行回数</label>
            <input
              v-model.number="draft.maxRetries"
              type="number"
              class="rounded border border-line bg-surface-0 px-2 py-1 tabular-nums outline-none focus:border-accent"
            />

            <label class="text-ink-dim">ツール呼び出し</label>
            <label class="flex items-center gap-2">
              <input v-model="draft.useTools" type="checkbox" />
              <span class="text-ink-dim">
                無効にすると、スキーマをプロンプトに載せる経路へ切り替わります
              </span>
            </label>
          </div>

          <p class="rounded border border-line bg-surface-0 p-2 text-[11px] text-ink-dim">
            API キーの実値は保存されません。ここには<strong class="text-ink">
              環境変数名</strong
            >だけを入れてください。値が解決できない場合はエコー応答へ退避し、その旨が
            会話ログに現れます。
          </p>
        </div>

        <div v-else class="flex-1 p-6 text-center text-[11px] text-ink-dim">
          左の一覧から選ぶか、＋ で新規作成してください。
        </div>

        <div v-if="draft" class="flex justify-end gap-2 border-t border-line px-4 py-2.5">
          <button
            class="rounded px-2 py-1 text-[11px] text-ink-dim hover:text-ink"
            @click="draft = null"
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
    </div>
  </div>
</template>
