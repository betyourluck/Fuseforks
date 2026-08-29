<script setup lang="ts">
/**
 * モデルテンプレートの管理ダイアログ。
 *
 * API キーはここで入力するが、**この画面には戻ってこない**。
 * 値は OS の資格情報ストアへ片道で渡り、読み出す API は存在しない。
 * 表示できるのは「登録済みかどうか」だけ（failures.md #1 / #2）。
 */
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";

import * as ipc from "../lib/ipc";
import { parsePriceInput, todayIsoDate } from "../lib/priceInput";
import {
  baseUrlMismatch as checkBaseUrlMismatch,
  presetBaseUrlFor,
  providerSkills,
  suggestsResponses,
} from "../lib/providerSkills";
import { askConfirm } from "../composables/useConfirm";
import { useOrchestrator } from "../composables/useOrchestrator";
import type { Effort, ModelTemplate, ModelTemplateId, Provider } from "../types";

const emit = defineEmits<{ (e: "close"): void }>();

const { t } = useI18n();
const orchestrator = useOrchestrator();
const { state } = orchestrator;

const selectedId = ref<ModelTemplateId | null>(state.templates[0]?.id ?? null);
const draft = ref<ModelTemplate | null>(null);

/**
 * 単価の入力欄（Spec 41）。**文字列で持って、保存の直前に数値へ寄せる。**
 *
 * `v-model.number` を使わないのは、**空欄と 0 を区別する**ため — `.number` は
 * 空文字を `0` にするので、「未設定」が「無料」に化ける。**未設定は 0 ではない。**
 *
 * **setter には `string | number` が届く**（2026-08-19 の実バグ）。`type="number"` の
 * `<input>` では `.number` を付けなくても Vue が数値へ自動変換するので、文字列前提の
 * `raw.trim()` は数字を打った瞬間に `TypeError` で死に、欄が書かれなかった。
 * 変換は `parsePriceInput`（純関数・`lib/priceInput.ts`）の 1 実装に寄せてある。
 */
function priceField(key: keyof ModelTemplate) {
  return computed<string | number>({
    get: () => {
      const v = draft.value?.[key];
      return typeof v === "number" ? String(v) : "";
    },
    set: (raw) => {
      if (!draft.value) return;
      // 空欄 = 未設定へ戻す。数値でなければ触らない（通貨記号を書かせない）。
      const next = parsePriceInput(raw);
      if (next !== undefined) {
        (draft.value as unknown as Record<string, number | null>)[key as string] = next;
        // **手入力した日が「この単価を信じられる時点」**（Spec 41 D1: 取得なら JSON の
        // 日付、手入力ならその日）。空欄へ戻すだけのときは触らない。
        if (next !== null) draft.value.pricingAsOf = todayIsoDate(new Date());
      }
    },
  });
}

const priceInput = priceField("inputPerMtok");
const priceOutput = priceField("outputPerMtok");
const priceCacheRead = priceField("cacheReadPerMtok");
const priceCacheWrite = priceField("cacheWritePerMtok");
const priceCacheWrite1h = priceField("cacheWrite1hPerMtok");

const fetchingPrices = ref(false);
const priceNotice = ref("");

/**
 * 単価表から引いて**欄に入れるだけ**（Spec 41 D3）。**保存はしない。**
 *
 * **取得できなかった欄は触らない** — 手で入れた値を空で潰さない。
 * 呼ばれるのは**このボタンだけ**で、起動・画面遷移・タイマーからは呼ばない
 * （`data_contract` の `pricing_fetch_freeze`）。
 */
async function fetchPrices() {
  if (!draft.value || fetchingPrices.value) return;
  fetchingPrices.value = true;
  priceNotice.value = "";
  try {
    const table = await ipc.fetchModelPrices();
    const hit = table.models.find((m) => m.key === draft.value?.model);
    if (!hit) {
      priceNotice.value = t("modelTemplate.pricing.notFound", { model: draft.value.model });
      return;
    }
    const d = draft.value;
    if (hit.inputPerMtok !== null) d.inputPerMtok = hit.inputPerMtok;
    if (hit.outputPerMtok !== null) d.outputPerMtok = hit.outputPerMtok;
    if (hit.cacheReadPerMtok !== null) d.cacheReadPerMtok = hit.cacheReadPerMtok;
    if (hit.cacheWritePerMtok !== null) d.cacheWritePerMtok = hit.cacheWritePerMtok;
    if (hit.cacheWrite1hPerMtok !== null) d.cacheWrite1hPerMtok = hit.cacheWrite1hPerMtok;
    d.pricingAsOf = table.asOf;
    priceNotice.value =
      table.dropped > 0
        ? t("modelTemplate.pricing.filledWithDropped", { n: table.dropped })
        : t("modelTemplate.pricing.filled");
  } catch (err) {
    priceNotice.value = String((err as { message?: string })?.message ?? err);
  } finally {
    fetchingPrices.value = false;
  }
}
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

/**
 * プロバイダを変えたとき、base URL が別プロバイダの既定値のままなら差し替える。
 * 判定は `lib/providerSkills` の純関数（テストで留めてある）。
 */
function onProviderChange(next: string | null): void {
  if (!draft.value) return;
  draft.value.provider = (next as ModelTemplate["provider"]) ?? null;

  const preset = presetBaseUrlFor(next, draft.value.baseUrl);
  if (preset) draft.value.baseUrl = preset;
}

/**
 * プロバイダと base URL が明らかに食い違っているか。
 *
 * 判定するのは「**他社の既定値がそのまま残っている**」という一点だけ。
 * 自前のプロキシやローカルサーバの URL は正当なので、警告してはいけない。
 * 実際に `provider: anthropic` + `api.openai.com` の設定が保存され、
 * 起動しても必ず失敗する状態になっていた。
 */
const baseUrlMismatch = computed(() =>
  draft.value ? checkBaseUrlMismatch(draft.value.provider, draft.value.baseUrl) : null,
);

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
    title: t("modelTemplate.deleteKeyTitle", { name: template.name }),
    message: t("modelTemplate.deleteKeyMessage"),
    confirmLabel: t("modelTemplate.deleteAction"),
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

const PROVIDERS: { value: Provider | null; labelKey: string }[] = [
  { value: null, labelKey: "modelTemplate.providerAuto" },
  { value: "open_ai_compat", labelKey: "modelTemplate.providerOpenAiCompat" },
  { value: "anthropic", labelKey: "modelTemplate.providerAnthropic" },
  { value: "gemini", labelKey: "modelTemplate.providerGemini" },
  { value: "xai_responses", labelKey: "modelTemplate.providerXaiResponses" },
  { value: "open_ai_responses", labelKey: "modelTemplate.providerOpenAiResponses" },
  { value: "meta_responses", labelKey: "modelTemplate.providerMetaResponses" },
  {
    value: "perplexity_responses",
    labelKey: "modelTemplate.providerPerplexityResponses",
  },
];

/**
 * 固有スキルの出し分け（Spec 31 D3 / D4）。表示条件と「効かない設定が残って
 * いる」の判定は対なので、1 本の純関数から両方引く。判定は provider であって
 * モデル名ではない（D2 — モデル名で分けると `Provider::detect` と 2 系統になる）。
 */
const skills = computed(() =>
  providerSkills(
    draft.value ?? {
      provider: null,
      googleSearch: false,
      xaiWebSearch: false,
      xaiXSearch: false,
      openaiWebSearch: false,
      openaiReasoningPro: false,
      metaWebSearch: false,
      perplexityWebSearch: false,
      perplexityFinanceSearch: false,
      perplexityPeopleSearch: false,
      perplexityFetchUrl: false,
    },
  ),
);

/**
 * 効かない設定が残っている行の一覧（表示は下の 1 つの v-for が描く）。
 *
 * **ラベルには持ち主のワイヤ名を併記する** — 同じ表示名「web 検索」が
 * OpenAI と Perplexity の 2 ワイヤに実在するので、スキル名だけの警告は
 * すぐ上に見えているチェック済みトグルへの警告と読み違える
 * （2026-08-30 実機の指摘。Perplexity のテンプレートに OpenAI 時代の
 * 残骸フラグが警告を出し続けていた）。
 *
 * **`clear` はその場で残骸を消す唯一の UI 経路。** これが無いと、消すには
 * プロトコルを往復するか `world.json` の手編集しかなく、警告が
 * 「直せない小言」として残り続ける。
 */
const strandedRows = computed(() => {
  const s = skills.value;
  const d = draft.value;
  if (!d) return [];
  const rows: {
    key: string;
    labelKey: string;
    ownerKey: string;
    strongKey: string;
    afterKey: string;
    clear: () => void;
  }[] = [];
  if (s.metaWeb.stranded)
    rows.push({
      key: "metaWeb",
      labelKey: "modelTemplate.metaWebSearch",
      ownerKey: "modelTemplate.providerMetaResponses",
      strongKey: "modelTemplate.strandedMetaStrong",
      afterKey: "modelTemplate.strandedMetaAfter",
      clear: () => (d.metaWebSearch = false),
    });
  if (s.google.stranded)
    rows.push({
      key: "google",
      labelKey: "modelTemplate.googleSearch",
      ownerKey: "modelTemplate.providerGemini",
      strongKey: "modelTemplate.strandedStrong",
      afterKey: "modelTemplate.strandedAfter",
      clear: () => (d.googleSearch = false),
    });
  if (s.xaiWeb.stranded)
    rows.push({
      key: "xaiWeb",
      labelKey: "modelTemplate.xaiWebSearch",
      ownerKey: "modelTemplate.providerXaiResponses",
      strongKey: "modelTemplate.strandedXaiStrong",
      afterKey: "modelTemplate.strandedXaiAfter",
      clear: () => (d.xaiWebSearch = false),
    });
  if (s.xaiX.stranded)
    rows.push({
      key: "xaiX",
      labelKey: "modelTemplate.xaiXSearch",
      ownerKey: "modelTemplate.providerXaiResponses",
      strongKey: "modelTemplate.strandedXaiStrong",
      afterKey: "modelTemplate.strandedXaiAfter",
      clear: () => (d.xaiXSearch = false),
    });
  if (s.openaiWeb.stranded)
    rows.push({
      key: "openaiWeb",
      labelKey: "modelTemplate.openaiWebSearch",
      ownerKey: "modelTemplate.providerOpenAiResponses",
      strongKey: "modelTemplate.strandedOpenAiStrong",
      afterKey: "modelTemplate.strandedOpenAiAfter",
      clear: () => (d.openaiWebSearch = false),
    });
  if (s.perplexityWeb.stranded)
    rows.push({
      key: "perplexityWeb",
      labelKey: "modelTemplate.perplexityWebSearch",
      ownerKey: "modelTemplate.providerPerplexityResponses",
      strongKey: "modelTemplate.strandedPerplexityStrong",
      afterKey: "modelTemplate.strandedPerplexityAfter",
      clear: () => (d.perplexityWebSearch = false),
    });
  if (s.perplexityFinance.stranded)
    rows.push({
      key: "perplexityFinance",
      labelKey: "modelTemplate.perplexityFinanceSearch",
      ownerKey: "modelTemplate.providerPerplexityResponses",
      strongKey: "modelTemplate.strandedPerplexityStrong",
      afterKey: "modelTemplate.strandedPerplexityAfter",
      clear: () => (d.perplexityFinanceSearch = false),
    });
  if (s.perplexityPeople.stranded)
    rows.push({
      key: "perplexityPeople",
      labelKey: "modelTemplate.perplexityPeopleSearch",
      ownerKey: "modelTemplate.providerPerplexityResponses",
      strongKey: "modelTemplate.strandedPerplexityStrong",
      afterKey: "modelTemplate.strandedPerplexityAfter",
      clear: () => (d.perplexityPeopleSearch = false),
    });
  if (s.perplexityFetch.stranded)
    rows.push({
      key: "perplexityFetch",
      labelKey: "modelTemplate.perplexityFetchUrl",
      ownerKey: "modelTemplate.providerPerplexityResponses",
      strongKey: "modelTemplate.strandedPerplexityStrong",
      afterKey: "modelTemplate.strandedPerplexityAfter",
      clear: () => (d.perplexityFetchUrl = false),
    });
  if (s.openaiPro.stranded)
    rows.push({
      key: "openaiPro",
      labelKey: "modelTemplate.openaiReasoningPro",
      ownerKey: "modelTemplate.providerOpenAiResponses",
      strongKey: "modelTemplate.strandedOpenAiStrong",
      afterKey: "modelTemplate.strandedOpenAiAfter",
      clear: () => (d.openaiReasoningPro = false),
    });
  return rows;
});

/**
 * Responses へ切り替えると能力が増えるテンプレートか（Spec 34・利用者裁定
 * 2026-08-11）。**案内であって判定ではない** — ワイヤの選択は provider だけで
 * 決まり、ここはモデル名も見るが効き目には一切触れない。
 *
 * 実機で「gpt-5.6 に固有スキルが出ない」が最初の確認で出た。原因は設計どおり
 * （自動判定しない = D7）だが、**既存の gpt-* テンプレートは 100% が互換**なので、
 * 切り替えるまで新しい能力が在ることが画面のどこにも出なかった。
 */
const responsesHint = computed(
  () => draft.value !== null && suggestsResponses(draft.value),
);

const EFFORTS: { value: Effort | null; label: string }[] = [
  // null の表示は辞書キー modelTemplate.effortNone で引く（テンプレート側で分岐）。
  { value: null, label: "" },
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
    name: t("modelTemplate.newTemplateName"),
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
    xaiWebSearch: false,
    xaiXSearch: false,
    // Spec 34 P1 は既定値だけ。トグルと注記（固定費 4,434 / +1,538）は P2。
    openaiWebSearch: false,
    openaiReasoningPro: false,
    // Spec 37。既定 OFF（検索は入力を桁で膨らませる — 実測 66,350）。
    metaWebSearch: false,
    // Spec 45。4 本とも既定 OFF（finance / people は 1 回 $0.005 の課金つき）。
    perplexityWebSearch: false,
    perplexityFinanceSearch: false,
    perplexityPeopleSearch: false,
    perplexityFetchUrl: false,
    requestTimeoutSecs: 120,
    maxRetries: 3,
    // 単価は既定を持たない（Spec 41）。**0 ではなく未設定**で始まり、
    // 「取得」か手入力で初めて埋まる。
    inputPerMtok: null,
    outputPerMtok: null,
    cacheReadPerMtok: null,
    cacheWritePerMtok: null,
    cacheWrite1hPerMtok: null,
    pricingAsOf: null,
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
    title: t("modelTemplate.deleteTemplateTitle", { name: template.name }),
    message: t("modelTemplate.deleteTemplateMessage"),
    confirmLabel: t("modelTemplate.deleteAction"),
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
    class="fixed inset-0 z-40 flex items-center justify-center bg-scrim"
    @click.self="emit('close')"
  >
    <div
      class="flex h-[560px] w-[760px] overflow-hidden rounded-lg border border-line bg-surface-1 shadow-2xl"
    >
      <!-- 一覧 -->
      <div class="flex w-56 shrink-0 flex-col border-r border-line">
        <header class="flex items-center border-b border-line px-3 py-2 text-xs">
          <h2 class="flex-1 font-semibold">{{ $t("modelTemplate.title") }}</h2>
          <button
            class="rounded border border-line px-1.5 hover:border-accent hover:text-accent"
            :title="$t('modelTemplate.addTitle')"
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
                :title="$t('modelTemplate.deleteTitle')"
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
            {{ $t("modelTemplate.empty") }}
          </li>
        </ul>
      </div>

      <!-- 編集フォーム -->
      <div class="flex min-w-0 flex-1 flex-col">
        <header class="flex items-center border-b border-line px-3 py-2 text-xs">
          <h3 class="flex-1 truncate font-semibold">
            {{ draft ? draft.name : (current?.name ?? $t("modelTemplate.selectPrompt")) }}
          </h3>
          <button class="px-1 text-ink-dim hover:text-ink" @click="emit('close')">
            ✕
          </button>
        </header>

        <div v-if="draft" class="min-h-0 flex-1 space-y-3 overflow-y-auto p-4 text-[12px]">
          <div class="grid grid-cols-[128px_minmax(0,1fr)] items-center gap-x-3 gap-y-2.5">
            <label class="text-ink-dim">{{ $t("modelTemplate.id") }}</label>
            <div>
              <input
                v-model="draft.id"
                :readonly="!isNewDraft"
                :title="
                  isNewDraft
                    ? $t('modelTemplate.idNewTitle')
                    : $t('modelTemplate.idLockedTitle')
                "
                class="w-full rounded border border-line bg-surface-0 px-2 py-1 font-mono outline-none focus:border-accent read-only:opacity-60"
              />
              <p v-if="!isNewDraft" class="mt-1 text-[11px] text-ink-dim">
                {{ $t("modelTemplate.idLockedHint") }}
              </p>
            </div>

            <label class="text-ink-dim">{{ $t("modelTemplate.displayName") }}</label>
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
                {{ $t("modelTemplate.baseUrlMismatch") }}
                <button
                  class="ml-1 underline hover:text-ink"
                  @click="draft.baseUrl = baseUrlMismatch"
                >
                  {{ $t("modelTemplate.fixBaseUrl", { url: baseUrlMismatch }) }}
                </button>
              </p>
            </div>

            <label class="text-ink-dim">{{ $t("modelTemplate.model") }}</label>
            <input
              v-model="draft.model"
              class="rounded border border-line bg-surface-0 px-2 py-1 font-mono outline-none focus:border-accent"
            />

            <label class="text-ink-dim">{{ $t("modelTemplate.protocol") }}</label>
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
                {{ $t(p.labelKey) }}
              </option>
            </select>

            <!--
              切り替えの案内。**プロトコルを選ぶその場所に出す** — 固有スキルの
              節は切り替えるまで現れないので、そこに書いても届かない。
              警告色にしないのは、いまの設定が壊れているわけではないため
              （stranded は「効かない設定が残っている」で、こちらは「もっと使える」）。
            -->
            <div v-if="responsesHint" class="col-span-2 -mt-1 text-[11px] text-ink-dim">
              {{ $t("modelTemplate.responsesHint") }}
            </div>

            <label class="text-ink-dim">{{ $t("modelTemplate.apiKey") }}</label>
            <div>
              <div class="flex gap-2">
                <input
                  v-model="secretInput"
                  type="password"
                  autocomplete="off"
                  spellcheck="false"
                  :placeholder="
                    credentialStored
                      ? $t('modelTemplate.keyPlaceholderStored')
                      : $t('modelTemplate.keyPlaceholder')
                  "
                  class="min-w-0 flex-1 rounded border border-line bg-surface-0 px-2 py-1 font-mono outline-none focus:border-accent"
                  @keydown.enter.prevent="saveSecret"
                />
                <button
                  class="rounded bg-accent px-2.5 py-1 text-[11px] font-medium text-surface-0 disabled:opacity-40"
                  :disabled="!secretInput || savingSecret"
                  @click="saveSecret"
                >
                  {{ savingSecret ? $t("modelTemplate.registering") : $t("modelTemplate.register") }}
                </button>
              </div>

              <p v-if="credentialStored === true" class="mt-1 text-[11px] text-run">
                {{ $t("modelTemplate.keyStored") }}
                <button class="ml-1 underline text-fail hover:opacity-80" @click="clearSecret">
                  {{ $t("modelTemplate.delete") }}
                </button>
              </p>

              <template v-else>
                <p
                  v-if="draft.credential === 'unset'"
                  class="mt-1 text-[11px] text-warn"
                >
                  {{ $t("modelTemplate.keyMissing") }}
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
                    {{ $t("modelTemplate.authNotRequired") }}
                  </span>
                </label>
              </template>
            </div>

            <label class="text-ink-dim">{{ $t("modelTemplate.contextLength") }}</label>
            <input
              v-model.number="draft.contextLength"
              type="number"
              class="rounded border border-line bg-surface-0 px-2 py-1 tabular-nums outline-none focus:border-accent"
            />

            <label class="text-ink-dim">{{ $t("modelTemplate.maxOutput") }}</label>
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
              :placeholder="$t('modelTemplate.temperaturePlaceholder')"
              class="rounded border border-line bg-surface-0 px-2 py-1 tabular-nums outline-none focus:border-accent"
              @input="onTemperature(($event.target as HTMLInputElement).value)"
            />

            <label class="text-ink-dim">{{ $t("modelTemplate.effort") }}</label>
            <select
              v-model="draft.effort"
              class="rounded border border-line bg-surface-0 px-2 py-1 outline-none focus:border-accent"
            >
              <option v-for="e in EFFORTS" :key="String(e.value)" :value="e.value">
                {{ e.value === null ? $t("modelTemplate.effortNone") : e.label }}
              </option>
            </select>

            <label class="text-ink-dim">{{ $t("modelTemplate.timeout") }}</label>
            <input
              v-model.number="draft.requestTimeoutSecs"
              type="number"
              class="rounded border border-line bg-surface-0 px-2 py-1 tabular-nums outline-none focus:border-accent"
            />

            <label class="text-ink-dim">{{ $t("modelTemplate.maxRetries") }}</label>
            <input
              v-model.number="draft.maxRetries"
              type="number"
              class="rounded border border-line bg-surface-0 px-2 py-1 tabular-nums outline-none focus:border-accent"
            />

            <!--
              単価（Spec 41）。**5 欄 + 日付。** 3 欄（入力 / 出力 / キャッシュ）では
              実測で 1.52 倍ずれるので、書き込みを独立させてある。
              **空欄は無視ではなく 1 段上へ落ちる**ので、書き込みの概念が無い
              モデル（Gemini / xAI）では空が正しい状態。
            -->
            <label class="col-span-2 mt-2 text-xs font-semibold text-ink-dim">
              {{ $t("modelTemplate.pricing.title") }}
            </label>
            <label class="col-span-2 -mt-1 text-xs text-ink-dim">
              {{ $t("modelTemplate.pricing.hint") }}
            </label>

            <label class="text-ink-dim">{{ $t("modelTemplate.pricing.input") }}</label>
            <input
              v-model="priceInput"
              type="number"
              step="any"
              min="0"
              :placeholder="$t('modelTemplate.pricing.unset')"
              class="rounded border border-line bg-surface-0 px-2 py-1 tabular-nums outline-none focus:border-accent"
            />

            <label class="text-ink-dim">{{ $t("modelTemplate.pricing.output") }}</label>
            <input
              v-model="priceOutput"
              type="number"
              step="any"
              min="0"
              :placeholder="$t('modelTemplate.pricing.unset')"
              class="rounded border border-line bg-surface-0 px-2 py-1 tabular-nums outline-none focus:border-accent"
            />

            <label class="text-ink-dim">{{ $t("modelTemplate.pricing.cacheRead") }}</label>
            <input
              v-model="priceCacheRead"
              type="number"
              step="any"
              min="0"
              :placeholder="$t('modelTemplate.pricing.fallsBack')"
              class="rounded border border-line bg-surface-0 px-2 py-1 tabular-nums outline-none focus:border-accent"
            />

            <label class="text-ink-dim">{{ $t("modelTemplate.pricing.cacheWrite") }}</label>
            <input
              v-model="priceCacheWrite"
              type="number"
              step="any"
              min="0"
              :placeholder="$t('modelTemplate.pricing.fallsBack')"
              class="rounded border border-line bg-surface-0 px-2 py-1 tabular-nums outline-none focus:border-accent"
            />

            <label class="text-ink-dim">{{ $t("modelTemplate.pricing.cacheWrite1h") }}</label>
            <input
              v-model="priceCacheWrite1h"
              type="number"
              step="any"
              min="0"
              :placeholder="$t('modelTemplate.pricing.fallsBack')"
              class="rounded border border-line bg-surface-0 px-2 py-1 tabular-nums outline-none focus:border-accent"
            />

            <label class="text-ink-dim">{{ $t("modelTemplate.pricing.asOf") }}</label>
            <div class="flex items-center gap-2">
              <span class="tabular-nums text-ink-dim">
                {{ draft.pricingAsOf ?? $t("modelTemplate.pricing.unset") }}
              </span>
              <button
                type="button"
                class="rounded border border-line px-2 py-1 text-xs hover:border-accent disabled:opacity-50"
                :disabled="fetchingPrices"
                @click="fetchPrices"
              >
                {{ fetchingPrices ? $t("modelTemplate.pricing.fetching") : $t("modelTemplate.pricing.fetch") }}
              </button>
            </div>
            <p v-if="priceNotice" class="col-span-2 -mt-1 text-xs text-ink-dim">
              {{ priceNotice }}
            </p>

            <label class="text-ink-dim">{{ $t("modelTemplate.toolCalls") }}</label>
            <label class="flex items-center gap-2">
              <input v-model="draft.useTools" type="checkbox" />
              <span class="text-ink-dim">
                {{ $t("modelTemplate.toolCallsHint") }}
              </span>
            </label>

            <!--
              固有スキル（Spec 31）。**そのワイヤを明示選択したときだけ**出す。
              判定は provider であってモデル名ではない（D2）。見出しを置くのは、
              ここから先が「そのプロバイダにしか無い能力」だと読める必要が
              あるため — 上のトークン上限や思考段階は全社共通の欄で、性質が違う。
            -->
            <template v-if="skills.anyOffered">
              <div class="col-span-2 mt-1 border-t border-line pt-2 text-[11px] font-medium text-ink-dim">
                {{ $t("modelTemplate.vendorSkills") }}
              </div>
            </template>

            <!--
              Gemini ネイティブを選んだときだけ出す。OpenAI 互換の口では
              google_search が 400 で拒否されるため、押しても効かない。
            -->
            <template v-if="skills.google.offered">
              <label class="text-ink-dim">{{ $t("modelTemplate.googleSearch") }}</label>
              <label class="flex items-center gap-2">
                <input v-model="draft.googleSearch" type="checkbox" />
                <span class="text-ink-dim">
                  {{ $t("modelTemplate.googleSearchHint") }}
                </span>
              </label>
            </template>

            <!--
              Grok の Live Search（Spec 31 D3）。web と X を**別トグル**にするのは、
              別ツール・別課金・別 output 種別と実測済みのため。1 つに畳むと
              web だけ欲しい村が X の攻撃面まで一緒に開けることになる。
            -->
            <template v-if="skills.xaiWeb.offered">
              <label class="text-ink-dim">{{ $t("modelTemplate.xaiWebSearch") }}</label>
              <label class="flex items-center gap-2">
                <input v-model="draft.xaiWebSearch" type="checkbox" />
                <span class="text-ink-dim">
                  {{ $t("modelTemplate.xaiWebSearchHint") }}
                </span>
              </label>
            </template>

            <template v-if="skills.xaiX.offered">
              <label class="text-ink-dim">{{ $t("modelTemplate.xaiXSearch") }}</label>
              <label class="flex items-center gap-2">
                <input v-model="draft.xaiXSearch" type="checkbox" />
                <span class="text-ink-dim">
                  {{ $t("modelTemplate.xaiXSearchHint") }}
                </span>
              </label>
            </template>

            <!--
              OpenAI の web 検索（Spec 34 D5）。**トグルは 1 つ** — xAI が 2 つに
              割れたのは別ツール・別課金・別 output 種別を実測したからで、
              数を写す理由は無い。送る type は `web_search`（`web_search_preview`
              は別名で、input_tokens が 1 バイトも違わない）。
            -->
            <template v-if="skills.openaiWeb.offered">
              <label class="text-ink-dim">{{ $t("modelTemplate.openaiWebSearch") }}</label>
              <label class="flex items-center gap-2">
                <input v-model="draft.openaiWebSearch" type="checkbox" />
                <span class="text-ink-dim">
                  {{ $t("modelTemplate.openaiWebSearchHint") }}
                </span>
              </label>
            </template>

            <!--
              Pro 推論モード（Spec 34 D4）。**トグルで足りる** — `mode` を
              送らないのと `"standard"` は input_tokens が完全に一致する
              （実測 20 / 20）ので、省略 ≡ standard。
              **注記に数字を 2 つ書く** — 精度（Terra 23.3 → 28.5）と入力の
              固定費（+1,538）。片方だけだと押す判断ができない。
            -->
            <template v-if="skills.openaiPro.offered">
              <label class="text-ink-dim">{{ $t("modelTemplate.openaiReasoningPro") }}</label>
              <label class="flex items-center gap-2">
                <input v-model="draft.openaiReasoningPro" type="checkbox" />
                <span class="text-ink-dim">
                  {{ $t("modelTemplate.openaiReasoningProHint") }}
                </span>
              </label>
            </template>

            <!--
              パッシブな固有スキル（設定を持たず、そのワイヤなら常に効く）。
              **チェックボックスは置かない** — 操作できないものを操作の形で
              出すのは「押しても効かないチェックを見せない」の裏返しの嘘になる。
              無効化したチェックも同じで、何かすれば有効にできると読める。
              バッジの見た目は会話ペインの「外部」と同じものを使う（同じ規律を
              2 箇所で持たない）。
            -->
            <template v-for="key in skills.passive" :key="key">
              <label class="text-ink-dim">{{ $t(`modelTemplate.${key}`) }}</label>
              <div class="flex items-start gap-2">
                <span
                  class="mt-px shrink-0 rounded-sm bg-surface-2 px-1 text-[9px] text-ink-dim ring-1 ring-line"
                >
                  {{ $t("modelTemplate.passiveBadge") }}
                </span>
                <span class="text-ink-dim">{{ $t(`modelTemplate.${key}Hint`) }}</span>
              </div>
            </template>

            <!--
              検索は入力トークンを桁で増やす（実測 98,213 / うちキャッシュ 62,720）。
              天井（Spec 11）の小さい村では 1 回で尽きうるので、押す前に言う。
            -->
            <template v-if="skills.xaiWeb.offered || skills.xaiX.offered">
              <div class="col-span-2 text-[11px] text-ink-dim">
                {{ $t("modelTemplate.xaiSearchCost") }}
              </div>
            </template>

            <!--
              **2 段で書く**（Spec 34 D5 の実測）。初回 4,454 / 2 回目以降は
              キャッシュに乗って約 484。「桁で増える」だけ書くと毎回 4.4k 払うと
              読める。検索させない問いでも掛かる固定費なので、押す前に言う。
            -->
            <template v-if="skills.openaiWeb.offered">
              <div class="col-span-2 text-[11px] text-ink-dim">
                {{ $t("modelTemplate.openaiSearchCost") }}
              </div>
            </template>

            <!--
              Meta の web 検索（Spec 37 D3）。**トグル 1 つ**に閉じる —
              `search_context_size` という軸は実在する（実測 200）が、
              `filters` / `max_results` は 400 で名指しされる。**受理集合が
              列挙されないワイヤ**なので、器を増やす前に 1 つずつ確かめる。
            -->
            <template v-if="skills.metaWeb.offered">
              <label class="text-ink-dim">{{ $t("modelTemplate.metaWebSearch") }}</label>
              <label class="flex items-center gap-2">
                <input v-model="draft.metaWebSearch" type="checkbox" />
                <span class="text-ink-dim">
                  {{ $t("modelTemplate.metaWebSearchHint") }}
                </span>
              </label>
            </template>

            <!--
              このワイヤの代償と性質を、選ぶ場所で言う（Spec 37 D7 の a）。
              - **添付は接続先に恒久保存される**（`/v1/files` / `expires_at: null`）。
                「学習に使われます」とは書かない — あれは規約で、こちらは
                観測できる事実。**性質の違う 2 つを 1 文に混ぜると、どちらも
                確かめられなくなる**
              - 強制ツール呼び出しを持てない（`tool_choice` は auto のみ）
              - 検索は入力を桁で膨らませる（実測 66,350）
            -->
            <template v-if="skills.metaWeb.offered">
              <div class="col-span-2 text-[11px] text-ink-dim">
                {{ $t("modelTemplate.metaResponsesCaveats") }}
              </div>
            </template>

            <!--
              Perplexity の固有スキル 4 本（Spec 45 D3）。**別トグル** —
              別ツール・別課金・別 output 種別（xAI が 2 つに割れたのと同じ根拠。
              1 つに畳むと web だけ欲しい村が人物検索の課金面まで開ける）。
              finance を ON にするとコアが max_steps: 5 を対で送る（D4 —
              送らないと 200 のまま黙って空振りする実測）。
            -->
            <template v-if="skills.perplexityWeb.offered">
              <label class="text-ink-dim">{{ $t("modelTemplate.perplexityWebSearch") }}</label>
              <label class="flex items-center gap-2">
                <input v-model="draft.perplexityWebSearch" type="checkbox" />
                <span class="text-ink-dim">
                  {{ $t("modelTemplate.perplexityWebSearchHint") }}
                </span>
              </label>
            </template>

            <template v-if="skills.perplexityFinance.offered">
              <label class="text-ink-dim">{{ $t("modelTemplate.perplexityFinanceSearch") }}</label>
              <label class="flex items-center gap-2">
                <input v-model="draft.perplexityFinanceSearch" type="checkbox" />
                <span class="text-ink-dim">
                  {{ $t("modelTemplate.perplexityFinanceSearchHint") }}
                </span>
              </label>
            </template>

            <template v-if="skills.perplexityPeople.offered">
              <label class="text-ink-dim">{{ $t("modelTemplate.perplexityPeopleSearch") }}</label>
              <label class="flex items-center gap-2">
                <input v-model="draft.perplexityPeopleSearch" type="checkbox" />
                <span class="text-ink-dim">
                  {{ $t("modelTemplate.perplexityPeopleSearchHint") }}
                </span>
              </label>
            </template>

            <template v-if="skills.perplexityFetch.offered">
              <label class="text-ink-dim">{{ $t("modelTemplate.perplexityFetchUrl") }}</label>
              <label class="flex items-center gap-2">
                <input v-model="draft.perplexityFetchUrl" type="checkbox" />
                <span class="text-ink-dim">
                  {{ $t("modelTemplate.perplexityFetchUrlHint") }}
                </span>
              </label>
            </template>

            <!-- 課金の注意（Spec 45 D3。押す前に言う — xAI / OpenAI の前例と同じ棚）。 -->
            <template v-if="skills.perplexityWeb.offered">
              <div class="col-span-2 text-[11px] text-ink-dim">
                {{ $t("modelTemplate.perplexityToolsCost") }}
              </div>
            </template>

            <!--
              相乗り（open_ai_responses）から切り替えた直後の案内（Spec 45 D3）。
              フラグは自動で写さない（切り替えただけで課金面が開く経路を作らない）
              ので、残った openaiWebSearch を名指しして入れ直し先を示す。
              **表示条件だけで書けるので「1 回だけ警告」のような状態の記憶は
              持たない。** stranded の警告（下）と対で出る — あちらは残った設定の
              説明、こちらは新ワイヤ側の直し方。

              **perplexityWebSearch を入れたら消える**（2026-08-27 利用者指摘 —
              P4 の実機で「入れ直してください」が入れ直した後も出続けていた）。
              入れ直しの案内は、入れ直しが済んだ瞬間に嘘ではないが騒音になる。
            -->
            <template
              v-if="
                skills.perplexityWeb.offered &&
                draft.openaiWebSearch &&
                !draft.perplexityWebSearch
              "
            >
              <div class="col-span-2 text-[11px] text-warn">
                {{ $t("modelTemplate.perplexitySwitchNote") }}
              </div>
            </template>

            <!--
              このワイヤの代償を 2 つ、選ぶ場所で言う。
              - 温度: gpt-5.6 系は 400 で拒むので**型に欄が無い**（D11）。
                黙って落とすと「設定したのに効かない」になり、送って 400 に
                するより悪い（どちらも効かないが、後者は理由が読める）
              - 画像: ネイティブ経路は画像を運ばない（Spec 23 D8 の据え置き）。
                note_dropped_attachment はワイヤの能力を見ていないので、
                **切り替えると黙って落ちる**（D8）
            -->
            <template v-if="skills.openaiWeb.offered || skills.openaiPro.offered">
              <div class="col-span-2 text-[11px] text-ink-dim">
                {{ $t("modelTemplate.openaiResponsesCaveats") }}
              </div>
            </template>

            <!--
              効かない設定が残っている状態。隠すだけだと真のまま見えなくなるので、
              その時だけ理由と直し方を出す。スキルごとに 1 行 — まとめて 1 つの
              警告にすると、どれを直せばよいかが読めなくなる。
              ラベルの（ワイヤ名）と「オフにする」の理由は strandedRows の doc が正。
            -->
            <template v-for="row in strandedRows" :key="row.key">
              <label class="text-warn"
                >{{ $t(row.labelKey) }}（{{ $t(row.ownerKey) }}）</label
              >
              <div class="text-warn">
                <p>
                  {{ $t("modelTemplate.strandedBefore")
                  }}<strong>{{ $t(row.strongKey) }}</strong
                  >{{ $t(row.afterKey) }}{{ $t("modelTemplate.strandedOrClear") }}
                </p>
                <button
                  type="button"
                  class="mt-1 rounded border border-line px-2 py-0.5 text-[11px] text-ink hover:border-accent"
                  @click="row.clear()"
                >
                  {{ $t("modelTemplate.strandedTurnOff") }}
                </button>
              </div>
            </template>
          </div>

          <p class="rounded border border-line bg-surface-0 p-2 text-[11px] text-ink-dim">
            {{ $t("modelTemplate.keyStoreBefore")
            }}<strong class="text-ink">{{ $t("modelTemplate.keyStoreStrong") }}</strong
            >{{ $t("modelTemplate.keyStoreAfter") }}
          </p>
        </div>

        <div v-else class="flex-1 p-6 text-center text-[11px] text-ink-dim">
          {{ $t("modelTemplate.noSelection") }}
        </div>

        <div v-if="draft" class="flex justify-end gap-2 border-t border-line px-4 py-2.5">
          <button
            class="rounded px-2 py-1 text-[11px] text-ink-dim hover:text-ink"
            @click="draft = null"
          >
            {{ $t("modelTemplate.cancel") }}
          </button>
          <!-- 保存の合図はボタン自身に出す。視線は押した場所にあるので、
               離れたトーストより確実に届く。成功時だけ緑 + ✓ を 1.6 秒。 -->
          <button
            class="rounded px-3 py-1 text-[11px] font-medium text-surface-0 transition-colors disabled:opacity-60"
            :class="savedFlash ? 'bg-run' : 'bg-accent'"
            :disabled="saving"
            @click="save"
          >
            {{
              saving
                ? $t("modelTemplate.saving")
                : savedFlash
                  ? $t("modelTemplate.saved")
                  : $t("modelTemplate.save")
            }}
          </button>
        </div>
      </div>
    </div>
  </div>
</template>
