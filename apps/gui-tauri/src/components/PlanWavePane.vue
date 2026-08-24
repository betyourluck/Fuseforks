<script setup lang="ts">
/**
 * 中央ペイン下段: 作業状況タブ（Spec 08 の波ペイン — plan 実行の可視化、
 * Airflow Grid 相当。GUI 表示名は 2026-08-02 から「作業状況」で、黒板と
 * タブ切り替え。台帳・テスト内の「波ペイン」はこの部品を指す）。
 *
 * 列 = 波（古い→新しいを左→右）、行 = エージェント、セル = タスクの解決状態。
 * 描くのは**モデルが作った計画の実行痕**。**例外は確認待ちの波（Spec 43）** —
 * `planReview` の個体の提案はここに編集パネルとして現れ、人が本文を直し・
 * 消し・足してから「実行」する（実行痕の側は今までどおり読み取り専用）。
 * ノードと辺の図は上段の TopologyMap の役割 — ここは「誰に・いつ・どうなったか」の
 * 時系列だけを持つ。
 *
 * # 行を全登録エージェントに固定する理由
 *
 * 行集合を波の内容から導出すると、波が流れるたびに行が増減して目が追えない。
 * 左ペインと同じ order 順で全員を並べ、波に登場しない行のセルは空白にする。
 *
 * # セルに所要バーを描かない理由
 *
 * 波内の並列タスクの壁時計は「キュー待ちを含む最遅 1 体分」（Spec 04 Notes 8）で、
 * バーの長さ比較は「実行が並列」という誤読を誘う。所要は数字（ツールチップ）で出す。
 */
import { computed, nextTick, ref, watch } from "vue";
import { useI18n } from "vue-i18n";

import { useOrchestrator } from "../composables/useOrchestrator";
import type {
  AgentId,
  BottomTab,
  PlanTaskInput,
  PlanTaskRecord,
  PlanTaskState,
  PlanWaveRecord,
} from "../types";
import BottomPaneTabs from "./BottomPaneTabs.vue";

defineProps<{ activeTab: BottomTab }>();

const emit = defineEmits<{ (e: "selectTab", tab: BottomTab): void }>();

const { t } = useI18n();

const orchestrator = useOrchestrator();
const { state } = orchestrator;

/** 行 = 全登録エージェント。左ペイン・一覧と同じ order 順。 */
const rows = computed(() => [...state.agents].sort((a, b) => a.order - b.order));

const waves = computed(() => state.planWaves);

/** 分類の表示語彙（辞書キー）。色の対応は data_contract.yaml の PlanTaskState が正。 */
const STATE_LABEL_KEYS: Record<PlanTaskState, string> = {
  running: "waves.state.running",
  answered: "waves.state.answered",
  handed_off: "waves.state.handed_off",
  undeliverable: "waves.state.undeliverable",
  no_answer: "waves.state.no_answer",
  timed_out: "waves.state.timed_out",
  interrupted: "waves.state.interrupted",
  budget_exhausted: "waves.state.budget_exhausted",
};

function taskFor(wave: PlanWaveRecord, agentId: AgentId): PlanTaskRecord | undefined {
  return wave.tasks.find((t) => t.to === agentId);
}

function displayName(agentId: AgentId): string {
  return state.agents.find((a) => a.id === agentId)?.name ?? agentId;
}

/** セル色。running だけがアニメーションを持つ（動いているのはそこだけ）。 */
function cellClass(taskState: PlanTaskState): string {
  switch (taskState) {
    case "running":
      return "animate-pulse bg-accent/70";
    case "answered":
      return "bg-run";
    case "handed_off":
    case "interrupted":
      // 注意色。打ち切りは人が止めさせた結果で、失敗ではない
      // （data_contract の turn_interrupt 不変条件 4 と同じ判断）。
      return "bg-warn";
    default:
      // undeliverable / no_answer / timed_out / budget_exhausted。
      // 予算切れは資源の事実なので失敗系（人の打ち切りの注意色と混ぜない —
      // data_contract の token_budget.exhaustion）。内訳はツールチップで読む。
      return "bg-fail";
  }
}

function formatMs(ms: number): string {
  return ms >= 1000 ? `${(ms / 1000).toFixed(1)}s` : `${ms}ms`;
}

/** 列見出しのツールチップ。同定は planId が持つ（表示名は一意でない）。 */
function waveTitle(wave: PlanWaveRecord): string {
  const lines = [
    `planId ${wave.planId}`,
    t("waves.startedAt", { time: new Date(wave.startedAtMs).toLocaleTimeString() }),
  ];
  if (wave.state === "pending") lines.push(t("waves.pendingBadge"));
  if (wave.state === "discarded") lines.push(t("waves.discardedBadge"));
  if (wave.elapsedMs !== null)
    lines.push(t("waves.elapsedSlowest", { duration: formatMs(wave.elapsedMs) }));
  if (wave.bundleChars !== null)
    lines.push(t("waves.bundleChars", { count: wave.bundleChars }));
  return lines.join("\n");
}

function cellTitle(task: PlanTaskRecord): string {
  const lines = [
    t("waves.cellState", {
      id: task.to,
      name: displayName(task.to),
      state: t(STATE_LABEL_KEYS[task.state]),
    }),
    t("waves.requestChars", { count: task.msgChars }),
  ];
  if (task.elapsedMs !== null)
    lines.push(t("waves.elapsedQueued", { duration: formatMs(task.elapsedMs) }));
  return lines.join("\n");
}

// ---- 編集窓（Spec 43）。確認待ちの波を 1 件ずつ、古い順に扱う。 ------------

/** 確認待ちの波（最古の 1 件）。複数あるときは件数だけ横に出す。 */
const pendingWave = computed(() => state.planWaves.find((w) => w.state === "pending"));
const pendingCount = computed(
  () => state.planWaves.filter((w) => w.state === "pending").length,
);

/**
 * 編集中の下書き。**フロントだけが持つ**（D4 — 編集の中間状態に API を
 * 与えない。画面を切り替えたら消え、提案は記録された形のまま残る）。
 */
const draft = ref<PlanTaskInput[]>([]);
/** 追加行の宛先選択。 */
const addTarget = ref<AgentId | "">("");
/** 実行・破棄の連打防止。 */
const busy = ref(false);

watch(
  () => pendingWave.value?.planId,
  () => {
    // 提案の真実（記録された tasks）から下書きを組み直す。
    draft.value = (pendingWave.value?.tasks ?? []).map((task) => ({
      to: task.to,
      message: task.message ?? "",
    }));
    addTarget.value = "";
  },
  { immediate: true },
);

/**
 * 追加で選べる宛先 = **進行役の今の接続先**から、下書きに居ない相手。
 * 窓は線を引き直す場所ではない（凍結 3 — 検証はコア側でもう一度掛かる）。
 */
const addableTargets = computed(() => {
  const wave = pendingWave.value;
  if (!wave) return [];
  const connected =
    state.agents.find((a) => a.id === wave.agentId)?.connectedAgents ?? [];
  return connected.filter((id) => !draft.value.some((task) => task.to === id));
});

function removeDraftTask(index: number): void {
  draft.value.splice(index, 1);
}

function addDraftTask(): void {
  if (!addTarget.value) return;
  draft.value.push({ to: addTarget.value, message: "" });
  addTarget.value = "";
}

async function dispatchPending(): Promise<void> {
  const wave = pendingWave.value;
  if (!wave || busy.value) return;
  busy.value = true;
  try {
    // 成功の投影は event（planWaveStarted）が運ぶ。失敗は mutate がトーストへ。
    await orchestrator.dispatchPlanWave(wave.planId, draft.value);
  } finally {
    busy.value = false;
  }
}

async function discardPending(): Promise<void> {
  const wave = pendingWave.value;
  if (!wave || busy.value) return;
  busy.value = true;
  try {
    await orchestrator.discardPlanWave(wave.planId);
  } finally {
    busy.value = false;
  }
}

/** 横スクロールの入れ物。右端追従の判定に使う。 */
const scroller = ref<HTMLElement | null>(null);

/**
 * 新しい波が届いたら右端へ追従する — ただし**既に右端に居るときだけ**。
 * 過去の波へ遡って読んでいる最中に視点を奪わない（Airflow も追従しない）。
 */
watch(
  () => state.planWaves.length,
  async () => {
    const el = scroller.value;
    if (!el) return;
    const atRight = el.scrollLeft + el.clientWidth >= el.scrollWidth - 16;
    if (!atRight) return;
    await nextTick();
    el.scrollLeft = el.scrollWidth;
  },
);
</script>

<template>
  <div class="flex h-full flex-col">
    <!--
      高さは 4 ペイン共通の 38px 固定（AgentList のコメント参照）。
      下線は引かない — VS Code のパネル（問題 / 出力）と同じで、タブと中身の
      間に境界線を置かない（2026-08-02 利用者指定。下段タブの 2 ペイン共通）。
    -->
    <header
      class="flex h-[38px] shrink-0 items-center gap-3 px-3 text-xs text-ink-dim"
    >
      <!-- タイトルはタブが兼ねる。読み方の説明はタブのホバーへ（サーヴァントの絆と同じ規則）。 -->
      <BottomPaneTabs :active="activeTab" @select="emit('selectTab', $event)" />
      <span v-if="waves.length">{{ $t("waves.waveCount", { count: waves.length }) }}</span>
    </header>

    <!-- 確認待ちの編集パネル（Spec 43）。最古の 1 件を扱い、残りは件数だけ。 -->
    <div
      v-if="pendingWave"
      class="shrink-0 border-b border-line bg-surface-1 px-3 py-2"
      data-plan-pending-editor
    >
      <div class="mb-1.5 flex items-center gap-2 text-[11px]">
        <span class="rounded-sm bg-warn/20 px-1.5 py-0.5 text-warn">
          {{ $t("waves.pendingBadge") }}
        </span>
        <span class="text-ink">
          {{ $t("waves.pendingTitle", { name: displayName(pendingWave.agentId) }) }}
        </span>
        <span v-if="pendingCount > 1" class="text-ink-dim">
          {{ $t("waves.pendingMore", { count: pendingCount - 1 }) }}
        </span>
      </div>
      <div class="flex flex-col gap-1.5">
        <div
          v-for="(task, index) in draft"
          :key="`${pendingWave.planId}:${task.to}`"
          class="flex items-start gap-2"
        >
          <span class="w-32 shrink-0 truncate pt-1 text-[11px] text-ink" :title="task.to">
            {{ displayName(task.to) }}
          </span>
          <textarea
            v-model="task.message"
            rows="2"
            class="min-h-[2.5rem] flex-1 resize-y rounded border border-line bg-surface-0 px-2 py-1 text-[12px] text-ink"
          />
          <button
            type="button"
            class="shrink-0 pt-1 text-[11px] text-ink-dim hover:text-fail"
            :title="$t('waves.removeTask')"
            @click="removeDraftTask(index)"
          >
            ✕
          </button>
        </div>
        <div class="flex items-center gap-2">
          <select
            v-model="addTarget"
            class="rounded border border-line bg-surface-0 px-1.5 py-0.5 text-[11px] text-ink"
          >
            <option value="">{{ $t("waves.addTargetPlaceholder") }}</option>
            <option v-for="id in addableTargets" :key="id" :value="id">
              {{ displayName(id) }}
            </option>
          </select>
          <button
            type="button"
            class="text-[11px] text-ink-dim hover:text-ink disabled:opacity-40"
            :disabled="!addTarget"
            @click="addDraftTask"
          >
            {{ $t("waves.addTask") }}
          </button>
          <div class="flex-1" />
          <button
            type="button"
            class="rounded border border-line px-2 py-0.5 text-[11px] text-ink-dim hover:text-fail disabled:opacity-40"
            :disabled="busy"
            @click="discardPending"
          >
            {{ $t("waves.discard") }}
          </button>
          <button
            type="button"
            class="rounded bg-accent px-3 py-0.5 text-[11px] text-surface-0 disabled:opacity-40"
            :disabled="busy || draft.length === 0"
            @click="dispatchPending"
          >
            {{ $t("waves.dispatch") }}
          </button>
        </div>
      </div>
    </div>

    <div
      v-if="waves.length === 0"
      class="flex flex-1 items-center justify-center text-xs text-ink-dim"
    >
      {{ $t("waves.empty") }}
    </div>

    <div v-else ref="scroller" class="min-h-0 flex-1 overflow-auto">
      <div
        class="grid w-max min-w-full"
        :style="{ gridTemplateColumns: `150px repeat(${waves.length}, 104px)` }"
      >
        <!-- 見出し行: 進行役 + ターン内の波番号。同定は planId（ツールチップ）。 -->
        <div
          class="sticky left-0 z-10 border-b border-line bg-surface-0 px-3 py-1.5 text-[11px] text-ink-dim"
        >
          {{ $t("waves.servant") }}
        </div>
        <div
          v-for="wave in waves"
          :key="`head-${wave.planId}`"
          class="border-b border-line px-2 py-1 text-center text-[11px] text-ink-dim"
          :title="waveTitle(wave)"
        >
          <span class="block truncate text-ink">{{ displayName(wave.agentId) }}</span>
          <span class="block text-[10px]">
            {{ $t("waves.waveN", { n: wave.wave })
            }}<template v-if="wave.state === 'pending'"> · {{ $t("waves.pendingBadge") }}</template
            ><template v-else-if="wave.state === 'discarded'"> · {{ $t("waves.discardedBadge") }}</template
            ><template v-else-if="wave.elapsedMs !== null"> · {{ formatMs(wave.elapsedMs) }}</template>
          </span>
        </div>

        <!-- 本体: エージェント行 × 波列。 -->
        <template v-for="agent in rows" :key="agent.id">
          <div
            class="sticky left-0 z-10 truncate border-b border-line/50 bg-surface-0 px-3 py-1.5 text-[11px] text-ink"
            :title="$t('waves.agentTitle', { id: agent.id, name: agent.name })"
          >
            {{ agent.name }}
          </div>
          <div
            v-for="wave in waves"
            :key="`${wave.planId}:${agent.id}`"
            class="flex items-center justify-center border-b border-line/50 px-2 py-1.5"
          >
            <!-- 確認待ちは破線（動いていない）、破棄は薄く。実行痕だけが色を持つ。 -->
            <div
              v-if="taskFor(wave, agent.id)"
              class="h-3.5 w-full rounded-sm"
              :class="
                wave.state === 'pending'
                  ? 'border border-dashed border-warn/60'
                  : wave.state === 'discarded'
                    ? 'bg-line/40'
                    : cellClass(taskFor(wave, agent.id)!.state)
              "
              :title="cellTitle(taskFor(wave, agent.id)!)"
            />
          </div>
        </template>
      </div>
    </div>
  </div>
</template>
