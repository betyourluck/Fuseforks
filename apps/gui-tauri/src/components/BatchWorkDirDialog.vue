<script setup lang="ts">
/**
 * 作業フォルダの一括切り替え（Spec 29）。
 *
 * 入口は**エージェント一覧のヘッダ**（「モデル登録」の隣）。タイトルバーでも
 * システム設定でもないのは、Spec 14 P3 の二分法が**タイトルバー vs システム
 * 設定**の裁定で、一覧ヘッダはその射程の外に最初から実在するため
 * （「モデル登録」が前例）。新しいカテゴリは作らず、前例の隣に置く。
 *
 * **`mutate()` を通さない**（`batchWorkDir.ts` の doc）。個体ごとに捕まえて
 * 結果を 1 つにまとめ、読み直しは最後に 1 回だけ。
 */
import { computed, ref } from "vue";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

import { snapshotToSpec } from "../lib/agentSpec";
import { inListOrder } from "../lib/agentNav";
import { applyWorkDir, canApply, type BatchSummary } from "../lib/batchWorkDir";
import { formatError } from "../lib/errorText";
import * as ipc from "../lib/ipc";
import { useOrchestrator } from "../composables/useOrchestrator";
import type { AgentId } from "../types";

const emit = defineEmits<{ (e: "close"): void }>();

const orchestrator = useOrchestrator();
const { state } = orchestrator;

/** 一覧の並びは左ペインと同じ（`inListOrder` の 1 実装を共有）。 */
const agents = computed(() => inListOrder(state.agents));

/**
 * チェック中の id。**既定は全 ON**（動機が「村ごと切り替え」なので全員が主ケース）。
 * **永続化しない** — ダイアログの寿命だけ（D1）。
 */
const checked = ref<Set<AgentId>>(new Set(state.agents.map((agent) => agent.id)));

const path = ref("");
const busy = ref(false);
/** 進捗（完了 / 総数）。適用中だけ出す。 */
const progress = ref<[number, number] | null>(null);
const summary = ref<BatchSummary | null>(null);

const targets = computed(() => agents.value.filter((agent) => checked.value.has(agent.id)));
const applicable = computed(() => canApply(path.value, targets.value.length));
const allChecked = computed(() => checked.value.size === state.agents.length);

function toggle(id: AgentId): void {
  // Set の同一参照を書き換えても computed が動かないので、作り直す。
  const next = new Set(checked.value);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  checked.value = next;
}

/** 全解除 / 全選択。選択的変更が「8 体分外す」にならないための 1 打（D1）。 */
function toggleAll(): void {
  checked.value = allChecked.value
    ? new Set()
    : new Set(state.agents.map((agent) => agent.id));
}

/**
 * ネイティブのフォルダ選択（「参照…」ボタンと同じ機構）。
 * 返るパスは **OS ネイティブ形式のまま**扱う（D2 — 正規化しない）。
 */
async function browse(): Promise<void> {
  const picked = await openDialog({ directory: true, multiple: false });
  if (typeof picked === "string") path.value = picked;
}

async function apply(): Promise<void> {
  if (!applicable.value || busy.value) return;

  // **trim は 1 箇所だけ**（承認時の注記 B）。ここで確定した値を渡すので、
  // D2（中身は見ない）と D3（有無だけ見る）の層分けが実装で崩れない。
  const trimmed = path.value.trim();

  busy.value = true;
  summary.value = null;
  progress.value = [0, targets.value.length];
  try {
    summary.value = await applyWorkDir(
      targets.value.map((agent) => ({ id: agent.id, name: agent.name })),
      async (target) => {
        // **適用の瞬間の投影から組み立てる**（承認時の注記 A・D2）。開いた時点の
        // snapshot を抱え込むと、その間に別経路で変わった欄まで巻き戻す。
        const current = state.agents.find((agent) => agent.id === target.id);
        if (!current) throw new Error(target.id);
        await ipc.updateAgent(snapshotToSpec(current, { workDir: trimmed }));
      },
      (error) => formatError(ipc.toErrorPayload(error)),
      (done, total) => {
        progress.value = [done, total];
      },
    );
  } finally {
    busy.value = false;
    progress.value = null;
    // **読み直しは最後に 1 回**。一覧の現在値表示も、これで適用後の値へ揃う。
    await orchestrator.refreshAll();
  }
}
</script>

<template>
  <div
    class="fixed inset-0 z-40 flex items-center justify-center bg-scrim"
    @click.self="emit('close')"
  >
    <div
      class="flex h-[560px] w-[560px] flex-col overflow-hidden rounded-lg border border-line bg-surface-1 shadow-2xl"
    >
      <header class="flex shrink-0 items-center gap-2 border-b border-line px-3 py-2.5 text-xs">
        <h2 class="flex-1 font-semibold">{{ $t("batchWorkDir.title") }}</h2>
        <button
          class="rounded px-1.5 text-ink-dim hover:text-accent"
          :title="$t('common.close')"
          @click="emit('close')"
        >
          ✕
        </button>
      </header>

      <p class="shrink-0 border-b border-line bg-surface-0 px-3 py-2 text-[11px] text-ink-dim">
        {{ $t("batchWorkDir.intro") }}
      </p>

      <div class="min-h-0 flex-1 overflow-y-auto px-3 py-2 text-[11px]">
        <div class="mb-1.5 flex items-center gap-2">
          <span class="flex-1 text-ink-dim">
            {{ $t("batchWorkDir.selected", { count: targets.length, total: agents.length }) }}
          </span>
          <button
            class="rounded border border-line px-1.5 py-0.5 hover:border-accent hover:text-accent"
            @click="toggleAll"
          >
            {{ allChecked ? $t("batchWorkDir.clearAll") : $t("batchWorkDir.selectAll") }}
          </button>
        </div>

        <!--
          現在値を並べるのは、変更前に現状が見えることが一括操作の安全側だから
          （どの個体が何を向いているかを知らずに上書きさせない）。
        -->
        <ul class="space-y-0.5">
          <li v-for="agent in agents" :key="agent.id">
            <label class="flex cursor-pointer items-center gap-2 rounded px-1 py-1 hover:bg-surface-2">
              <input
                type="checkbox"
                :checked="checked.has(agent.id)"
                :disabled="busy"
                @change="toggle(agent.id)"
              />
              <span class="w-32 shrink-0 truncate">{{ agent.name }}</span>
              <span
                class="min-w-0 flex-1 truncate font-mono text-ink-dim"
                :title="agent.workDir ?? ''"
              >
                {{ agent.workDir ?? $t("batchWorkDir.unset") }}
              </span>
            </label>
          </li>
        </ul>
      </div>

      <div class="shrink-0 space-y-2 border-t border-line px-3 py-2.5 text-[11px]">
        <div class="flex gap-1.5">
          <input
            v-model="path"
            type="text"
            :disabled="busy"
            :placeholder="$t('batchWorkDir.placeholder')"
            class="min-w-0 flex-1 rounded border border-line bg-surface-0 px-2 py-1 outline-none focus:border-accent"
          />
          <button
            class="shrink-0 rounded border border-line px-2 py-1 hover:border-accent hover:text-accent"
            :disabled="busy"
            @click="browse"
          >
            {{ $t("batchWorkDir.browse") }}
          </button>
        </div>

        <div class="flex items-center gap-2">
          <span v-if="progress" class="flex-1 text-ink-dim tabular-nums">
            {{ $t("batchWorkDir.progress", { done: progress[0], total: progress[1] }) }}
          </span>
          <span v-else class="flex-1" />
          <button
            class="rounded border border-accent px-2.5 py-1 text-accent hover:bg-accent hover:text-surface-0 disabled:cursor-not-allowed disabled:border-line disabled:text-ink-dim disabled:hover:bg-transparent"
            :disabled="!applicable || busy"
            @click="apply"
          >
            {{ $t("batchWorkDir.apply") }}
          </button>
        </div>

        <!--
          結果は個体ごとに名指しで出す（D2）。失敗した個体がどれで・なぜかが
          出ないと、直す手掛かりが無い。
        -->
        <div v-if="summary" class="space-y-0.5">
          <p>
            {{ $t("batchWorkDir.resultOk", { count: summary.succeeded }) }}
            <span v-if="summary.failed.length" class="text-warn">
              / {{ $t("batchWorkDir.resultFailed", { count: summary.failed.length }) }}
            </span>
          </p>
          <p v-for="failure in summary.failed" :key="failure.id" class="pl-2 text-warn">
            {{ failure.name }}（{{ failure.id }}）: {{ failure.reason }}
          </p>
        </div>
      </div>
    </div>
  </div>
</template>
