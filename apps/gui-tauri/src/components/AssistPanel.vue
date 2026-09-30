<script setup lang="ts">
/**
 * AI 下書き補助のパネル（Spec 63）。SKILL / Construct（`MarkdownEditor`）と判断役（`JudgeDialog`）から開く。
 *
 * - 生成役のテンプレートを選び、ヒアリング → 下書きまで進める
 * - **下書きは編集中の本文へ流し込むだけ**（`apply` を出す）。保存は開いた側の既存の保存ボタン
 * - **`appended` の中身は読まない**（D3）— コアが作ったメッセージを逐語で保持して送り返すだけ。
 *   画面の会話は、利用者の入力と返り値の `text` から組む
 * - 会話は使い捨て（閉じると消える。会話があれば閉じる前に確認）
 */
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";

import { askConfirm } from "../composables/useConfirm";
import { useOrchestrator } from "../composables/useOrchestrator";
import {
  applyNeedsConfirm,
  draftDelta,
  pickTemplate,
  readRemembered,
  templateChoices,
  writeRemembered,
} from "../lib/assist";
import { formatError } from "../lib/errorText";
import { assistDraft, toErrorPayload } from "../lib/ipc";
import type { AssistMessage, AssistReply, AssistTarget, ErrorPayload } from "../types";

const props = defineProps<{
  target: AssistTarget;
  /** 編集中の本文（開いた側が持つ。未保存の変更を含む）。 */
  current: string;
}>();
const emit = defineEmits<{
  (e: "apply", content: string): void;
  (e: "close"): void;
}>();

const { t } = useI18n();
const { state } = useOrchestrator();

function storage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

const choices = computed(() => templateChoices(state.templates));
const templateId = ref(pickTemplate(state.templates, readRemembered(storage())));

function selectTemplate(id: string): void {
  templateId.value = id;
  writeRemembered(storage(), id);
}

/** コアへ送り返す履歴（不透明。中身を読まない）。 */
const history = ref<AssistMessage[]>([]);
/** 画面の会話（利用者の入力と、返り値の `text` から組む）。 */
const transcript = ref<{ from: "user" | "generator"; text: string }[]>([]);
const input = ref("");
const busy = ref(false);
const error = ref<ErrorPayload | null>(null);
const draft = ref<Extract<AssistReply, { type: "draft" }> | null>(null);
/** 反映の直前の本文（取り消しの 1 段）。 */
const previous = ref<string | null>(null);
/** 閉じた後に返ってきた結果を捨てるための番号。 */
let seq = 0;

const canSend = computed(() => !busy.value && templateId.value !== null && input.value.trim() !== "");
const canForce = computed(() => !busy.value && templateId.value !== null);

async function send(forceDraft: boolean): Promise<void> {
  if (templateId.value === null || busy.value) return;
  const text = input.value.trim();
  if (!forceDraft && text === "") return;
  busy.value = true;
  error.value = null;
  const mine = ++seq;
  transcript.value.push({ from: "user", text: forceDraft ? t("assist.forceDraftSent") : text });
  try {
    const reply = await assistDraft({
      target: props.target,
      templateId: templateId.value,
      history: history.value,
      input: forceDraft ? (text || null) : text,
      forceDraft,
      current: props.current,
    });
    if (mine !== seq) return;
    // **中身を解釈せず逐語で足す**（D3）。
    history.value = [...history.value, ...reply.appended];
    input.value = "";
    if (reply.type === "question") {
      transcript.value.push({ from: "generator", text: reply.text });
    } else {
      draft.value = reply;
      transcript.value.push({
        from: "generator",
        text: [reply.text, t("assist.draftShown", { chars: reply.draftChars })].filter(Boolean).join("\n\n"),
      });
    }
  } catch (err) {
    if (mine !== seq) return;
    // 送れなかった発話は会話から外す（コアの履歴にも入っていない）。
    transcript.value.pop();
    error.value = toErrorPayload(err);
  } finally {
    if (mine === seq) busy.value = false;
  }
}

function onKeydown(event: KeyboardEvent): void {
  if (event.isComposing) return;
  if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
    event.preventDefault();
    void send(false);
  }
}

const delta = computed(() => (draft.value ? draftDelta(props.current, draft.value.content) : null));

const validationText = computed(() => {
  const v = draft.value?.validation;
  if (!v) return null;
  return v.valid
    ? { ok: true, text: t("assist.validationOk", { attempts: draft.value?.attempts ?? 1 }) }
    : { ok: false, text: t("assist.validationNg", { location: v.location, message: v.message }) };
});

async function applyDraft(): Promise<void> {
  if (!draft.value) return;
  if (
    applyNeedsConfirm(props.current) &&
    !(await askConfirm({
      title: t("assist.applyConfirmTitle"),
      message: t("assist.applyConfirmMessage"),
      confirmLabel: t("assist.apply"),
      cancelLabel: t("assist.keep"),
    }))
  ) {
    return;
  }
  previous.value = props.current;
  emit("apply", draft.value.content);
}

function undoApply(): void {
  if (previous.value === null) return;
  emit("apply", previous.value);
  previous.value = null;
}

async function requestClose(): Promise<void> {
  if (
    transcript.value.length > 0 &&
    !(await askConfirm({
      title: t("assist.closeConfirmTitle"),
      message: t("assist.closeConfirmMessage"),
      confirmLabel: t("assist.closeConfirm"),
      cancelLabel: t("assist.keep"),
    }))
  ) {
    return;
  }
  seq += 1;
  emit("close");
}
</script>

<template>
  <div
    class="fixed inset-0 z-50 flex items-center justify-center bg-scrim"
    data-assist-panel
    @click.self="requestClose"
  >
    <div
      class="flex h-[680px] max-h-[92vh] w-[960px] max-w-[96vw] flex-col overflow-hidden rounded-lg border border-line bg-surface-1 shadow-2xl"
    >
      <header class="flex shrink-0 items-center gap-2 border-b border-line px-3 py-2.5 text-xs">
        <svg class="size-3.5 shrink-0 text-accent" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <path d="M12 3v3M12 18v3M3 12h3M18 12h3M5.6 5.6l2.1 2.1M16.3 16.3l2.1 2.1M5.6 18.4l2.1-2.1M16.3 7.7l2.1-2.1" />
        </svg>
        <span class="font-semibold">{{ t(`assist.title.${target.kind}`) }}</span>
        <label class="ml-auto flex items-center gap-1.5 text-[11px] text-ink-dim">
          {{ t("assist.templateLabel") }}
          <select
            class="rounded border border-line bg-surface-0 px-1.5 py-0.5 text-[11px] text-ink outline-none focus:border-accent"
            :value="templateId ?? ''"
            :disabled="busy"
            @change="selectTemplate(($event.target as HTMLSelectElement).value)"
          >
            <option v-if="templateId === null" value="" disabled>{{ t("assist.noTemplate") }}</option>
            <option
              v-for="c in choices"
              :key="c.id"
              :value="c.id"
              :disabled="c.disabled"
              :title="c.disabled ? t('assist.templateNoTools') : c.model"
            >
              {{ c.disabled ? `${c.name}（${t("assist.templateNoToolsShort")}）` : c.name }}
            </option>
          </select>
        </label>
        <button class="px-1 text-ink-dim hover:text-ink" :aria-label="$t('common.close')" @click="requestClose">✕</button>
      </header>

      <p class="shrink-0 border-b border-line bg-surface-0 px-3 py-2 text-[11px] text-ink-dim">
        {{ t("assist.description") }}
      </p>

      <div class="grid min-h-0 flex-1 grid-cols-2">
        <!-- 左: ヒアリング -->
        <section class="flex min-h-0 flex-col border-r border-line">
          <div class="min-h-0 flex-1 space-y-2 overflow-y-auto p-3 text-[12px]">
            <p v-if="transcript.length === 0" class="text-[11px] text-ink-dim">{{ t("assist.empty") }}</p>
            <div
              v-for="(entry, i) in transcript"
              :key="i"
              class="selectable whitespace-pre-wrap break-words rounded px-2 py-1.5"
              :class="entry.from === 'user' ? 'ml-6 bg-surface-2 text-ink' : 'mr-6 border border-line bg-surface-0 text-ink'"
            >
              <span class="mb-0.5 block text-[10px] text-ink-dim">
                {{ entry.from === "user" ? t("assist.you") : t("assist.generator") }}
              </span>
              {{ entry.text }}
            </div>
            <p v-if="busy" class="text-[11px] text-ink-dim">{{ t("assist.thinking") }}</p>
          </div>
          <p v-if="error" class="selectable shrink-0 border-t border-line px-3 py-1.5 text-[11px] text-fail">
            {{ formatError(error) }}
          </p>
          <div class="shrink-0 border-t border-line p-2">
            <textarea
              v-model="input"
              rows="3"
              class="w-full resize-none rounded border border-line bg-surface-0 px-2 py-1.5 text-[12px] outline-none focus:border-accent"
              :placeholder="t(`assist.placeholder.${target.kind}`)"
              :disabled="busy"
              @keydown="onKeydown"
            />
            <div class="mt-1.5 flex items-center gap-2">
              <span class="text-[10px] text-ink-dim">{{ t("assist.sendNote") }}</span>
              <button
                class="ml-auto rounded border border-line px-2 py-1 text-[11px] hover:border-accent hover:text-accent disabled:opacity-40"
                :disabled="!canForce"
                data-assist-force
                @click="send(true)"
              >
                {{ t("assist.forceDraft") }}
              </button>
              <button
                class="rounded bg-accent px-3 py-1 text-[11px] font-medium text-surface-0 disabled:opacity-40"
                :disabled="!canSend"
                @click="send(false)"
              >
                {{ busy ? t("assist.sending") : t("assist.send") }}
              </button>
            </div>
          </div>
        </section>

        <!-- 右: 下書き -->
        <section class="flex min-h-0 flex-col">
          <div class="flex shrink-0 items-center gap-2 border-b border-line px-3 py-1.5 text-[11px] text-ink-dim">
            {{ t("assist.draftHeading") }}
            <span v-if="delta" class="ml-auto tabular-nums" data-assist-delta>
              {{
                t("assist.delta", {
                  before: delta.beforeChars,
                  after: delta.afterChars,
                  added: delta.added,
                  removed: delta.removed,
                })
              }}
            </span>
          </div>
          <div class="min-h-0 flex-1 overflow-auto p-3">
            <p v-if="!draft" class="text-[11px] text-ink-dim">{{ t("assist.noDraft") }}</p>
            <template v-else>
              <p
                v-if="validationText"
                class="mb-2 rounded border px-2 py-1 text-[11px]"
                :class="validationText.ok ? 'border-run text-run' : 'border-fail text-fail'"
                data-assist-validation
              >
                {{ validationText.text }}
              </p>
              <pre class="selectable whitespace-pre-wrap break-words rounded border border-line/50 bg-surface-0 p-2 font-mono text-[12px] leading-relaxed text-ink">{{ draft.content }}</pre>
              <div v-if="draft.notes" class="mt-2 rounded border border-line bg-surface-0 p-2 text-[11px]">
                <span class="mb-0.5 block text-ink-dim">{{ t("assist.notes") }}</span>
                <span class="selectable whitespace-pre-wrap text-ink">{{ draft.notes }}</span>
              </div>
            </template>
          </div>
          <div class="flex shrink-0 items-center gap-2 border-t border-line px-3 py-2">
            <span class="text-[10px] text-ink-dim">{{ t("assist.applyNote") }}</span>
            <button
              class="ml-auto rounded px-2 py-1 text-[11px] text-ink-dim hover:text-ink disabled:opacity-40"
              :disabled="previous === null"
              @click="undoApply"
            >
              {{ t("assist.undo") }}
            </button>
            <button
              class="rounded bg-accent px-3 py-1 text-[11px] font-medium text-surface-0 disabled:opacity-40"
              :disabled="!draft || busy"
              data-assist-apply
              @click="applyDraft"
            >
              {{ t("assist.apply") }}
            </button>
          </div>
        </section>
      </div>
    </div>
  </div>
</template>
