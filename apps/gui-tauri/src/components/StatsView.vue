<script setup lang="ts">
/**
 * 統計画面（Spec 39）— 3 ペインを丸ごと差し替える全画面。
 *
 * **数字はコアの集計（`sessionStats`）から出る唯一の経路**で、ここは描くだけ
 * （実効の重みも比も再計算しない — `stats_contract`）。取り直しは
 * `turnRecorded` の通知（`state.turnRecordedTick`）と、スコープ・会話の切り替え。
 * この画面は `v-if` で足されるので、閉じている間は誰も `sessionStats` を叩かない。
 *
 * **記録の無い会話は 0 の表を出さない**（D6）— この版より前の会話は `Turn` を
 * 持たず、0 は「払っていない」と読まれる。
 *
 * 描画はテーブル + SVG の棒 1 本。チャートライブラリは足さない — 色は
 * `style.css` の CSS 変数から引く規律で、SVG なら破れない（D8）。
 */

import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";

import { useOrchestrator } from "../composables/useOrchestrator";
import { formatError } from "../lib/errorText";
import { compactNumber, exactNumber } from "../lib/format";
import { sessionStats, toErrorPayload } from "../lib/ipc";
import {
  STOP_LABEL_KEYS,
  formatDuration,
  formatPercent,
  seriesBars,
  statsNotice,
  stopTone,
} from "../lib/statsView";
import type { ErrorPayload, StatsReport, StatsScope, StopCount } from "../types";

const emit = defineEmits<{ (e: "close"): void }>();

const { t } = useI18n();
const { state } = useOrchestrator();

/** スコープの選択。既定は今の会話。 */
const scopeKind = ref<"session" | "all">("session");
const report = ref<StatsReport | null>(null);
const error = ref<ErrorPayload | null>(null);
/** 飛行中の取得を数える。古い応答が新しい応答を上書きしないよう、最後の 1 本だけ採る。 */
let fetchSeq = 0;

const scope = computed<StatsScope | null>(() => {
  if (scopeKind.value === "all") return { kind: "all" };
  if (!state.currentSessionId) return null;
  return { kind: "session", sessionId: state.currentSessionId };
});

async function refresh(): Promise<void> {
  const target = scope.value;
  if (!target) {
    report.value = null;
    return;
  }
  const seq = ++fetchSeq;
  try {
    const next = await sessionStats(target);
    if (seq !== fetchSeq) return;
    report.value = next;
    error.value = null;
  } catch (err) {
    if (seq !== fetchSeq) return;
    error.value = toErrorPayload(err);
  }
}

// 開いた瞬間・スコープや会話が変わったとき・ターンが記録されたとき。
watch([scope, () => state.turnRecordedTick], () => void refresh(), { immediate: true });

const notice = computed(() => statsNotice(report.value));

/** 個体の表示名（カードと同じ）。居なくなった個体は id のまま。 */
function agentName(agentId: string): string {
  return state.agents.find((a) => a.id === agentId)?.name ?? agentId;
}

function stopLabel(row: StopCount): string {
  const base = t(STOP_LABEL_KEYS[row.stop]);
  return row.code ? `${base}: ${row.code}` : base;
}

/** SVG の棒。描画領域は固定（横 720 / 縦 96）— `viewBox` で伸縮する。 */
const SERIES_W = 720;
const SERIES_H = 96;
const bars = computed(() =>
  report.value?.series ? seriesBars(report.value.series.points, SERIES_W, SERIES_H) : [],
);

const totals = computed(() => report.value?.totals ?? null);
</script>

<template>
  <section class="flex min-h-0 flex-1 flex-col overflow-hidden bg-surface-0" data-stats-view>
    <!-- 見出し行: 題・スコープ・戻る -->
    <header class="flex shrink-0 items-center gap-3 border-b border-line bg-surface-1 px-4 py-2">
      <h1 class="text-sm font-bold text-ink">{{ t("stats.title") }}</h1>
      <div class="flex items-center gap-1 rounded border border-line p-0.5 text-xs" role="tablist">
        <button
          type="button"
          class="scope-btn"
          :class="{ 'is-on': scopeKind === 'session' }"
          role="tab"
          :aria-selected="scopeKind === 'session'"
          @click="scopeKind = 'session'"
        >
          {{ t("stats.scope.session") }}
        </button>
        <button
          type="button"
          class="scope-btn"
          :class="{ 'is-on': scopeKind === 'all' }"
          role="tab"
          :aria-selected="scopeKind === 'all'"
          @click="scopeKind = 'all'"
        >
          {{ t("stats.scope.all") }}
        </button>
      </div>
      <p class="min-w-0 flex-1 truncate text-xs text-ink-dim">{{ t("stats.unitNote") }}</p>
      <button
        type="button"
        class="rounded border border-line px-2 py-1 text-xs text-ink-dim hover:text-ink"
        @click="emit('close')"
      >
        {{ t("stats.backToVillage") }}
      </button>
    </header>

    <div class="min-h-0 flex-1 overflow-auto p-4">
      <p v-if="error" class="mb-3 rounded border border-fail px-3 py-2 text-xs text-fail">
        {{ formatError(error) }}
      </p>

      <p v-if="notice === 'loading'" class="text-xs text-ink-dim">{{ t("stats.loading") }}</p>

      <!-- 記録が無い会話: 0 の表を出さず、1 行だけ（D6）。 -->
      <p v-else-if="notice === 'empty'" class="text-sm text-ink-dim" data-stats-empty>
        {{ scopeKind === "all" ? t("stats.emptyAll") : t("stats.emptySession") }}
      </p>

      <template v-else-if="report && totals">
        <!-- 合計タイル -->
        <div class="mb-4 grid grid-cols-2 gap-2 md:grid-cols-3 lg:grid-cols-6">
          <div class="tile">
            <div class="tile-label">{{ t("stats.tiles.turns") }}</div>
            <div class="tile-value">{{ exactNumber(totals.turns) }}</div>
            <div class="tile-sub" :class="{ 'text-warn': totals.failed > 0 }">
              {{ t("stats.tiles.failedOf", { count: exactNumber(totals.failed) }) }}
            </div>
          </div>
          <div class="tile">
            <div class="tile-label">{{ t("stats.tiles.effective") }}</div>
            <div class="tile-value" :title="exactNumber(totals.effective)">
              {{ compactNumber(totals.effective) }}
            </div>
            <div class="tile-sub">{{ t("stats.tiles.effectiveNote") }}</div>
          </div>
          <div class="tile">
            <div class="tile-label">{{ t("stats.tiles.prompt") }}</div>
            <div class="tile-value" :title="exactNumber(totals.prompt)">
              {{ compactNumber(totals.prompt) }}
            </div>
            <div class="tile-sub" :class="{ 'text-warn': totals.prompt > 0 && totals.cached === 0 }">
              {{ t("stats.tiles.cacheRate", { rate: formatPercent(totals.cacheRate) }) }}
            </div>
          </div>
          <div class="tile">
            <div class="tile-label">{{ t("stats.tiles.completion") }}</div>
            <div class="tile-value" :title="exactNumber(totals.completion)">
              {{ compactNumber(totals.completion) }}
            </div>
            <div class="tile-sub">
              {{ t("stats.tiles.reasoningOf", { count: compactNumber(totals.reasoning) }) }}
            </div>
          </div>
          <div class="tile">
            <div class="tile-label">{{ t("stats.tiles.avgTokens") }}</div>
            <div class="tile-value">{{ compactNumber(totals.avgTokensPerTurn) }}</div>
            <div class="tile-sub">
              {{ t("stats.tiles.outputShare", { rate: formatPercent(totals.outputShare) }) }}
            </div>
          </div>
          <div class="tile">
            <div class="tile-label">{{ t("stats.tiles.avgElapsed") }}</div>
            <div class="tile-value">{{ formatDuration(totals.avgElapsedMs) }}</div>
            <div class="tile-sub">{{ t("stats.tiles.perTurn") }}</div>
          </div>
        </div>

        <!-- 時系列（session のみ）: ターンごとの実効トークン。 -->
        <div v-if="report.series" class="mb-4 rounded border border-line bg-surface-1 p-3">
          <div class="mb-1 flex items-baseline justify-between text-xs text-ink-dim">
            <span>{{ t("stats.series.title") }}</span>
            <span v-if="report.series.dropped > 0">
              {{ t("stats.series.dropped", { count: exactNumber(report.series.dropped) }) }}
            </span>
          </div>
          <svg
            class="block h-24 w-full"
            :viewBox="`0 0 ${SERIES_W} ${SERIES_H}`"
            preserveAspectRatio="none"
            role="img"
            :aria-label="t('stats.series.title')"
          >
            <rect
              v-for="bar in bars"
              :key="bar.point.tsMs + bar.point.agentId"
              :x="bar.x"
              :y="bar.y"
              :width="bar.width"
              :height="bar.height"
              :class="bar.tone === 'ok' ? 'fill-accent' : 'fill-fail'"
            >
              <title>
                {{ agentName(bar.point.agentId) }} · {{ exactNumber(bar.point.effective) }} ·
                {{ t(STOP_LABEL_KEYS[bar.point.stop.kind]) }}
              </title>
            </rect>
          </svg>
        </div>

        <!-- 会話ごとの合計（all の主役の表）。 -->
        <div v-if="scopeKind === 'all'" class="mb-4 overflow-x-auto rounded border border-line">
          <table class="stats-table">
            <thead>
              <tr>
                <th>{{ t("stats.sessions.title") }}</th>
                <th>{{ t("stats.sessions.forkedFrom") }}</th>
                <th class="num">{{ t("stats.columns.turns") }}</th>
                <th class="num">{{ t("stats.columns.effective") }}</th>
              </tr>
            </thead>
            <tbody>
              <tr
                v-for="s in report.scopeMeta.sessions"
                :key="s.sessionId"
                :class="{ 'is-current': s.sessionId === state.currentSessionId }"
              >
                <td class="truncate" :title="s.sessionId">
                  {{ s.title || t("stats.sessions.untitled") }}
                </td>
                <td class="text-ink-dim">{{ s.forkedFrom ? t("stats.sessions.forked") : "" }}</td>
                <!-- 0 は「記録なし」— 払っていないのではなく、この版より前の会話。 -->
                <td class="num">{{ s.turns === 0 ? "—" : exactNumber(s.turns) }}</td>
                <td class="num">{{ s.turns === 0 ? "—" : compactNumber(s.effective) }}</td>
              </tr>
            </tbody>
          </table>
        </div>

        <!-- 個体別（promptfoo のプロバイダ見出しを表に写した形）。 -->
        <div class="mb-4 overflow-x-auto rounded border border-line">
          <table class="stats-table">
            <thead>
              <tr>
                <th>{{ t("stats.columns.agent") }}</th>
                <th>{{ t("stats.columns.model") }}</th>
                <th class="num">{{ t("stats.columns.turns") }}</th>
                <th class="num">{{ t("stats.columns.failed") }}</th>
                <th class="num">{{ t("stats.columns.effective") }}</th>
                <th class="num">{{ t("stats.columns.avgTokens") }}</th>
                <th class="num">{{ t("stats.columns.cacheRate") }}</th>
                <th class="num">{{ t("stats.columns.outputShare") }}</th>
                <th class="num">{{ t("stats.columns.avgElapsed") }}</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="row in report.byAgent" :key="row.agentId">
                <td :title="row.agentId">{{ agentName(row.agentId) }}</td>
                <td class="font-mono text-xs text-ink-dim">{{ row.model }}</td>
                <td class="num">{{ exactNumber(row.turns) }}</td>
                <td class="num" :class="{ 'text-warn': row.failed > 0 }">{{ exactNumber(row.failed) }}</td>
                <td class="num" :title="exactNumber(row.effective)">{{ compactNumber(row.effective) }}</td>
                <td class="num">{{ compactNumber(row.avgTokensPerTurn) }}</td>
                <td class="num" :class="{ 'text-warn': row.prompt > 0 && row.cached === 0 }">
                  {{ formatPercent(row.cacheRate) }}
                </td>
                <td class="num">{{ formatPercent(row.outputShare) }}</td>
                <td class="num">{{ formatDuration(row.avgElapsedMs) }}</td>
              </tr>
            </tbody>
          </table>
        </div>

        <!-- 終わり方の内訳。 -->
        <div class="rounded border border-line bg-surface-1 p-3">
          <div class="mb-2 text-xs text-ink-dim">{{ t("stats.stopsTitle") }}</div>
          <ul class="flex flex-wrap gap-2 text-xs">
            <li
              v-for="row in report.byStop"
              :key="row.stop + (row.code ?? '')"
              class="rounded border border-line px-2 py-1"
              :class="stopTone(row.stop) === 'ok' ? 'text-ink' : 'text-warn'"
            >
              {{ stopLabel(row) }}
              <span class="ml-1 font-mono">{{ exactNumber(row.count) }}</span>
            </li>
          </ul>
        </div>
      </template>
    </div>
  </section>
</template>

<style scoped>
.scope-btn {
  padding: 2px 8px;
  border-radius: 3px;
  color: var(--color-ink-dim);
  background: transparent;
  border: none;
  cursor: pointer;
}
.scope-btn.is-on {
  color: var(--color-ink);
  background: color-mix(in oklab, var(--color-accent) 20%, transparent);
}
.tile {
  border: 1px solid var(--color-line);
  background: var(--color-surface-1);
  border-radius: 4px;
  padding: 8px 10px;
}
.tile-label {
  font-size: 11px;
  color: var(--color-ink-dim);
}
.tile-value {
  font-size: 20px;
  font-weight: 700;
  color: var(--color-ink);
  font-variant-numeric: tabular-nums;
}
.tile-sub {
  font-size: 11px;
  color: var(--color-ink-dim);
}
.stats-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 12px;
}
.stats-table th,
.stats-table td {
  padding: 4px 8px;
  border-bottom: 1px solid var(--color-line);
  text-align: left;
  white-space: nowrap;
}
.stats-table th {
  color: var(--color-ink-dim);
  font-weight: 600;
  background: var(--color-surface-1);
}
.stats-table td.num,
.stats-table th.num {
  text-align: right;
  font-variant-numeric: tabular-nums;
}
.stats-table tr.is-current td {
  background: color-mix(in oklab, var(--color-accent) 10%, transparent);
}
.fill-accent {
  fill: var(--color-accent);
}
.fill-fail {
  fill: var(--color-fail);
}
</style>
