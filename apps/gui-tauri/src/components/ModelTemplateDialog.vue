<script setup lang="ts">
/**
 * モデルテンプレートの管理ダイアログ。
 *
 * API キーはここで入力するが、**この画面には戻ってこない**。
 * 値は OS の資格情報ストアへ片道で渡り、読み出す API は存在しない。
 * 表示できるのは「登録済みかどうか」だけ（failures.md #1 / #2）。
 */
import { computed, ref, watch } from "vue";

import * as ipc from "../lib/ipc";
import { askConfirm } from "../composables/useConfirm";
import { useOrchestrator } from "../composables/useOrchestrator";
import type { Effort, ModelTemplate, ModelTemplateId, Provider } from "../types";

const emit = defineEmits<{ (e: "close"): void }>();

const orchestrator = useOrchestrator();
const { state } = orchestrator;

const selectedId = ref<ModelTemplateId | null>(state.templates[0]?.id ?? null);
const draft = ref<ModelTemplate | null>(null);
/** 削除の通信中である行の ID。連打による二重送信を塞ぐ。 */
const removing = ref<ModelTemplateId | null>(null);

/**
 * 編集中の下書きが未登録（新規作成）かどうか。
 *
 * 識別子は登録済みテンプレートでは変更させない。upsert は ID を鍵にするので、
 * 変更して保存すると**改名ではなく複製**になり、古い行が残る。実際に
 * 「削除しても保存しても前回の情報が残る」という形で表面化した。
 * さらに ID は資格情報ストアの鍵でもあり、エージェントからの参照先でもあるため、
 * 変えると登録済みキーが孤児になり、参照していたエージェントが宙に浮く。
 */
const isNewDraft = computed(
  () => !!draft.value && !state.templates.some((t) => t.id === draft.value!.id),
);

/** プロバイダごとの既定 base URL。切り替え時にこの値へ揃える。 */
const DEFAULT_BASE_URL: Record<string, string> = {
  open_ai_compat: "https://api.openai.com/v1",
  anthropic: "https://api.anthropic.com/v1",
  gemini: "https://generativelanguage.googleapis.com/v1beta",
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

/**
 * API キーが資格情報ストアに登録済みか。`null` は未確認。
 *
 * **値そのものは決して取得しない。** 表示に必要なのは有無だけで、
 * 値を持ってくると、秘密が UI 層のメモリに載る理由が無いのに載る。
 */
const credentialStored = ref<boolean | null>(null);
/** 入力中の新しいキー。保存が済んだ時点で即座に破棄する。 */
const secretInput = ref("");
const savingSecret = ref(false);

async function refreshCredentialState(templateId: string | undefined): Promise<void> {
  credentialStored.value = null;
  if (!templateId) return;
  try {
    credentialStored.value = await ipc.modelCredentialExists(templateId);
  } catch {
    credentialStored.value = null;
  }
}

watch(() => draft.value?.id, refreshCredentialState, { immediate: true });

/** 入力されたキーを資格情報ストアへ預ける。 */
async function saveSecret(): Promise<void> {
  const template = draft.value;
  const secret = secretInput.value;
  if (!template || !secret) return;

  const id = template.id;
  savingSecret.value = true;
  try {
    // テンプレート本体が未保存だと、コア側が「未登録」で弾く。先に保存する。
    // 保存に失敗したらここで止める — 続けると「未登録」の 2 つ目のエラーが
    // 重なり、どちらが本当の原因か読めなくなる。
    if (!(await orchestrator.upsertTemplate(template))) return;
    const ok = await orchestrator.setCredential(id, secret);
    if (ok) {
      // 取得元の切り替えはコア側が行うので、その結果を読み直す。
      // 手元で `keyring` を代入すると、コアが別の判断をしたときに食い違う。
      reseedDraft(id);
      credentialStored.value = true;
    }
  } finally {
    // 成否によらず、入力欄に秘密を残さない。
    secretInput.value = "";
    savingSecret.value = false;
  }
}

/** 登録済みのキーを削除する。 */
async function clearSecret(): Promise<void> {
  const template = draft.value;
  if (!template) return;
  const confirmed = await askConfirm({
    title: `${template.name} の API キーを削除しますか？`,
    message: "OS の資格情報ストアから消えます。このテンプレートを使うサーヴァントは接続できなくなります。",
    confirmLabel: "削除する",
    danger: true,
  });
  if (!confirmed) return;

  const id = template.id;
  const ok = await orchestrator.clearCredential(id);
  if (ok) {
    // 取得元は「認証不要」ではなく「未設定」へ戻る（コア側の判断）。
    // ここでも代入せず、その結果を読み直す。
    reseedDraft(id);
    credentialStored.value = false;
  }
}

/**
 * 「認証不要」の明示。ローカル推論サーバなど、キーを要求しない接続先で使う。
 *
 * 未設定のまま送ることは許さないので、キーを入れない構成では
 * ここを明示的に立ててもらう。既定でこちらへ倒すと、入れ忘れが
 * 「要らない」と解釈されて 401 になる。
 */
function setAuthNotRequired(notRequired: boolean): void {
  if (!draft.value) return;
  draft.value.credential = notRequired ? "not_required" : "unset";
}

const PROVIDERS: { value: Provider | null; label: string }[] = [
  { value: null, label: "自動判定（baseUrl から）" },
  { value: "open_ai_compat", label: "OpenAI 互換" },
  { value: "anthropic", label: "Anthropic ネイティブ" },
  { value: "gemini", label: "Gemini ネイティブ" },
];

/**
 * Google 接地のチェックを出してよいか。
 *
 * Gemini を**明示選択**したときだけ。自動判定のままだと OpenAI 互換の口へ出て、
 * `google_search` は 400 で拒否される。押しても効かないチェックを見せない。
 */
const supportsGoogleSearch = computed(() => draft.value?.provider === "gemini");

/**
 * 接地が有効なのに、ワイヤが Gemini ではない状態か。
 *
 * UI からは作れないが、`world.json` を直接編集すれば作れる。**隠すだけだと
 * 真のまま見えなくなる**ので、その時だけ理由を出す。効かない設定が黙って
 * 残っているのが一番たちが悪い（コアは grounding_active() で無効化するが、
 * 利用者から見ると「チェックしたのに検索しない」になる）。
 */
const strandedGoogleSearch = computed(
  () => !!draft.value?.googleSearch && draft.value.provider !== "gemini",
);

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
    maxOutputTokens: 8192,
    credential: "unset",
    provider: null,
    useTools: true,
    effort: null,
    googleSearch: false,
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

/**
 * 保存後の下書きをコア側の値で作り直す。
 *
 * 保存してフォームを閉じる作りだと、**保存した結果が画面から消える**。
 * ユーザーには「反映されていない」ようにしか見えず、実際に
 * 「ダイアログを開き直さないと反映されない」という報告になった。
 * 保存後もその項目に留まり、コアが受け取った値をそのまま表示する。
 */
function reseedDraft(id: ModelTemplateId): void {
  const saved = state.templates.find((t) => t.id === id);
  draft.value = saved ? { ...saved } : null;
  selectedId.value = saved ? saved.id : null;
}

/** 保存の通信中。連打の二重送信を塞ぎ、ボタンの文言も変える。 */
const saving = ref(false);
/** 保存直後の合図。reseed された値は打った値と同一で画面に差分が出ないため、
    これが無いと「押しても無反応」に見える（#6 の処方が作った死角）。 */
const savedFlash = ref(false);
let savedTimer: ReturnType<typeof setTimeout> | undefined;

async function save(): Promise<void> {
  if (!draft.value || saving.value) return;
  const id = draft.value.id;
  saving.value = true;
  try {
    const ok = await orchestrator.upsertTemplate(draft.value);
    if (!ok) return; // 失敗はエラートーストが説明する。成功表示を重ねない。
    reseedDraft(id);
    savedFlash.value = true;
    clearTimeout(savedTimer);
    savedTimer = setTimeout(() => (savedFlash.value = false), 1600);
  } finally {
    saving.value = false;
  }
}

async function remove(template: ModelTemplate): Promise<void> {
  // 連打で 2 回目以降が「存在しません」になるのを、通信中フラグで塞ぐ。
  if (removing.value) return;
  const ok = await askConfirm({
    title: `${template.name} を削除しますか？`,
    message: "このテンプレートを参照しているサーヴァントは、モデルを選び直すまで動きません。",
    confirmLabel: "削除する",
    danger: true,
  });
  if (!ok) return;

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

        <ul class="min-h-0 flex-1 overflow-y-auto p-2">
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

        <div v-if="draft" class="min-h-0 flex-1 space-y-3 overflow-y-auto p-4 text-[12px]">
          <div class="grid grid-cols-[128px_minmax(0,1fr)] items-center gap-x-3 gap-y-2.5">
            <label class="text-ink-dim">識別子</label>
            <div>
              <input
                v-model="draft.id"
                :readonly="!isNewDraft"
                :title="
                  isNewDraft
                    ? '作成後は変更できません'
                    : '登録済みの識別子は変更できません'
                "
                class="w-full rounded border border-line bg-surface-0 px-2 py-1 font-mono outline-none focus:border-accent read-only:opacity-60"
              />
              <p v-if="!isNewDraft" class="mt-1 text-[11px] text-ink-dim">
                識別子は資格情報の鍵とサーヴァントからの参照先を兼ねるため、作成後は変更できません。
              </p>
            </div>

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

            <label class="text-ink-dim">API キー</label>
            <div>
              <div class="flex gap-2">
                <input
                  v-model="secretInput"
                  type="password"
                  autocomplete="off"
                  spellcheck="false"
                  :placeholder="
                    credentialStored ? '新しいキーを貼って上書き' : 'キーを貼り付け'
                  "
                  class="min-w-0 flex-1 rounded border border-line bg-surface-0 px-2 py-1 font-mono outline-none focus:border-accent"
                  @keydown.enter.prevent="saveSecret"
                />
                <button
                  class="rounded bg-accent px-2.5 py-1 text-[11px] font-medium text-surface-0 disabled:opacity-40"
                  :disabled="!secretInput || savingSecret"
                  @click="saveSecret"
                >
                  {{ savingSecret ? "登録中…" : "登録" }}
                </button>
              </div>

              <p v-if="credentialStored === true" class="mt-1 text-[11px] text-run">
                登録済み。OS の資格情報ストアに保存されています。
                <button class="ml-1 underline text-fail hover:opacity-80" @click="clearSecret">
                  削除
                </button>
              </p>

              <template v-else>
                <p
                  v-if="draft.credential === 'unset'"
                  class="mt-1 text-[11px] text-warn"
                >
                  未登録です。このままでは送信前に設定不備として弾かれ、エコー応答へ
                  退避します。
                </p>
                <label class="mt-1.5 flex items-center gap-2 text-[11px] text-ink-dim">
                  <input
                    type="checkbox"
                    :checked="draft.credential === 'not_required'"
                    @change="
                      setAuthNotRequired(($event.target as HTMLInputElement).checked)
                    "
                  />
                  <span>
                    この接続先は認証不要（ローカル推論サーバなど）
                  </span>
                </label>
              </template>
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

            <!--
              Gemini ネイティブを選んだときだけ出す。OpenAI 互換の口では
              google_search が 400 で拒否されるため、押しても効かない。
            -->
            <template v-if="supportsGoogleSearch">
              <label class="text-ink-dim">Google グラウンディング</label>
              <label class="flex items-center gap-2">
                <input v-model="draft.googleSearch" type="checkbox" />
                <span class="text-ink-dim">
                  Google 検索で裏取りしてから答えます（同梱ツールや委譲とは併用できます）
                </span>
              </label>
            </template>

            <!--
              効かない設定が残っている状態。隠すだけだと真のまま見えなくなるので、
              その時だけ理由と直し方を出す。
            -->
            <template v-else-if="strandedGoogleSearch">
              <label class="text-warn">Google グラウンディング</label>
              <p class="text-warn">
                有効になっていますが、<strong>プロトコルが Gemini ではないため働きません</strong>。
                グラウンディングは Gemini ネイティブ経路にしかありません。使うならプロトコルを
                「Gemini ネイティブ」にしてください。
              </p>
            </template>
          </div>

          <p class="rounded border border-line bg-surface-0 p-2 text-[11px] text-ink-dim">
            API キーは <strong class="text-ink">OS の資格情報ストア</strong>
            （Windows 資格情報マネージャー / macOS キーチェーン / Secret Service）に
            保存されます。設定ファイルには一切書かれず、この画面へ読み戻す経路も
            ありません。表示できるのは「登録済みかどうか」だけです。
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
          <!-- 保存の合図はボタン自身に出す。視線は押した場所にあるので、
               離れたトーストより確実に届く。成功時だけ緑 + ✓ を 1.6 秒。 -->
          <button
            class="rounded px-3 py-1 text-[11px] font-medium text-surface-0 transition-colors disabled:opacity-60"
            :class="savedFlash ? 'bg-run' : 'bg-accent'"
            :disabled="saving"
            @click="save"
          >
            {{ saving ? "保存中…" : savedFlash ? "保存しました ✓" : "保存" }}
          </button>
        </div>
      </div>
    </div>
  </div>
</template>
