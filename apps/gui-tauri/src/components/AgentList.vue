<script setup lang="ts">
/**
 * 左ペイン: 登録済みエージェントのカードリスト。
 *
 * 新規作成はここに置く。エージェントが 0 体のとき、作成導線が
 * 別画面にあると「何もできないアプリ」に見えるため。
 */
import { computed, ref } from "vue";
import { VueDraggable } from "vue-draggable-plus";

import AgentCard from "./AgentCard.vue";
import AgentSettingsDialog from "./AgentSettingsDialog.vue";
import ModelTemplateDialog from "./ModelTemplateDialog.vue";
import { useOrchestrator } from "../composables/useOrchestrator";
import { batchAction, batchLabel } from "../lib/batchStart";
import type { AgentId, AgentSnapshot, AgentSpec } from "../types";

const orchestrator = useOrchestrator();
const { state } = orchestrator;

const showTemplates = ref(false);
/** 設定ダイアログを開いているエージェント。`null` なら閉じている。 */
const configuring = ref<AgentId | null>(null);
const creating = ref(false);
const newName = ref("");

/** 表示順に並べたエージェント。 */
const agents = computed(() => [...state.agents].sort((a, b) => a.order - b.order));

/** 稼働中の数。ヘッダの要約に出す。 */
const runningCount = computed(
  () => state.agents.filter((a) => a.status === "running").length,
);

/** 一括ボタンが次に行う操作。規則は lib/batchStart.ts。 */
const batch = computed(() => batchAction(state.agents));

/** 一括ボタンの記号と説明（押せないときは理由）。 */
const batchView = computed(() =>
  batchLabel(batch.value, state.agents.filter((a) => a.batchStart).length),
);

/** 一括起動 / 一括停止を実行する。 */
async function runBatch(): Promise<void> {
  if (batch.value.mode === "none") return;
  await orchestrator.runBatch(batch.value.targets, batch.value.mode === "start");
}

/** ID を名前から機械的に導く。衝突したら連番を足す。 */
function deriveId(name: string): AgentId {
  const base =
    name
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9_-]+/g, "_")
      .replace(/^_+|_+$/g, "") || "agent";

  if (!state.agents.some((a) => a.id === base)) return base;
  let n = 2;
  while (state.agents.some((a) => a.id === `${base}_${n}`)) n += 1;
  return `${base}_${n}`;
}

async function submitNew(): Promise<void> {
  const name = newName.value.trim();
  if (!name) return;

  const template = state.templates[0];
  if (!template) {
    // テンプレートが 1 件も無いと必ず失敗するので、先にそちらへ誘導する。
    showTemplates.value = true;
    return;
  }

  const spec: AgentSpec = {
    id: deriveId(name),
    name,
    modelTemplateId: template.id,
    ragSources: [],
    connectedAgents: [],
    order: state.agents.length,
    workDir: null,
    maxToolIterations: null,
    // 新規作成の保存値は null（既定に従う）。UI の全 ON 表示は null の効果。
    enabledTools: null,
    hearsRoomLog: true,
    // 作ったら一括起動の対象に入れる。外すのが例外側（重いモデル・実験中）。
    batchStart: true,
    // 役職は P3 の作成 UI で選ばせる（Spec 14）。P1 の時点では常に無し。
    roleId: null,
  };

  const created = await orchestrator.createAgent(spec);
  if (created) {
    orchestrator.select(created.id);
    newName.value = "";
    creating.value = false;
  }
}

/** ドラッグライブラリが返した表示順を保存する。 */
async function reorder(reordered: AgentSnapshot[]): Promise<void> {
  await orchestrator.reorder(reordered.map((agent) => agent.id));
}
</script>

<template>
  <div class="flex h-full flex-col">
    <!--
      ヘッダの高さは 4 ペイン共通で 38px に固定する（タイトルバーと同じ）。
      **パディングで揃えない** — 中身の高さがペインごとに違う（ボタンを持つ側は
      テキストだけの側より 6px 高い）ので、padding を合わせても実効高は割れる。
    -->
    <header
      class="flex h-[38px] shrink-0 items-center gap-2 border-b border-line px-3 text-xs"
    >
      <h2 class="font-semibold tracking-wide">{{ $t("agentList.heading") }}</h2>

      <!--
        一括起動 / 一括停止。対象は各カードのトグル（batchStart）で選ぶ。
        起こせる相手が居れば ▶、対象が全員動いていれば ■ へ役が変わる。
        規則は lib/batchStart.ts（純関数・単体テスト付き）。
      -->
      <button
        class="rounded border px-1.5 py-0.5 leading-none transition-colors disabled:opacity-40"
        :class="
          batch.mode === 'stop'
            ? 'border-line text-warn hover:border-warn'
            : 'border-line text-run hover:border-run'
        "
        :disabled="batch.mode === 'none'"
        :title="$t(batchView.titleKey, batchView.titleParams ?? {})"
        @click="runBatch"
      >
        {{ batchView.icon }}
      </button>

      <span class="flex-1 text-ink-dim tabular-nums">
        {{ $t("agentList.runningSummary", { running: runningCount, total: state.agents.length }) }}
      </span>
      <button
        class="grid size-6 place-items-center rounded text-ink-dim transition-colors hover:text-accent focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-accent"
        :title="$t('agentList.manageTemplates')"
        :aria-label="$t('agentList.manageTemplates')"
        @click="showTemplates = true"
      >
        <svg
          class="size-4"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.8"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M12 3a3.5 3.5 0 0 0-3.5 3.5c0 .7.2 1.4.6 2L12 12l2.9-3.5c.4-.6.6-1.3.6-2A3.5 3.5 0 0 0 12 3Z" />
          <path d="M8.5 8.5A3.5 3.5 0 0 0 5 12c0 .7.2 1.4.6 2L9 16.9l3-4.9" />
          <path d="M15.5 8.5A3.5 3.5 0 0 1 19 12c0 .7-.2 1.4-.6 2L15 16.9l-3-4.9" />
          <circle cx="12" cy="12" r="1.5" />
          <path d="M9 16.9 8.1 20 12 18.5l3.9 1.5-.9-3.1" />
        </svg>
      </button>
      <button
        class="grid size-6 place-items-center rounded text-ink-dim transition-colors hover:text-accent focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-accent"
        :title="$t('agentList.addServant')"
        :aria-label="$t('agentList.addServant')"
        @click="creating = !creating"
      >
        <svg
          class="size-4"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          aria-hidden="true"
        >
          <path d="M12 5v14M5 12h14" />
        </svg>
      </button>
    </header>

    <form v-if="creating" class="border-b border-line p-3" @submit.prevent="submitNew">
      <input
        v-model="newName"
        :placeholder="$t('agentList.namePlaceholder')"
        autofocus
        class="w-full rounded border border-line bg-surface-1 px-2 py-1.5 outline-none focus:border-accent"
      />
      <p v-if="!state.templates.length" class="mt-1.5 text-[11px] text-warn">
        {{ $t("agentList.noTemplates") }}
      </p>
      <div class="mt-2 flex justify-end gap-2">
        <button
          type="button"
          class="rounded px-2 py-1 text-ink-dim hover:text-ink"
          @click="creating = false"
        >
          {{ $t("agentList.cancel") }}
        </button>
        <button
          type="submit"
          class="rounded bg-accent px-3 py-1 font-medium text-surface-0 disabled:opacity-40"
          :disabled="!newName.trim()"
        >
          {{ $t("agentList.create") }}
        </button>
      </div>
    </form>

    <VueDraggable
      :model-value="agents"
      tag="div"
      class="min-h-0 flex-1 space-y-2 overflow-y-auto p-3"
      :animation="150"
      :force-fallback="true"
      ghost-class="opacity-40"
      chosen-class="agent-card--dragging"
      filter="button, input, textarea"
      :prevent-on-filter="false"
      @update:model-value="reorder"
    >
      <AgentCard
        v-for="agent in agents"
        :key="agent.id"
        :agent="agent"
        :icon="state.icons[agent.id]"
        :last-tool="state.lastTool[agent.id]"
        :selected="agent.id === state.selectedAgentId"
        @select="orchestrator.select(agent.id)"
        @configure="configuring = agent.id"
        @toggle="(running) => orchestrator.toggleRunning(agent.id, running)"
        @batch-start="(included) => orchestrator.setBatchStart(agent.id, included)"
      />

      <p
        v-if="!agents.length"
        class="px-2 py-8 text-center text-[11px] leading-relaxed text-ink-dim"
      >
        {{ $t("agentList.emptyLine1") }}<br />
        {{ $t("agentList.emptyLine2") }}
      </p>
    </VueDraggable>

    <ModelTemplateDialog v-if="showTemplates" @close="showTemplates = false" />
    <AgentSettingsDialog
      v-if="configuring"
      :agent-id="configuring"
      @close="configuring = null"
    />
  </div>
</template>

<style>
.agent-card--dragging {
  border-color: var(--color-accent);
}
</style>
