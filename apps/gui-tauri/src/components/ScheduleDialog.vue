<script setup lang="ts">
/**
 * 予定（スケジュール実行）の管理ダイアログ。タイトルバーの ⏰ から開く（Spec 07）。
 *
 * **2 ペイン**（2026-08-30 利用者指示）: 左が一覧 + 「新規作成」、右が入力。
 * 一覧の予定を選ぶと右ペインへ流し込まれ、**その場で編集して保存できる**
 * （`update_schedule`）。それまでは作って消すしかなく、検収コマンドを
 * 1 文字直すにも作り直しだった。
 *
 * 下書きは**捨てられる前提**（`RoleDialog` と同じ二層）— 別の予定を選ぶと
 * 確認なしで上書きされる。dirty 確認は付けない（利用者裁定 2026-08-21 の
 * `RoleDialog` の判断をそのまま写す — 意図的な破棄の経路にまで確認が生える）。
 *
 * 再現規則は種別のラジオ + 時刻入力で、cron 式の自由入力欄は置かない
 * （読めない人には一切読めない）。
 *
 * **限界の告知が本文にある**: アプリを起動していない間、予定は動かない。
 * 書かずに「毎週木曜 17 時」と名乗るのは、できないことをできると見せる嘘になる
 * （Spec 05 で潰したのと同じ形）。
 */
import { computed, onMounted, onUnmounted, reactive, ref } from "vue";
import { useI18n } from "vue-i18n";

import * as ipc from "../lib/ipc";
import { formatError } from "../lib/errorText";
import { askConfirm } from "../composables/useConfirm";
import { useOrchestrator } from "../composables/useOrchestrator";
import {
  draftFromSchedule,
  draftValid,
  emptyDraft,
  optionsFromDraft,
  recurrenceFromDraft,
} from "../lib/scheduleDraft";
import {
  acceptanceCommandLine,
  acceptanceDisplay,
  probeCommandLine,
  probeDisplay,
} from "../lib/scheduleProbe";
import { WEEKDAY_LABEL_KEYS, type ScheduleView } from "../types";

const emit = defineEmits<{ (e: "close"): void }>();

const { t } = useI18n();
const orchestrator = useOrchestrator();
const { state } = orchestrator;

const schedules = ref<ScheduleView[]>([]);
const loading = ref(true);
const busy = ref(false);
/** 読み込み・操作の失敗。SCHEDULE_STORE_BLOCKED（ファイル破損）もここに出る。 */
const error = ref("");

// ---- 右ペインの状態 ------------------------------------------------------------

/** 右ペインの形。none = 何も選んでいない / new = 新規登録 / edit = 既存の編集。 */
const panel = ref<"none" | "new" | "edit">("none");
/** 編集中の予定の ID（`panel === "edit"` のときだけ意味を持つ）。 */
const selectedId = ref<string | null>(null);
/** フォームの下書き。変換の規則は `lib/scheduleDraft.ts`（純関数）が持つ。 */
const draft = reactive(emptyDraft());

const agents = computed(() => state.agents);
const selected = computed(
  () => schedules.value.find((task) => task.id === selectedId.value) ?? null,
);
const valid = computed(() => draftValid(draft));

/** 新規登録を始める。下書きは初期値へ戻す（前の下書きは捨てられる前提）。 */
function startNew(): void {
  selectedId.value = null;
  panel.value = "new";
  Object.assign(draft, emptyDraft());
}

/** 一覧の予定を選んで編集を始める。 */
function select(task: ScheduleView): void {
  selectedId.value = task.id;
  panel.value = "edit";
  Object.assign(draft, draftFromSchedule(task));
}

// ---- 操作 ----------------------------------------------------------------------

/** 一覧の取り直し（load と定期 pull が共有する 1 実装）。 */
async function refreshList(): Promise<void> {
  schedules.value = await ipc.listSchedules();
  // 選んでいた予定が消えていたら（外部の削除・破損）、右ペインを畳む。
  if (panel.value === "edit" && !selected.value) {
    panel.value = "none";
    selectedId.value = null;
  }
}

async function load(): Promise<void> {
  loading.value = true;
  error.value = "";
  try {
    await refreshList();
  } catch (e) {
    const payload = ipc.toErrorPayload(e);
    error.value = formatError(payload);
  } finally {
    loading.value = false;
  }
}

onMounted(load);

// 開いている間、10 秒ごとに一覧を取り直す（黒板タブと同じ pull —
// 表示中だけ・push 注入なし）。発火の結果（直近の判定・検収）はダイアログを
// 開いたまま動くので、取り直さないと「走ったのに画面が沈黙する」
// （検収 4 の実機で踏んだ）。**下書きには触れない** — 更新するのは一覧と、
// そこから引かれる右ペインの状態表示だけ。
const pullTimer = setInterval(() => {
  if (busy.value || loading.value) return;
  // 静かな取り直し。失敗は次の周期に任せる（エラー表示で入力を邪魔しない）。
  refreshList().catch(() => {});
}, 10_000);
onUnmounted(() => clearInterval(pullTimer));

async function add(): Promise<void> {
  if (!valid.value || busy.value) return;
  busy.value = true;
  error.value = "";
  try {
    const created = await ipc.createSchedule(
      draft.to,
      draft.message.trim(),
      recurrenceFromDraft(draft),
      optionsFromDraft(draft),
    );
    await load();
    // 作った予定をそのまま選んで編集モードへ — 保存された形（承認状態・
    // 次回の発火）がその場で読め、直したければそのまま保存できる。
    const stored = schedules.value.find((task) => task.id === created.id);
    if (stored) select(stored);
  } catch (e) {
    const payload = ipc.toErrorPayload(e);
    error.value = formatError(payload);
  } finally {
    busy.value = false;
  }
}

async function save(): Promise<void> {
  if (!valid.value || busy.value || !selectedId.value) return;
  busy.value = true;
  error.value = "";
  try {
    await ipc.updateSchedule(
      selectedId.value,
      draft.to,
      draft.message.trim(),
      recurrenceFromDraft(draft),
      optionsFromDraft(draft),
    );
    await load();
    // 保存された形（トリム済みの引数など）を流し込み直す。
    if (selected.value) Object.assign(draft, draftFromSchedule(selected.value));
  } catch (e) {
    const payload = ipc.toErrorPayload(e);
    error.value = formatError(payload);
  } finally {
    busy.value = false;
  }
}

async function remove(task: ScheduleView): Promise<void> {
  // 復元できない操作は確認を挟む（アプリ内のダイアログ。ブラウザの confirm は使わない）。
  const ok = await askConfirm({
    title: t("schedule.deleteConfirmTitle"),
    message: t("schedule.deleteConfirmMessage", {
      recurrence: task.recurrenceLabel,
      target: agentLabel(task.to),
    }),
    confirmLabel: t("schedule.deleteAction"),
    danger: true,
  });
  if (!ok) return;
  busy.value = true;
  error.value = "";
  try {
    await ipc.deleteSchedule(task.id);
    panel.value = "none";
    selectedId.value = null;
    await load();
  } catch (e) {
    const payload = ipc.toErrorPayload(e);
    error.value = formatError(payload);
  } finally {
    busy.value = false;
  }
}

async function toggleEnabled(task: ScheduleView): Promise<void> {
  busy.value = true;
  error.value = "";
  try {
    await ipc.setScheduleEnabled(task.id, !task.enabled);
    await load();
  } catch (e) {
    const payload = ipc.toErrorPayload(e);
    error.value = formatError(payload);
  } finally {
    busy.value = false;
  }
}

/**
 * 前判定の実行を、この端末で承認する（Spec 28 D10）。
 *
 * **押す前にコマンド行の原文を出す**（テンプレート側）。中身を見ずに押せる形に
 * すると、承認が「読まずにクリックする儀式」に落ちる。
 */
async function approveProbe(task: ScheduleView): Promise<void> {
  if (!task.probe && !task.acceptance) return;
  // **1 回の承認で前後両方に効く**（IPC 側の規律）ので、確認には両方の
  // コマンド行を出す — 片方だけ見せて両方を承認させると、読んでいない
  // ものへの同意になる。
  const lines = [probeCommandLine(task), acceptanceCommandLine(task)]
    .filter((line) => line.length > 0)
    .join("\n");
  const ok = await askConfirm({
    title: t("schedule.approveConfirmTitle"),
    message: t("schedule.approveConfirmMessage", { command: lines }),
    confirmLabel: t("schedule.approveAction"),
  });
  if (!ok) return;
  busy.value = true;
  error.value = "";
  try {
    await ipc.approveScheduleProbe(task.id);
    await load();
  } catch (e) {
    const payload = ipc.toErrorPayload(e);
    error.value = formatError(payload);
  } finally {
    busy.value = false;
  }
}

// ---- 表示 ----------------------------------------------------------------------

/**
 * 直近の判定の 1 行。
 *
 * **規則は `probeDisplay`（純関数）が持ち、ここは訳語を当てるだけ。**
 */
function lastProbeLabel(task: ScheduleView): string {
  return reportLabel(probeDisplay(task.lastProbe), "schedule.probeNeverRan");
}

/** 直近の検収の 1 行（Spec 46）。規則は `acceptanceDisplay` が持つ。 */
function lastAcceptanceLabel(task: ScheduleView): string {
  return reportLabel(acceptanceDisplay(task.lastAcceptance), "schedule.acceptanceNeverRan");
}

function reportLabel(
  display: ReturnType<typeof probeDisplay>,
  neverRanKey: string,
): string {
  if (!display) return t(neverRanKey);
  const at = new Date(display.atMs).toLocaleString("ja-JP", {
    month: "numeric",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
  const reason = display.reason === null ? "" : `（${display.reason}）`;
  return `${t(display.labelKey)}${reason} · ${at}`;
}

/** 宛先の表示。id と表示名の併記（Spec 06 の規律 — 番号がずれた村で効いた）。 */
function agentLabel(id: string): string {
  const agent = state.agents.find((a) => a.id === id);
  return agent ? t("schedule.agentLabel", { id, name: agent.name }) : id;
}

function formatNextDue(task: ScheduleView): string {
  if (!task.enabled) return t("schedule.paused");
  if (task.nextDueMs === null) return "—";
  return new Date(task.nextDueMs).toLocaleString("ja-JP", {
    month: "numeric",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}
</script>

<template>
  <div
    class="fixed inset-0 z-40 flex items-center justify-center bg-scrim"
    @click.self="emit('close')"
  >
    <div
      class="flex h-[680px] w-[1000px] flex-col overflow-hidden rounded-lg border border-line bg-surface-1 shadow-2xl"
    >
      <header class="flex shrink-0 items-center gap-2 border-b border-line px-3 py-2.5 text-xs">
        <h2 class="flex-1 font-semibold">{{ $t("schedule.title") }}</h2>
        <button class="px-1 text-ink-dim hover:text-ink" @click="emit('close')">✕</button>
      </header>

      <!-- 限界の告知（P2）。これを書かずに「毎週木曜 17 時」と名乗るのは嘘になる。 -->
      <p class="shrink-0 border-b border-line bg-surface-0 px-3 py-2 text-[11px] text-ink-dim">
        {{ $t("schedule.noticeIntro") }}
        <strong class="text-warn">{{ $t("schedule.noticeLimit") }}</strong>
        {{ $t("schedule.noticeCatchUp") }}
      </p>

      <p
        v-if="error"
        class="selectable shrink-0 border-b border-fail/50 bg-surface-0 px-3 py-2 text-[11px] text-fail"
      >
        {{ error }}
      </p>

      <div class="flex min-h-0 flex-1 text-[11px]">
        <!-- 左ペイン: 新規作成 + 一覧 -->
        <aside class="flex w-[300px] shrink-0 flex-col border-r border-line">
          <div class="shrink-0 border-b border-line p-2">
            <button
              class="w-full rounded bg-accent px-3 py-1.5 font-medium text-surface-0 hover:opacity-90"
              @click="startNew"
            >
              {{ $t("schedule.new") }}
            </button>
          </div>
          <div class="min-h-0 flex-1 overflow-y-auto p-2">
            <p v-if="loading" class="py-8 text-center text-ink-dim">
              {{ $t("schedule.loading") }}
            </p>
            <p
              v-else-if="!schedules.length"
              class="rounded border border-line bg-surface-0 p-3 text-ink-dim"
            >
              {{ $t("schedule.empty") }}
            </p>
            <ul v-else class="space-y-1.5">
              <!--
                行は button ではなく div + click（一時停止ボタンを行の中に
                置くため — button の入れ子は HTML として不正で、クリックの
                届き先も環境で割れる）。
              -->
              <li v-for="task in schedules" :key="task.id">
                <div
                  class="w-full cursor-pointer rounded border bg-surface-0 p-2 text-left"
                  :class="[
                    task.id === selectedId ? 'border-accent' : 'border-line hover:border-accent/50',
                    { 'opacity-60': !task.enabled },
                  ]"
                  @click="select(task)"
                >
                  <div class="flex items-center gap-1.5">
                    <span class="truncate font-medium text-ink">{{ agentLabel(task.to) }}</span>
                    <span class="ml-auto shrink-0 rounded bg-surface-1 px-1.5 py-0.5 text-ink-dim">
                      {{ task.recurrenceLabel }}
                    </span>
                  </div>
                  <p class="mt-1 truncate text-ink-dim" :title="task.message">
                    {{ task.message }}
                  </p>
                  <div class="mt-1 flex flex-wrap items-center gap-1.5 text-ink-dim">
                    <span>{{ $t("schedule.nextDue", { time: formatNextDue(task) }) }}</span>
                    <span v-if="task.probe" class="rounded bg-surface-1 px-1 py-0.5">
                      {{ $t("schedule.probeBadge") }}
                    </span>
                    <span v-if="task.acceptance" class="rounded bg-surface-1 px-1 py-0.5">
                      {{ $t("schedule.acceptanceBadge") }}
                    </span>
                    <!-- 未承認は一覧でも見せる — 開かないと気づけない形にしない。 -->
                    <span
                      v-if="!task.probeApproved || !task.acceptanceApproved"
                      class="rounded border border-warn/50 px-1 py-0.5 text-warn"
                    >
                      {{ $t("schedule.needsApproval") }}
                    </span>
                    <!-- @click.stop: 一時停止のつもりで選択まで動かさない。 -->
                    <button
                      class="ml-auto shrink-0 rounded border border-line px-1.5 py-0.5 hover:border-accent hover:text-accent disabled:opacity-40"
                      :disabled="busy"
                      @click.stop="toggleEnabled(task)"
                    >
                      {{ task.enabled ? $t("schedule.pause") : $t("schedule.resume") }}
                    </button>
                  </div>
                </div>
              </li>
            </ul>
          </div>
        </aside>

        <!-- 右ペイン: 入力（新規 / 編集） -->
        <section class="min-h-0 flex-1 overflow-y-auto p-3">
          <p v-if="panel === 'none'" class="py-10 text-center text-ink-dim">
            {{ $t("schedule.selectPrompt") }}
          </p>

          <template v-else>
            <h3 class="mb-2 font-semibold text-ink-dim">
              {{ panel === "new" ? $t("schedule.addHeading") : $t("schedule.editHeading") }}
            </h3>

            <!-- 編集モードだけ: 保存済みの状態（次回・直近の判定・承認）。 -->
            <div v-if="panel === 'edit' && selected" class="mb-2 space-y-1.5">
              <div class="space-y-0.5 rounded border border-line bg-surface-0 p-2 text-ink-dim">
                <div>{{ $t("schedule.nextDue", { time: formatNextDue(selected) }) }}</div>
                <div v-if="selected.probe">
                  {{ $t("schedule.probeLabel") }} {{ lastProbeLabel(selected) }}
                </div>
                <div v-if="selected.acceptance">
                  {{ $t("schedule.acceptanceLabel") }} {{ lastAcceptanceLabel(selected) }}
                </div>
              </div>
              <!--
                未承認は「動かないが理由が分からない」を防ぐための表示。
                コマンド行は承認の確認ダイアログが原文を出す。
              -->
              <div
                v-if="!selected.probeApproved || !selected.acceptanceApproved"
                class="flex items-center gap-2 rounded border border-warn/50 bg-surface-0 p-2"
              >
                <span class="flex-1 space-y-0.5 text-warn">
                  <p v-if="!selected.probeApproved">{{ $t("schedule.probeUnapproved") }}</p>
                  <p v-if="!selected.acceptanceApproved">
                    {{ $t("schedule.acceptanceUnapproved") }}
                  </p>
                </span>
                <button
                  class="shrink-0 rounded border border-warn px-2 py-0.5 text-warn hover:bg-warn hover:text-surface-0 disabled:opacity-40"
                  :disabled="busy"
                  @click="approveProbe(selected)"
                >
                  {{ $t("schedule.approve") }}
                </button>
              </div>
            </div>

            <p
              v-if="!agents.length"
              class="rounded border border-line bg-surface-0 p-3 text-ink-dim"
            >
              {{ $t("schedule.noAgents") }}
            </p>
            <div v-else class="space-y-2 rounded border border-line bg-surface-0 p-3">
              <label class="flex items-center gap-2">
                <span class="w-14 shrink-0 text-ink-dim">{{ $t("schedule.to") }}</span>
                <select
                  v-model="draft.to"
                  class="flex-1 rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent"
                >
                  <option value="" disabled>{{ $t("schedule.selectServant") }}</option>
                  <option v-for="agent in agents" :key="agent.id" :value="agent.id">
                    {{ $t("schedule.agentLabel", { id: agent.id, name: agent.name }) }}
                  </option>
                </select>
              </label>

              <label class="flex items-start gap-2">
                <span class="w-14 shrink-0 pt-1 text-ink-dim">{{ $t("schedule.request") }}</span>
                <textarea
                  v-model="draft.message"
                  rows="2"
                  :placeholder="$t('schedule.requestPlaceholder')"
                  class="flex-1 resize-none rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent"
                />
              </label>

              <div class="flex items-center gap-2">
                <span class="w-14 shrink-0 text-ink-dim">{{ $t("schedule.recurrence") }}</span>
                <label class="flex items-center gap-1">
                  <input v-model="draft.kind" type="radio" value="weekly" />
                  {{ $t("schedule.weekly") }}
                </label>
                <label class="flex items-center gap-1">
                  <input v-model="draft.kind" type="radio" value="daily" />
                  {{ $t("schedule.daily") }}
                </label>
                <label class="flex items-center gap-1">
                  <input v-model="draft.kind" type="radio" value="interval" />
                  {{ $t("schedule.interval") }}
                </label>
              </div>

              <div class="flex items-center gap-2 pl-16">
                <template v-if="draft.kind === 'weekly'">
                  <select
                    v-model="draft.weekday"
                    class="rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent"
                  >
                    <option
                      v-for="(labelKey, value) in WEEKDAY_LABEL_KEYS"
                      :key="value"
                      :value="value"
                    >
                      {{ $t(labelKey) }}
                    </option>
                  </select>
                </template>
                <template v-if="draft.kind !== 'interval'">
                  <input
                    v-model.number="draft.hour"
                    type="number"
                    min="0"
                    max="23"
                    class="w-14 rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent"
                  />
                  <span class="text-ink-dim">{{ $t("schedule.hourSuffix") }}</span>
                  <input
                    v-model.number="draft.minute"
                    type="number"
                    min="0"
                    max="59"
                    class="w-14 rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent"
                  />
                  <span class="text-ink-dim">{{ $t("schedule.minuteSuffix") }}</span>
                </template>
                <template v-else>
                  <input
                    v-model.number="draft.everyMinutes"
                    type="number"
                    min="1"
                    class="w-20 rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent"
                  />
                  <span class="text-ink-dim">{{ $t("schedule.everyMinutesSuffix") }}</span>
                </template>
              </div>

              <!-- 前判定（Spec 28）。既定は付けない = 既存の予定と同じ挙動。 -->
              <div class="border-t border-line pt-2">
                <label class="flex items-center gap-2">
                  <input v-model="draft.probeOn" type="checkbox" />
                  <span class="font-medium text-ink">{{ $t("schedule.probeToggle") }}</span>
                </label>
                <p class="mt-1 pl-6 text-ink-dim">{{ $t("schedule.probeHint") }}</p>

                <div v-if="draft.probeOn" class="mt-2 space-y-2 pl-6">
                  <label class="flex items-center gap-2">
                    <span class="w-20 shrink-0 text-ink-dim">{{ $t("schedule.probeCommand") }}</span>
                    <input
                      v-model="draft.probeCommand"
                      :placeholder="$t('schedule.probeCommandPlaceholder')"
                      class="flex-1 rounded border border-line bg-surface-1 px-2 py-1 font-mono outline-none focus:border-accent"
                    />
                  </label>

                  <label class="flex items-start gap-2">
                    <span class="w-20 shrink-0 pt-1 text-ink-dim">{{ $t("schedule.probeArgs") }}</span>
                    <!-- 1 行 1 引数の欄なので広め + 縦だけ伸ばせる（依頼文と違い
                         行の数がそのまま argv の数 — 見切れると引数が数えられない）。 -->
                    <textarea
                      v-model="draft.probeArgs"
                      rows="4"
                      :placeholder="$t('schedule.probeArgsPlaceholder')"
                      class="flex-1 resize-y rounded border border-line bg-surface-1 px-2 py-1 font-mono outline-none focus:border-accent"
                    />
                  </label>

                  <label class="flex items-center gap-2">
                    <span class="w-20 shrink-0 text-ink-dim">{{ $t("schedule.probeExpectLabel") }}</span>
                    <input
                      v-model="draft.probeExpect"
                      :placeholder="$t('schedule.probeExpectPlaceholder')"
                      class="flex-1 rounded border border-line bg-surface-1 px-2 py-1 font-mono outline-none focus:border-accent"
                    />
                  </label>
                  <p class="pl-20 text-ink-dim">{{ $t("schedule.probeExpectHint") }}</p>

                  <label class="flex items-center gap-2">
                    <span class="w-20 shrink-0 text-ink-dim">{{ $t("schedule.probeTimeout") }}</span>
                    <input
                      v-model.number="draft.probeTimeout"
                      type="number"
                      min="1"
                      max="3600"
                      class="w-20 rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent"
                    />
                    <span class="text-ink-dim">{{ $t("schedule.probeTimeoutSuffix") }}</span>
                  </label>

                  <label class="flex items-center gap-2">
                    <span class="w-20 shrink-0 text-ink-dim">{{ $t("schedule.probeCwd") }}</span>
                    <input
                      v-model="draft.probeCwd"
                      :placeholder="$t('schedule.probeCwdPlaceholder')"
                      class="flex-1 rounded border border-line bg-surface-1 px-2 py-1 font-mono outline-none focus:border-accent"
                    />
                  </label>
                </div>
              </div>

              <!-- 後判定（Spec 46）。既定は付けない = 既存の予定と同じ挙動。 -->
              <div class="border-t border-line pt-2">
                <label class="flex items-center gap-2">
                  <input v-model="draft.acceptanceOn" type="checkbox" />
                  <span class="font-medium text-ink">{{ $t("schedule.acceptanceToggle") }}</span>
                </label>
                <p class="mt-1 pl-6 text-ink-dim">{{ $t("schedule.acceptanceHint") }}</p>

                <div v-if="draft.acceptanceOn" class="mt-2 space-y-2 pl-6">
                  <label class="flex items-center gap-2">
                    <span class="w-20 shrink-0 text-ink-dim">{{ $t("schedule.probeCommand") }}</span>
                    <input
                      v-model="draft.accCommand"
                      :placeholder="$t('schedule.probeCommandPlaceholder')"
                      class="flex-1 rounded border border-line bg-surface-1 px-2 py-1 font-mono outline-none focus:border-accent"
                    />
                  </label>

                  <label class="flex items-start gap-2">
                    <span class="w-20 shrink-0 pt-1 text-ink-dim">{{ $t("schedule.probeArgs") }}</span>
                    <!-- 前判定の引数欄と同じ理由で広め + 縦だけ伸ばせる。 -->
                    <textarea
                      v-model="draft.accArgs"
                      rows="4"
                      :placeholder="$t('schedule.probeArgsPlaceholder')"
                      class="flex-1 resize-y rounded border border-line bg-surface-1 px-2 py-1 font-mono outline-none focus:border-accent"
                    />
                  </label>

                  <label class="flex items-center gap-2">
                    <span class="w-20 shrink-0 text-ink-dim">{{ $t("schedule.probeExpectLabel") }}</span>
                    <input
                      v-model="draft.accExpect"
                      :placeholder="$t('schedule.probeExpectPlaceholder')"
                      class="flex-1 rounded border border-line bg-surface-1 px-2 py-1 font-mono outline-none focus:border-accent"
                    />
                  </label>
                  <p class="pl-20 text-ink-dim">{{ $t("schedule.acceptanceExpectHint") }}</p>

                  <label class="flex items-center gap-2">
                    <span class="w-20 shrink-0 text-ink-dim">
                      {{ $t("schedule.acceptanceMaxAttempts") }}
                    </span>
                    <input
                      v-model.number="draft.accMaxAttempts"
                      type="number"
                      min="1"
                      max="5"
                      class="w-14 rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent"
                    />
                    <span class="text-ink-dim">{{ $t("schedule.acceptanceMaxAttemptsSuffix") }}</span>
                  </label>

                  <label class="flex items-center gap-2">
                    <span class="w-20 shrink-0 text-ink-dim">{{ $t("schedule.probeTimeout") }}</span>
                    <input
                      v-model.number="draft.accTimeout"
                      type="number"
                      min="1"
                      max="3600"
                      class="w-20 rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent"
                    />
                    <span class="text-ink-dim">{{ $t("schedule.probeTimeoutSuffix") }}</span>
                  </label>

                  <label class="flex items-center gap-2">
                    <span class="w-20 shrink-0 text-ink-dim">{{ $t("schedule.probeCwd") }}</span>
                    <input
                      v-model="draft.accCwd"
                      :placeholder="$t('schedule.probeCwdPlaceholder')"
                      class="flex-1 rounded border border-line bg-surface-1 px-2 py-1 font-mono outline-none focus:border-accent"
                    />
                  </label>
                </div>
              </div>

              <!-- 前後処理（Spec 28）。向きが逆なので性質を書き分ける。 -->
              <div class="space-y-1 border-t border-line pt-2">
                <label class="flex items-center gap-2">
                  <input
                    v-model="draft.sessionMode"
                    type="checkbox"
                    true-value="fresh"
                    false-value="continue"
                  />
                  <span class="font-medium text-ink">{{ $t("schedule.freshToggle") }}</span>
                </label>
                <p class="pl-6 text-ink-dim">{{ $t("schedule.freshHint") }}</p>

                <label class="flex items-center gap-2 pt-1">
                  <input v-model="draft.summarizeAfter" type="checkbox" />
                  <span class="font-medium text-ink">{{ $t("schedule.summarizeToggle") }}</span>
                </label>
                <p class="pl-6 text-ink-dim">{{ $t("schedule.summarizeHint") }}</p>
              </div>

              <div class="flex items-center gap-2 border-t border-line pt-2">
                <!-- 編集モードだけ: 一時停止と削除（対象は保存済みの側）。 -->
                <template v-if="panel === 'edit' && selected">
                  <button
                    class="rounded border border-line px-2 py-0.5 hover:border-accent hover:text-accent disabled:opacity-40"
                    :disabled="busy"
                    @click="toggleEnabled(selected)"
                  >
                    {{ selected.enabled ? $t("schedule.pause") : $t("schedule.resume") }}
                  </button>
                  <button
                    class="rounded border border-line px-2 py-0.5 text-fail hover:border-fail disabled:opacity-40"
                    :disabled="busy"
                    @click="remove(selected)"
                  >
                    {{ $t("schedule.delete") }}
                  </button>
                </template>
                <button
                  v-if="panel === 'new'"
                  class="ml-auto rounded bg-accent px-3 py-1 font-medium text-surface-0 disabled:opacity-40"
                  :disabled="!valid || busy"
                  @click="add"
                >
                  {{ busy ? $t("schedule.adding") : $t("schedule.add") }}
                </button>
                <button
                  v-else
                  class="ml-auto rounded bg-accent px-3 py-1 font-medium text-surface-0 disabled:opacity-40"
                  :disabled="!valid || busy"
                  @click="save"
                >
                  {{ busy ? $t("schedule.saving") : $t("schedule.save") }}
                </button>
              </div>
            </div>
          </template>
        </section>
      </div>
    </div>
  </div>
</template>
