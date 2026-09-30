<script setup lang="ts">
/**
 * 判断役の編集ダイアログ（Spec 62 D10）。入口は左ペインの「判断特化」と地図のノード。
 *
 * - 本文は `judges/<id>/judge.toml` をそのまま編集する（問いと規則の正本はファイル）
 * - **保存の検査はコアが行う**。落ちたら `INVALID_JUDGE_FILE` の場所と理由をここに出し、
 *   ファイルは書かれない
 * - **「試す」は保存前の編集中の本文で走らせる**。配送はしない。押したときだけ Jev へ
 *   送る（開いただけでは 1 バイトも出ない）。問いの文面の一文で確度が動く（Notes 1）ので、
 *   書き換えたらその場で確かめられないと調整できない
 * - 選択状態（`selectedAgentId`）は変えない — 判断役は会話の相手ではない
 */
import { computed, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";

import AssistPanel from "./AssistPanel.vue";
import CodeEditor from "./CodeEditor.vue";
import { askConfirm } from "../composables/useConfirm";
import { useOrchestrator } from "../composables/useOrchestrator";
import { formatError } from "../lib/errorText";
import * as ipc from "../lib/ipc";
import { judgeStatusText } from "../lib/judges";
import type { AgentId, ErrorPayload, JudgeTrial } from "../types";

const props = defineProps<{ judgeId: AgentId }>();
const emit = defineEmits<{ (e: "close"): void }>();

const { t } = useI18n();
const orchestrator = useOrchestrator();
const { state } = orchestrator;

const judge = computed(() => state.judges.find((j) => j.id === props.judgeId) ?? null);

// 判断役が消えたら（別の経路で削除された等）閉じる。存在しない id の編集は保存できない。
watch(judge, (current) => {
  if (!current && !loading.value) emit("close");
});

const text = ref("");
/** 最後に読んだ / 保存した本文。dirty 判定の基準。 */
const saved = ref("");
const loading = ref(true);
const loadError = ref("");
const busy = ref(false);
/** 保存が検査に落ちた理由（`INVALID_JUDGE_FILE` の場所と文）。 */
const saveError = ref("");

const dirty = computed(() => text.value !== saved.value);

/** 「AI で作成」（Spec 63）。下書きは `text` へ流し込むだけで、保存は下の既存の保存ボタン。 */
const assistOpen = ref(false);

onMounted(async () => {
  try {
    const body = await ipc.readJudgeFile(props.judgeId);
    text.value = body;
    saved.value = body;
  } catch (error) {
    // 読めないまま空で保存させると、既存の規則を消しうる。
    loadError.value = formatError(error as ErrorPayload);
  } finally {
    loading.value = false;
  }
});

/**
 * 保存できるか。**保存ボタンと `Ctrl+S` が同じ述語を見る**（`editorSaveWiring.test.ts`）。
 */
const canSave = computed(() => !loading.value && !loadError.value && !busy.value && dirty.value);

function saveFromEditor(): void {
  if (canSave.value) void save();
}

async function save(): Promise<void> {
  busy.value = true;
  saveError.value = "";
  try {
    await ipc.saveJudgeFile(props.judgeId, text.value);
    saved.value = text.value;
    // 状態（有効かどうか・行き先）はコアの TopologyChanged で取り直される。
  } catch (error) {
    saveError.value = formatError(error as ErrorPayload);
  } finally {
    busy.value = false;
  }
}

// ---- 表示名 ----------------------------------------------------------------

const nameDraft = ref(judge.value?.name ?? "");
watch(
  () => judge.value?.name,
  (name) => {
    if (name !== undefined) nameDraft.value = name;
  },
);

async function commitName(): Promise<void> {
  const current = judge.value;
  const name = nameDraft.value.trim();
  if (!current || !name || name === current.name) {
    if (current) nameDraft.value = current.name;
    return;
  }
  const ok = await orchestrator.updateJudge({ id: current.id, name, order: current.order, enabled: current.enabled });
  if (!ok) nameDraft.value = current.name;
}

// ---- 試す ------------------------------------------------------------------

const sample = ref("");
const trying = ref(false);
const trial = ref<JudgeTrial | null>(null);
const trialError = ref("");

async function runTrial(): Promise<void> {
  if (!sample.value.trim() || trying.value) return;
  trying.value = true;
  trial.value = null;
  trialError.value = "";
  try {
    trial.value = await ipc.tryJudge(text.value, sample.value);
  } catch (error) {
    trialError.value = formatError(error as ErrorPayload);
  } finally {
    trying.value = false;
  }
}

/** 行き先の表示名（サーヴァントの一覧から引く。引けなければ id のまま）。 */
function targetName(id: AgentId): string {
  const agent = state.agents.find((a) => a.id === id);
  return agent ? `${agent.name}（${id}）` : id;
}

/** 当たった規則の表示。 */
const trialOutcome = computed(() => {
  const r = trial.value;
  if (!r) return "";
  if (r.undecided) return t("judges.trial.undecided", { reason: r.undecided });
  const where = r.otherwise ? t("judges.trial.otherwise") : t("judges.trial.rule", { n: r.rule ?? 0 });
  const dest = r.to?.length
    ? t("judges.trial.to", { targets: r.to.map(targetName).join(", ") })
    : t("judges.trial.return");
  return `${where} → ${dest}`;
});

// ---- 削除・閉じる ----------------------------------------------------------

async function remove(): Promise<void> {
  const current = judge.value;
  if (!current) return;
  const ok = await askConfirm({
    title: t("judges.deleteTitle", { name: current.name }),
    message: t("judges.deleteMessage"),
    confirmLabel: t("judges.delete"),
    danger: true,
  });
  if (!ok) return;
  if (await orchestrator.deleteJudge(current.id)) emit("close");
}

async function requestClose(): Promise<void> {
  if (
    dirty.value &&
    !(await askConfirm({
      title: t("judges.discardCloseTitle"),
      message: t("judges.discardCloseMessage"),
      confirmLabel: t("judges.discardCloseConfirm"),
      cancelLabel: t("judges.keepEditing"),
      danger: true,
    }))
  ) {
    return;
  }
  emit("close");
}

const statusView = computed(() => {
  const current = judge.value;
  if (!current) return null;
  const view = judgeStatusText(current.status);
  return { active: view.active, label: t(view.key, view.params) };
});
</script>

<template>
  <div
    class="fixed inset-0 z-40 flex items-center justify-center bg-scrim"
    @click.self="requestClose"
  >
    <div
      class="flex h-[720px] max-h-[92vh] w-[760px] max-w-[94vw] flex-col overflow-hidden rounded-lg border border-line bg-surface-1 shadow-2xl"
      data-judge-dialog
    >
      <header class="flex shrink-0 items-center gap-2 border-b border-line px-3 py-2.5 text-xs">
        <svg class="size-3.5 shrink-0 text-accent" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round" aria-hidden="true">
          <path d="M12 3 21 12 12 21 3 12Z" />
        </svg>
        <input
          v-model="nameDraft"
          class="min-w-0 flex-1 rounded border border-transparent bg-transparent px-1 py-0.5 font-semibold outline-none hover:border-line focus:border-accent"
          :aria-label="$t('judges.nameLabel')"
          @keydown.enter.prevent="($event.target as HTMLInputElement).blur()"
          @blur="commitName"
        />
        <span class="font-mono text-[11px] text-ink-dim">{{ judgeId }}</span>
        <button
          class="rounded border border-line px-2 py-1 text-ink-dim hover:border-fail hover:text-fail"
          @click="remove"
        >
          {{ $t("judges.delete") }}
        </button>
        <button class="px-1 text-ink-dim hover:text-ink" :aria-label="$t('common.close')" @click="requestClose">✕</button>
      </header>

      <p class="shrink-0 border-b border-line bg-surface-0 px-3 py-2 text-[11px] text-ink-dim">
        {{ $t("judges.description") }}
        <span
          v-if="statusView"
          class="mt-1 block"
          :class="statusView.active ? 'text-run' : 'text-warn'"
        >
          {{ statusView.label }}
        </span>
      </p>

      <div class="min-h-0 flex-1 overflow-y-auto p-3">
        <p v-if="loading" class="py-8 text-center text-[11px] text-ink-dim">{{ $t("judges.loading") }}</p>
        <p v-else-if="loadError" class="selectable py-8 text-center text-[11px] text-fail">{{ loadError }}</p>

        <template v-else>
          <div class="mb-1 flex items-center gap-2 text-[11px]">
            <span class="font-mono text-ink-dim">judge.toml</span>
            <span v-if="dirty" class="ml-auto text-warn">{{ $t("judges.unsaved") }}</span>
            <button
              class="rounded border border-line px-2 py-0.5 text-ink-dim hover:border-accent hover:text-accent"
              :class="dirty ? 'ml-2' : 'ml-auto'"
              data-assist-open
              @click="assistOpen = true"
            >
              {{ $t("assist.open") }}
            </button>
          </div>
          <CodeEditor v-model="text" class="h-72" language="plain" @save="saveFromEditor" />
          <p v-if="saveError" class="selectable mt-1.5 text-[11px] text-fail">{{ saveError }}</p>

          <!-- 試す（配送はしない。押したときだけ Jev へ送る）。 -->
          <h3 class="mb-1 mt-4 text-[11px] font-semibold text-ink-dim">{{ $t("judges.trial.heading") }}</h3>
          <p class="mb-1.5 text-[11px] text-ink-dim">{{ $t("judges.trial.help") }}</p>
          <textarea
            v-model="sample"
            rows="3"
            class="w-full resize-y rounded border border-line bg-surface-0 px-2 py-1.5 text-[12px] outline-none focus:border-accent"
            :placeholder="$t('judges.trial.placeholder')"
          />
          <div class="mt-1.5 flex items-center gap-2">
            <button
              class="rounded border border-line px-2 py-1 text-[11px] hover:border-accent hover:text-accent disabled:opacity-40"
              :disabled="!sample.trim() || trying || loading || !!loadError"
              @click="runTrial"
            >
              {{ trying ? $t("judges.trial.running") : $t("judges.trial.run") }}
            </button>
          </div>
          <p v-if="trialError" class="selectable mt-1.5 text-[11px] text-fail">{{ trialError }}</p>
          <div v-if="trial" class="mt-2 rounded border border-line bg-surface-0 p-2 text-[11px]" data-judge-trial>
            <p class="font-medium text-ink">{{ trialOutcome }}</p>
            <table class="mt-1.5 w-full border-collapse font-mono">
              <tbody>
                <tr v-for="(value, name) in trial.values" :key="name">
                  <td class="pr-3 align-top text-ink-dim">{{ name }}</td>
                  <td class="text-ink">{{ value }}</td>
                </tr>
              </tbody>
            </table>
            <p class="mt-1.5 text-ink-dim">
              {{ $t("judges.trial.meta", { model: trial.model, tokens: trial.inputTokens }) }}
            </p>
          </div>
        </template>
      </div>

      <div class="flex shrink-0 items-center gap-2 border-t border-line px-3 py-2.5">
        <span class="text-[11px] text-ink-dim">{{ $t("judges.saveNote") }}</span>
        <button
          class="ml-auto rounded bg-accent px-3 py-1 text-[11px] font-medium text-surface-0 disabled:opacity-40"
          :disabled="!canSave"
          @click="save"
        >
          {{ busy ? $t("judges.saving") : $t("judges.save") }}
        </button>
      </div>
    </div>

    <AssistPanel
      v-if="assistOpen"
      :target="{ kind: 'judge', id: judgeId }"
      :current="text"
      @apply="text = $event"
      @close="assistOpen = false"
    />
  </div>
</template>
