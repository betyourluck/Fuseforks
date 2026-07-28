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
/** 削除の通信中である行の ID。連打による二重送信を塞ぐ。 */
const removing = ref<ModelTemplateId | null>(null);

/** プロバイダごとの既定 base URL。切り替え時にこの値へ揃える。 */
const DEFAULT_BASE_URL: Record<string, string> = {
  open_ai_compat: "https://api.openai.com/v1",
  anthropic: "https://api.anthropic.com/v1",
};

/** 既知の既定値のいずれかであれば、プロバイダ変更に追随してよいと判断する。 */
const KNOWN_DEFAULTS = Object.values(DEFAULT_BASE_URL);

/**
 * プロバイダを変えたとき、base URL が別プロバイダの既定値のままなら差し替える。
 *
 * 実際に `provider: anthropic` と `baseUrl: api.openai.com` の組み合わせが
 * 保存され、Anthropic のモデル名で OpenAI へ送る設定ができてしまっていた。
 * ユーザーが手で入れた URL（プロキシ等）は正当なので、既定値のときだけ触る。
 */
function onProviderChange(next: string | null): void {
  if (!draft.value) return;
  draft.value.provider = (next as ModelTemplate["provider"]) ?? null;
  if (!next) return;

  const preset = DEFAULT_BASE_URL[next];
  if (preset && KNOWN_DEFAULTS.includes(draft.value.baseUrl)) {
    draft.value.baseUrl = preset;
  }
}

/**
 * プロバイダと base URL が明らかに食い違っているか。
 *
 * 判定するのは「**他社の既定値がそのまま残っている**」という一点だけ。
 * 自前のプロキシやローカルサーバの URL は正当なので、警告してはいけない。
 * 実際に `provider: anthropic` + `api.openai.com` の設定が保存され、
 * 起動しても必ず失敗する状態になっていた。
 */
const baseUrlMismatch = computed(() => {
  const d = draft.value;
  if (!d?.provider) return null;

  const expected = DEFAULT_BASE_URL[d.provider];
  if (!expected || d.baseUrl === expected) return null;
  // 他社の既定値そのものである場合に限って指摘する。
  return KNOWN_DEFAULTS.includes(d.baseUrl) ? expected : null;
});

/** API キー欄が環境変数名の書式か。Rust 側の `is_env_var_name` と同じ規則。 */
const apiKeyEnvLooksValid = computed(() => {
  const value = draft.value?.apiKeyEnv;
  if (!value) return true;
  return /^[A-Za-z_][A-Za-z0-9_]*$/.test(value) && value.length <= 128;
});

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
  // 連打で 2 回目以降が「存在しません」になるのを、通信中フラグで塞ぐ。
  if (removing.value) return;
  if (!confirm(`${template.name} を削除しますか？`)) return;

  removing.value = template.id;
  try {
    await orchestrator.deleteTemplate(template.id);
    if (selectedId.value === template.id) selectedId.value = null;
    if (draft.value?.id === template.id) draft.value = null;
  } finally {
    removing.value = null;
  }
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
            <!--
              選択と削除は兄弟の <button> にする。
              入れ子の <button> は不正な HTML で、内側のクリックが外側にも
              解釈されうる。実際に削除が意図せず二重発火していた。
            -->
            <div
              class="group flex items-center gap-1 rounded"
              :class="
                selectedId === template.id ? 'bg-surface-2' : 'hover:bg-surface-2'
              "
            >
              <button
                class="min-w-0 flex-1 px-2 py-1.5 text-left text-[12px]"
                @click="edit(template)"
              >
                <span class="block truncate">{{ template.name }}</span>
                <span class="block truncate text-[10px] text-ink-dim">
                  {{ template.model }}
                </span>
              </button>
              <button
                class="invisible px-2 py-1.5 text-fail group-hover:visible disabled:opacity-40"
                title="削除"
                :disabled="removing === template.id"
                @click="remove(template)"
              >
                ✕
              </button>
            </div>
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
            <div>
              <input
                v-model="draft.baseUrl"
                placeholder="https://api.openai.com/v1"
                class="w-full rounded border bg-surface-0 px-2 py-1 font-mono outline-none"
                :class="
                  baseUrlMismatch
                    ? 'border-warn focus:border-warn'
                    : 'border-line focus:border-accent'
                "
              />
              <p v-if="baseUrlMismatch" class="mt-1 text-[11px] text-warn">
                選択中のプロトコルと送信先が食い違っています。この設定では必ず失敗します。
                <button
                  class="ml-1 underline hover:text-ink"
                  @click="draft.baseUrl = baseUrlMismatch"
                >
                  {{ baseUrlMismatch }} に直す
                </button>
              </p>
            </div>

            <label class="text-ink-dim">モデル名</label>
            <input
              v-model="draft.model"
              class="rounded border border-line bg-surface-0 px-2 py-1 font-mono outline-none focus:border-accent"
            />

            <label class="text-ink-dim">プロトコル</label>
            <select
              :value="draft.provider"
              class="rounded border border-line bg-surface-0 px-2 py-1 outline-none focus:border-accent"
              @change="
                onProviderChange(
                  ($event.target as HTMLSelectElement).value || null,
                )
              "
            >
              <option v-for="p in PROVIDERS" :key="String(p.value)" :value="p.value ?? ''">
                {{ p.label }}
              </option>
            </select>

            <label class="text-ink-dim">API キーの環境変数名</label>
            <div>
              <input
                v-model="draft.apiKeyEnv"
                placeholder="ANTHROPIC_API_KEY"
                autocomplete="off"
                spellcheck="false"
                class="w-full rounded border bg-surface-0 px-2 py-1 font-mono outline-none"
                :class="
                  apiKeyEnvLooksValid
                    ? 'border-line focus:border-accent'
                    : 'border-fail focus:border-fail'
                "
              />
              <p v-if="!apiKeyEnvLooksValid" class="mt-1 text-[11px] text-fail">
                ここは<strong>変数名</strong>の欄です。キーの実値は保存できません
                （英大小文字・数字・<code>_</code> のみ、先頭は数字以外）。
              </p>
            </div>

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
            API キーの実値は<strong class="text-ink">保存できません</strong>。
            設定は平文のファイルに書かれるため、書式検査で入口を塞いでいます。
            ここには環境変数名（例
            <code class="text-ink">ANTHROPIC_API_KEY</code>）を入れ、実値は OS の
            環境変数に設定してアプリを起動してください。値が解決できない場合は
            エコー応答へ退避し、その旨が会話ログに現れます。
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
            class="rounded bg-accent px-3 py-1 text-[11px] font-medium text-surface-0 disabled:opacity-40"
            :disabled="!apiKeyEnvLooksValid"
            :title="apiKeyEnvLooksValid ? '保存' : 'API キーの環境変数名を修正してください'"
            @click="save"
          >
            保存
          </button>
        </div>
      </div>
    </div>
  </div>
</template>
