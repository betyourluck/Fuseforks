<script setup lang="ts">
/**
 * 予定（スケジュール実行）の管理ダイアログ。タイトルバーの ⏰ から開く（Spec 07）。
 *
 * 一覧・追加・削除・一時停止だけの薄い画面。再現規則は種別のラジオ + 時刻入力で、
 * cron 式の自由入力欄は置かない（読めない人には一切読めない）。
 *
 * **限界の告知が本文にある**: アプリを起動していない間、予定は動かない。
 * 書かずに「毎週木曜 17 時」と名乗るのは、できないことをできると見せる嘘になる
 * （Spec 05 で潰したのと同じ形）。
 */
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";

import * as ipc from "../lib/ipc";
import { formatError } from "../lib/errorText";
import { askConfirm } from "../composables/useConfirm";
import { useOrchestrator } from "../composables/useOrchestrator";
import {
  acceptanceCommandLine,
  acceptanceDisplay,
  acceptanceFormValid,
  parseProbeArgs,
  probeCommandLine,
  probeDisplay,
  probeFormValid,
} from "../lib/scheduleProbe";
import {
  WEEKDAY_LABEL_KEYS,
  type Recurrence,
  type ScheduleOptions,
  type ScheduleView,
  type SessionMode,
  type Weekday,
} from "../types";

const emit = defineEmits<{ (e: "close"): void }>();

const { t } = useI18n();
const orchestrator = useOrchestrator();
const { state } = orchestrator;

const schedules = ref<ScheduleView[]>([]);
const loading = ref(true);
const busy = ref(false);
/** 読み込み・操作の失敗。SCHEDULE_STORE_BLOCKED（ファイル破損）もここに出る。 */
const error = ref("");

// ---- 追加フォーム -------------------------------------------------------------

const formTo = ref("");
const formMessage = ref("");
const formKind = ref<Recurrence["kind"]>("weekly");
const formWeekday = ref<Weekday>("thu");
const formHour = ref(17);
const formMinute = ref(0);
const formEveryMinutes = ref(60);

// ---- 前判定と前後処理（Spec 28） -----------------------------------------------

/** 前判定を付けるか。**既定は付けない** — 既存の予定と同じ挙動から始まる。 */
const formProbeOn = ref(false);
const formCommand = ref("");
/** 引数は **1 行 1 引数**。空白区切りにするとシェルの引用規則が要る（Spec 15 P4）。 */
const formArgs = ref("");
const formExpect = ref("");
const formTimeout = ref(60);
const formCwd = ref("");
const formSessionMode = ref<SessionMode>("continue");
const formSummarizeAfter = ref(false);

// ---- 後判定（Spec 46） ---------------------------------------------------------

/** 後判定を付けるか。**既定は付けない** — 既存の予定と同じ挙動から始まる。 */
const formAcceptanceOn = ref(false);
const formAccCommand = ref("");
const formAccArgs = ref("");
const formAccExpect = ref("");
const formAccTimeout = ref(60);
const formAccCwd = ref("");
/** 再依頼を含めた総試行回数。既定 2 = 出し直し 1 回（コア側の既定と同じ）。 */
const formAccMaxAttempts = ref(2);

const agents = computed(() => state.agents);

/** フォームが送信できる状態か。数値の範囲は Rust 側でも検証される（二重化）。 */
const formValid = computed(() => {
  if (!formTo.value || !formMessage.value.trim()) return false;
  // 前判定を付けるなら、コマンドと合図は必須（コア側も読み込みで弾く）。
  if (
    formProbeOn.value &&
    !probeFormValid({
      command: formCommand.value,
      expect: formExpect.value,
      timeoutSecs: formTimeout.value,
    })
  ) {
    return false;
  }
  // 後判定も同じ述語 + 試行回数の値域（コア側も読み込みで弾く）。
  if (
    formAcceptanceOn.value &&
    !acceptanceFormValid({
      command: formAccCommand.value,
      expect: formAccExpect.value,
      timeoutSecs: formAccTimeout.value,
      maxAttempts: formAccMaxAttempts.value,
    })
  ) {
    return false;
  }
  if (formKind.value === "interval") return formEveryMinutes.value >= 1;
  return (
    formHour.value >= 0 &&
    formHour.value <= 23 &&
    formMinute.value >= 0 &&
    formMinute.value <= 59
  );
});

/** 送信する追加指定。**既定のままの欄も送る** — 受け側が既定へ畳む。 */
function buildOptions(): ScheduleOptions {
  return {
    probe: formProbeOn.value
      ? {
          command: formCommand.value.trim(),
          args: parseProbeArgs(formArgs.value),
          expect: formExpect.value.trim(),
          timeoutSecs: Math.floor(formTimeout.value),
          cwd: formCwd.value.trim() || null,
        }
      : null,
    sessionMode: formSessionMode.value,
    summarizeAfter: formSummarizeAfter.value,
    acceptance: formAcceptanceOn.value
      ? {
          command: formAccCommand.value.trim(),
          args: parseProbeArgs(formAccArgs.value),
          expect: formAccExpect.value.trim(),
          timeoutSecs: Math.floor(formAccTimeout.value),
          cwd: formAccCwd.value.trim() || null,
          maxAttempts: Math.floor(formAccMaxAttempts.value),
        }
      : null,
  };
}

function buildRecurrence(): Recurrence {
  switch (formKind.value) {
    case "interval":
      return { kind: "interval", everyMinutes: Math.floor(formEveryMinutes.value) };
    case "daily":
      return { kind: "daily", hour: formHour.value, minute: formMinute.value };
    case "weekly":
      return {
        kind: "weekly",
        weekday: formWeekday.value,
        hour: formHour.value,
        minute: formMinute.value,
      };
  }
}

// ---- 操作 ----------------------------------------------------------------------

async function load(): Promise<void> {
  loading.value = true;
  error.value = "";
  try {
    schedules.value = await ipc.listSchedules();
  } catch (e) {
    const payload = ipc.toErrorPayload(e);
    error.value = formatError(payload);
  } finally {
    loading.value = false;
  }
}

onMounted(load);

async function add(): Promise<void> {
  if (!formValid.value || busy.value) return;
  busy.value = true;
  error.value = "";
  try {
    await ipc.createSchedule(
      formTo.value,
      formMessage.value.trim(),
      buildRecurrence(),
      buildOptions(),
    );
    formMessage.value = "";
    // **前判定の欄は残す。** 似た監視をもう 1 件足す使い方が普通で、
    // 毎回コマンドを打ち直させるのは手間を増やすだけ。
    await load();
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
      class="flex h-[640px] w-[760px] flex-col overflow-hidden rounded-lg border border-line bg-surface-1 shadow-2xl"
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

      <div class="min-h-0 flex-1 overflow-y-auto p-3">
        <p v-if="loading" class="py-8 text-center text-[11px] text-ink-dim">
          {{ $t("schedule.loading") }}
        </p>

        <template v-else>
          <p v-if="error" class="selectable mb-2 rounded border border-fail/50 bg-surface-0 p-2 text-[11px] text-fail">
            {{ error }}
          </p>

          <!-- 一覧 -->
          <h3 class="mb-1 text-[11px] font-semibold text-ink-dim">{{ $t("schedule.listHeading") }}</h3>
          <p v-if="!schedules.length" class="rounded border border-line bg-surface-0 p-3 text-[11px] text-ink-dim">
            {{ $t("schedule.empty") }}
          </p>
          <ul v-else class="space-y-2">
            <li
              v-for="task in schedules"
              :key="task.id"
              class="rounded border border-line bg-surface-0 p-2 text-[11px]"
              :class="{ 'opacity-60': !task.enabled }"
            >
              <div class="flex items-center gap-2">
                <span class="font-medium text-ink">{{ agentLabel(task.to) }}</span>
                <span class="rounded bg-surface-1 px-1.5 py-0.5 text-ink-dim">
                  {{ task.recurrenceLabel }}
                </span>
                <span
                  class="ml-auto text-ink-dim"
                  :title="task.enabled ? $t('schedule.nextDueTitle') : $t('schedule.pausedTitle')"
                >
                  {{ $t("schedule.nextDue", { time: formatNextDue(task) }) }}
                </span>
              </div>
              <p class="mt-1 truncate text-ink-dim" :title="task.message">
                {{ task.message }}
              </p>

              <!-- 前判定と前後処理（Spec 28）。付いている予定にだけ出す。 -->
              <div v-if="task.probe" class="mt-1.5 rounded border border-line bg-surface-1 p-1.5">
                <div class="flex items-center gap-1.5">
                  <span class="shrink-0 text-ink-dim">{{ $t("schedule.probeLabel") }}</span>
                  <code class="selectable truncate font-mono text-ink" :title="probeCommandLine(task)">
                    {{ probeCommandLine(task) }}
                  </code>
                </div>
                <div class="mt-1 flex items-center gap-1.5 text-ink-dim">
                  <span>{{ $t("schedule.probeExpect", { expect: task.probe.expect }) }}</span>
                  <span class="ml-auto">{{ lastProbeLabel(task) }}</span>
                </div>
                <!--
                  未承認は「動かないが理由が分からない」を防ぐための表示。
                  **コマンド行は上に出ている**ので、押す前に中身が読める。
                -->
                <div
                  v-if="!task.probeApproved"
                  class="mt-1.5 flex items-center gap-2 rounded border border-warn/50 bg-surface-0 p-1.5"
                >
                  <span class="flex-1 text-warn">{{ $t("schedule.probeUnapproved") }}</span>
                  <button
                    class="shrink-0 rounded border border-warn px-2 py-0.5 text-warn hover:bg-warn hover:text-surface-0 disabled:opacity-40"
                    :disabled="busy"
                    @click="approveProbe(task)"
                  >
                    {{ $t("schedule.approve") }}
                  </button>
                </div>
              </div>

              <!-- 後判定（Spec 46）。付いている予定にだけ出す。前判定と同じ器。 -->
              <div v-if="task.acceptance" class="mt-1.5 rounded border border-line bg-surface-1 p-1.5">
                <div class="flex items-center gap-1.5">
                  <span class="shrink-0 text-ink-dim">{{ $t("schedule.acceptanceLabel") }}</span>
                  <code class="selectable truncate font-mono text-ink" :title="acceptanceCommandLine(task)">
                    {{ acceptanceCommandLine(task) }}
                  </code>
                </div>
                <div class="mt-1 flex items-center gap-1.5 text-ink-dim">
                  <span>{{
                    $t("schedule.acceptanceMeta", {
                      expect: task.acceptance.expect,
                      max: task.acceptance.maxAttempts,
                    })
                  }}</span>
                  <span class="ml-auto">{{ lastAcceptanceLabel(task) }}</span>
                </div>
                <div
                  v-if="!task.acceptanceApproved"
                  class="mt-1.5 flex items-center gap-2 rounded border border-warn/50 bg-surface-0 p-1.5"
                >
                  <span class="flex-1 text-warn">{{ $t("schedule.acceptanceUnapproved") }}</span>
                  <button
                    class="shrink-0 rounded border border-warn px-2 py-0.5 text-warn hover:bg-warn hover:text-surface-0 disabled:opacity-40"
                    :disabled="busy"
                    @click="approveProbe(task)"
                  >
                    {{ $t("schedule.approve") }}
                  </button>
                </div>
              </div>

              <p
                v-if="task.sessionMode === 'fresh' || task.summarizeAfter"
                class="mt-1 text-ink-dim"
              >
                <span v-if="task.sessionMode === 'fresh'">{{ $t("schedule.freshBadge") }}</span>
                <span v-if="task.sessionMode === 'fresh' && task.summarizeAfter"> · </span>
                <span v-if="task.summarizeAfter">{{ $t("schedule.summarizeBadge") }}</span>
              </p>

              <div class="mt-1.5 flex items-center gap-2">
                <button
                  class="rounded border border-line px-2 py-0.5 hover:border-accent hover:text-accent disabled:opacity-40"
                  :disabled="busy"
                  @click="toggleEnabled(task)"
                >
                  {{ task.enabled ? $t("schedule.pause") : $t("schedule.resume") }}
                </button>
                <button
                  class="rounded border border-line px-2 py-0.5 text-fail hover:border-fail disabled:opacity-40"
                  :disabled="busy"
                  @click="remove(task)"
                >
                  {{ $t("schedule.delete") }}
                </button>
              </div>
            </li>
          </ul>

          <!-- 追加フォーム -->
          <h3 class="mt-4 mb-1 text-[11px] font-semibold text-ink-dim">{{ $t("schedule.addHeading") }}</h3>
          <p v-if="!agents.length" class="rounded border border-line bg-surface-0 p-3 text-[11px] text-ink-dim">
            {{ $t("schedule.noAgents") }}
          </p>
          <div v-else class="space-y-2 rounded border border-line bg-surface-0 p-3 text-[11px]">
            <label class="flex items-center gap-2">
              <span class="w-14 shrink-0 text-ink-dim">{{ $t("schedule.to") }}</span>
              <select
                v-model="formTo"
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
                v-model="formMessage"
                rows="2"
                :placeholder="$t('schedule.requestPlaceholder')"
                class="flex-1 resize-none rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent"
              />
            </label>

            <div class="flex items-center gap-2">
              <span class="w-14 shrink-0 text-ink-dim">{{ $t("schedule.recurrence") }}</span>
              <label class="flex items-center gap-1">
                <input v-model="formKind" type="radio" value="weekly" /> {{ $t("schedule.weekly") }}
              </label>
              <label class="flex items-center gap-1">
                <input v-model="formKind" type="radio" value="daily" /> {{ $t("schedule.daily") }}
              </label>
              <label class="flex items-center gap-1">
                <input v-model="formKind" type="radio" value="interval" /> {{ $t("schedule.interval") }}
              </label>
            </div>

            <div class="flex items-center gap-2 pl-16">
              <template v-if="formKind === 'weekly'">
                <select
                  v-model="formWeekday"
                  class="rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent"
                >
                  <option v-for="(labelKey, value) in WEEKDAY_LABEL_KEYS" :key="value" :value="value">
                    {{ $t(labelKey) }}
                  </option>
                </select>
              </template>
              <template v-if="formKind !== 'interval'">
                <input
                  v-model.number="formHour"
                  type="number"
                  min="0"
                  max="23"
                  class="w-14 rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent"
                />
                <span class="text-ink-dim">{{ $t("schedule.hourSuffix") }}</span>
                <input
                  v-model.number="formMinute"
                  type="number"
                  min="0"
                  max="59"
                  class="w-14 rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent"
                />
                <span class="text-ink-dim">{{ $t("schedule.minuteSuffix") }}</span>
              </template>
              <template v-else>
                <input
                  v-model.number="formEveryMinutes"
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
                <input v-model="formProbeOn" type="checkbox" />
                <span class="font-medium text-ink">{{ $t("schedule.probeToggle") }}</span>
              </label>
              <p class="mt-1 pl-6 text-ink-dim">{{ $t("schedule.probeHint") }}</p>

              <div v-if="formProbeOn" class="mt-2 space-y-2 pl-6">
                <label class="flex items-center gap-2">
                  <span class="w-20 shrink-0 text-ink-dim">{{ $t("schedule.probeCommand") }}</span>
                  <input
                    v-model="formCommand"
                    :placeholder="$t('schedule.probeCommandPlaceholder')"
                    class="flex-1 rounded border border-line bg-surface-1 px-2 py-1 font-mono outline-none focus:border-accent"
                  />
                </label>

                <label class="flex items-start gap-2">
                  <span class="w-20 shrink-0 pt-1 text-ink-dim">{{ $t("schedule.probeArgs") }}</span>
                  <textarea
                    v-model="formArgs"
                    rows="2"
                    :placeholder="$t('schedule.probeArgsPlaceholder')"
                    class="flex-1 resize-none rounded border border-line bg-surface-1 px-2 py-1 font-mono outline-none focus:border-accent"
                  />
                </label>

                <label class="flex items-center gap-2">
                  <span class="w-20 shrink-0 text-ink-dim">{{ $t("schedule.probeExpectLabel") }}</span>
                  <input
                    v-model="formExpect"
                    :placeholder="$t('schedule.probeExpectPlaceholder')"
                    class="flex-1 rounded border border-line bg-surface-1 px-2 py-1 font-mono outline-none focus:border-accent"
                  />
                </label>
                <p class="pl-20 text-ink-dim">{{ $t("schedule.probeExpectHint") }}</p>

                <label class="flex items-center gap-2">
                  <span class="w-20 shrink-0 text-ink-dim">{{ $t("schedule.probeTimeout") }}</span>
                  <input
                    v-model.number="formTimeout"
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
                    v-model="formCwd"
                    :placeholder="$t('schedule.probeCwdPlaceholder')"
                    class="flex-1 rounded border border-line bg-surface-1 px-2 py-1 font-mono outline-none focus:border-accent"
                  />
                </label>
              </div>
            </div>

            <!-- 後判定（Spec 46）。既定は付けない = 既存の予定と同じ挙動。 -->
            <div class="border-t border-line pt-2">
              <label class="flex items-center gap-2">
                <input v-model="formAcceptanceOn" type="checkbox" />
                <span class="font-medium text-ink">{{ $t("schedule.acceptanceToggle") }}</span>
              </label>
              <p class="mt-1 pl-6 text-ink-dim">{{ $t("schedule.acceptanceHint") }}</p>

              <div v-if="formAcceptanceOn" class="mt-2 space-y-2 pl-6">
                <label class="flex items-center gap-2">
                  <span class="w-20 shrink-0 text-ink-dim">{{ $t("schedule.probeCommand") }}</span>
                  <input
                    v-model="formAccCommand"
                    :placeholder="$t('schedule.probeCommandPlaceholder')"
                    class="flex-1 rounded border border-line bg-surface-1 px-2 py-1 font-mono outline-none focus:border-accent"
                  />
                </label>

                <label class="flex items-start gap-2">
                  <span class="w-20 shrink-0 pt-1 text-ink-dim">{{ $t("schedule.probeArgs") }}</span>
                  <textarea
                    v-model="formAccArgs"
                    rows="2"
                    :placeholder="$t('schedule.probeArgsPlaceholder')"
                    class="flex-1 resize-none rounded border border-line bg-surface-1 px-2 py-1 font-mono outline-none focus:border-accent"
                  />
                </label>

                <label class="flex items-center gap-2">
                  <span class="w-20 shrink-0 text-ink-dim">{{ $t("schedule.probeExpectLabel") }}</span>
                  <input
                    v-model="formAccExpect"
                    :placeholder="$t('schedule.probeExpectPlaceholder')"
                    class="flex-1 rounded border border-line bg-surface-1 px-2 py-1 font-mono outline-none focus:border-accent"
                  />
                </label>
                <p class="pl-20 text-ink-dim">{{ $t("schedule.acceptanceExpectHint") }}</p>

                <label class="flex items-center gap-2">
                  <span class="w-20 shrink-0 text-ink-dim">{{ $t("schedule.acceptanceMaxAttempts") }}</span>
                  <input
                    v-model.number="formAccMaxAttempts"
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
                    v-model.number="formAccTimeout"
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
                    v-model="formAccCwd"
                    :placeholder="$t('schedule.probeCwdPlaceholder')"
                    class="flex-1 rounded border border-line bg-surface-1 px-2 py-1 font-mono outline-none focus:border-accent"
                  />
                </label>
              </div>
            </div>

            <!-- 前後処理（Spec 28）。向きが逆なので性質を書き分ける。 -->
            <div class="space-y-1 border-t border-line pt-2">
              <label class="flex items-center gap-2">
                <input v-model="formSessionMode" type="checkbox" true-value="fresh" false-value="continue" />
                <span class="font-medium text-ink">{{ $t("schedule.freshToggle") }}</span>
              </label>
              <p class="pl-6 text-ink-dim">{{ $t("schedule.freshHint") }}</p>

              <label class="flex items-center gap-2 pt-1">
                <input v-model="formSummarizeAfter" type="checkbox" />
                <span class="font-medium text-ink">{{ $t("schedule.summarizeToggle") }}</span>
              </label>
              <p class="pl-6 text-ink-dim">{{ $t("schedule.summarizeHint") }}</p>
            </div>

            <div class="flex justify-end">
              <button
                class="rounded bg-accent px-3 py-1 font-medium text-surface-0 disabled:opacity-40"
                :disabled="!formValid || busy"
                @click="add"
              >
                {{ busy ? $t("schedule.adding") : $t("schedule.add") }}
              </button>
            </div>
          </div>
        </template>
      </div>
    </div>
  </div>
</template>
