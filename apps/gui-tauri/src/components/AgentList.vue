<script setup lang="ts">
/**
 * 左ペイン: 登録済みエージェントのカードリスト。
 *
 * 新規作成はここに置く。エージェントが 0 体のとき、作成導線が
 * 別画面にあると「何もできないアプリ」に見えるため。
 */
import { computed, ref } from "vue";

import AgentCard from "./AgentCard.vue";
import AgentSettingsDialog from "./AgentSettingsDialog.vue";
import ModelTemplateDialog from "./ModelTemplateDialog.vue";
import { useOrchestrator } from "../composables/useOrchestrator";
import type { AgentId, AgentSpec } from "../types";

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
  };

  const created = await orchestrator.createAgent(spec);
  if (created) {
    orchestrator.select(created.id);
    newName.value = "";
    creating.value = false;
  }
}

/** カードを 1 つ上下に動かす。 */
async function move(agentId: AgentId, direction: -1 | 1): Promise<void> {
  const ordered = agents.value.map((a) => a.id);
  const from = ordered.indexOf(agentId);
  const to = from + direction;
  if (from < 0 || to < 0 || to >= ordered.length) return;

  [ordered[from], ordered[to]] = [ordered[to], ordered[from]];
  await orchestrator.reorder(ordered);
}
</script>

<template>
  <div class="flex h-full flex-col">
    <header
      class="flex shrink-0 items-center gap-2 border-b border-line px-3 py-2.5 text-xs"
    >
      <h2 class="flex-1 font-semibold tracking-wide">エージェント</h2>
      <span class="text-ink-dim tabular-nums">
        {{ runningCount }} / {{ state.agents.length }} 稼働
      </span>
      <button
        class="rounded border border-line px-1.5 py-0.5 hover:border-accent hover:text-accent"
        title="モデルテンプレートを管理"
        @click="showTemplates = true"
      >
        ⚙
      </button>
      <button
        class="rounded border border-line px-1.5 py-0.5 hover:border-accent hover:text-accent"
        title="エージェントを追加"
        @click="creating = !creating"
      >
        ＋
      </button>
    </header>

    <form v-if="creating" class="border-b border-line p-3" @submit.prevent="submitNew">
      <input
        v-model="newName"
        placeholder="エージェント名（例: PlannerAgent）"
        autofocus
        class="w-full rounded border border-line bg-surface-1 px-2 py-1.5 outline-none focus:border-accent"
      />
      <p v-if="!state.templates.length" class="mt-1.5 text-[11px] text-warn">
        モデルテンプレートが未登録です。⚙ から先に 1 件登録してください。
      </p>
      <div class="mt-2 flex justify-end gap-2">
        <button
          type="button"
          class="rounded px-2 py-1 text-ink-dim hover:text-ink"
          @click="creating = false"
        >
          取消
        </button>
        <button
          type="submit"
          class="rounded bg-accent px-3 py-1 font-medium text-surface-0 disabled:opacity-40"
          :disabled="!newName.trim()"
        >
          作成
        </button>
      </div>
    </form>

    <div class="min-h-0 flex-1 space-y-2 overflow-y-auto p-3">
      <AgentCard
        v-for="agent in agents"
        :key="agent.id"
        :agent="agent"
        :icon="state.icons[agent.id]"
        :selected="agent.id === state.selectedAgentId"
        @select="orchestrator.select(agent.id)"
        @configure="configuring = agent.id"
        @toggle="(running) => orchestrator.toggleRunning(agent.id, running)"
        @move="(direction) => move(agent.id, direction)"
      />

      <p
        v-if="!agents.length"
        class="px-2 py-8 text-center text-[11px] leading-relaxed text-ink-dim"
      >
        エージェントがまだありません。<br />
        右上の ＋ から追加してください。
      </p>
    </div>

    <ModelTemplateDialog v-if="showTemplates" @close="showTemplates = false" />
    <AgentSettingsDialog
      v-if="configuring"
      :agent-id="configuring"
      @close="configuring = null"
    />
  </div>
</template>
