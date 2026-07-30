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

import * as ipc from "../lib/ipc";
import { useOrchestrator } from "../composables/useOrchestrator";
import { WEEKDAY_LABELS, type Recurrence, type ScheduleView, type Weekday } from "../types";

const emit = defineEmits<{ (e: "close"): void }>();

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

const agents = computed(() => state.agents);

/** フォームが送信できる状態か。数値の範囲は Rust 側でも検証される（二重化）。 */
const formValid = computed(() => {
  if (!formTo.value || !formMessage.value.trim()) return false;
  if (formKind.value === "interval") return formEveryMinutes.value >= 1;
  return (
    formHour.value >= 0 &&
    formHour.value <= 23 &&
    formMinute.value >= 0 &&
    formMinute.value <= 59
  );
});

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
    error.value = `[${payload.code}] ${payload.message}`;
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
    await ipc.createSchedule(formTo.value, formMessage.value.trim(), buildRecurrence());
    formMessage.value = "";
    await load();
  } catch (e) {
    const payload = ipc.toErrorPayload(e);
    error.value = `[${payload.code}] ${payload.message}`;
  } finally {
    busy.value = false;
  }
}

async function remove(task: ScheduleView): Promise<void> {
  // 会話ログのリセットと同じ流儀: 復元できない操作は確認を挟む。
  if (!confirm(`この予定を削除しますか？\n${task.recurrenceLabel} → ${agentLabel(task.to)}`)) {
    return;
  }
  busy.value = true;
  error.value = "";
  try {
    await ipc.deleteSchedule(task.id);
    await load();
  } catch (e) {
    const payload = ipc.toErrorPayload(e);
    error.value = `[${payload.code}] ${payload.message}`;
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
    error.value = `[${payload.code}] ${payload.message}`;
  } finally {
    busy.value = false;
  }
}

// ---- 表示 ----------------------------------------------------------------------

/** 宛先の表示。id と表示名の併記（Spec 06 の規律 — 番号がずれた村で効いた）。 */
function agentLabel(id: string): string {
  const agent = state.agents.find((a) => a.id === id);
  return agent ? `${id}（${agent.name}）` : id;
}

function formatNextDue(task: ScheduleView): string {
  if (!task.enabled) return "停止中";
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
    class="fixed inset-0 z-40 flex items-center justify-center bg-black/60"
    @click.self="emit('close')"
  >
    <div
      class="flex h-[640px] w-[760px] flex-col overflow-hidden rounded-lg border border-line bg-surface-1 shadow-2xl"
    >
      <header class="flex shrink-0 items-center gap-2 border-b border-line px-3 py-2.5 text-xs">
        <h2 class="flex-1 font-semibold">予定（スケジュール実行）</h2>
        <button class="px-1 text-ink-dim hover:text-ink" @click="emit('close')">✕</button>
      </header>

      <!-- 限界の告知（P2）。これを書かずに「毎週木曜 17 時」と名乗るのは嘘になる。 -->
      <p class="shrink-0 border-b border-line bg-surface-0 px-3 py-2 text-[11px] text-ink-dim">
        決まった時刻に、決まった依頼をエージェントへ届けます。結果は会話ペインに出ます。
        <strong class="text-warn">アプリを起動していない間、予定は動きません。</strong>
        過ぎた予定はさかのぼって実行されません（間隔の予定は再開後に 1 回だけ走ります）。
      </p>

      <div class="min-h-0 flex-1 overflow-y-auto p-3">
        <p v-if="loading" class="py-8 text-center text-[11px] text-ink-dim">読み込み中…</p>

        <template v-else>
          <p v-if="error" class="selectable mb-2 rounded border border-fail/50 bg-surface-0 p-2 text-[11px] text-fail">
            {{ error }}
          </p>

          <!-- 一覧 -->
          <h3 class="mb-1 text-[11px] font-semibold text-ink-dim">登録済みの予定</h3>
          <p v-if="!schedules.length" class="rounded border border-line bg-surface-0 p-3 text-[11px] text-ink-dim">
            予定はまだありません。下のフォームから「どのエージェントへ・どんな依頼を・いつ」を
            決めて追加してください。
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
                <span class="ml-auto text-ink-dim" :title="task.enabled ? '次回の発火予定' : '一時停止中'">
                  次回: {{ formatNextDue(task) }}
                </span>
              </div>
              <p class="mt-1 truncate text-ink-dim" :title="task.message">
                {{ task.message }}
              </p>
              <div class="mt-1.5 flex items-center gap-2">
                <button
                  class="rounded border border-line px-2 py-0.5 hover:border-accent hover:text-accent disabled:opacity-40"
                  :disabled="busy"
                  @click="toggleEnabled(task)"
                >
                  {{ task.enabled ? "一時停止" : "再開" }}
                </button>
                <button
                  class="rounded border border-line px-2 py-0.5 text-fail hover:border-fail disabled:opacity-40"
                  :disabled="busy"
                  @click="remove(task)"
                >
                  削除
                </button>
              </div>
            </li>
          </ul>

          <!-- 追加フォーム -->
          <h3 class="mt-4 mb-1 text-[11px] font-semibold text-ink-dim">予定を追加</h3>
          <p v-if="!agents.length" class="rounded border border-line bg-surface-0 p-3 text-[11px] text-ink-dim">
            エージェントが登録されていないため、宛先を選べません。
            先に左ペインでエージェントを作成してください。
          </p>
          <div v-else class="space-y-2 rounded border border-line bg-surface-0 p-3 text-[11px]">
            <label class="flex items-center gap-2">
              <span class="w-14 shrink-0 text-ink-dim">宛先</span>
              <select
                v-model="formTo"
                class="flex-1 rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent"
              >
                <option value="" disabled>エージェントを選ぶ</option>
                <option v-for="agent in agents" :key="agent.id" :value="agent.id">
                  {{ agent.id }}（{{ agent.name }}）
                </option>
              </select>
            </label>

            <label class="flex items-start gap-2">
              <span class="w-14 shrink-0 pt-1 text-ink-dim">依頼</span>
              <textarea
                v-model="formMessage"
                rows="2"
                placeholder="例: 今日の予定をまとめて"
                class="flex-1 resize-none rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent"
              />
            </label>

            <div class="flex items-center gap-2">
              <span class="w-14 shrink-0 text-ink-dim">繰り返し</span>
              <label class="flex items-center gap-1">
                <input v-model="formKind" type="radio" value="weekly" /> 毎週
              </label>
              <label class="flex items-center gap-1">
                <input v-model="formKind" type="radio" value="daily" /> 毎日
              </label>
              <label class="flex items-center gap-1">
                <input v-model="formKind" type="radio" value="interval" /> 分ごと
              </label>
            </div>

            <div class="flex items-center gap-2 pl-16">
              <template v-if="formKind === 'weekly'">
                <select
                  v-model="formWeekday"
                  class="rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent"
                >
                  <option v-for="(label, value) in WEEKDAY_LABELS" :key="value" :value="value">
                    {{ label }}
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
                <span class="text-ink-dim">時</span>
                <input
                  v-model.number="formMinute"
                  type="number"
                  min="0"
                  max="59"
                  class="w-14 rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent"
                />
                <span class="text-ink-dim">分</span>
              </template>
              <template v-else>
                <input
                  v-model.number="formEveryMinutes"
                  type="number"
                  min="1"
                  class="w-20 rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent"
                />
                <span class="text-ink-dim">分ごと</span>
              </template>
            </div>

            <div class="flex justify-end">
              <button
                class="rounded bg-accent px-3 py-1 font-medium text-surface-0 disabled:opacity-40"
                :disabled="!formValid || busy"
                @click="add"
              >
                {{ busy ? "追加中…" : "追加" }}
              </button>
            </div>
          </div>
        </template>
      </div>
    </div>
  </div>
</template>
