<script setup lang="ts">
/**
 * 中央ペイン上部: サーヴァントの絆（旧「村の地図」）。
 *
 * **描画は `v-network-graph`**（2026-08-13 に Vue Flow から差し替えた試作）。
 * 選んだ理由は 2 つで、**1 つ目が決定的**:
 *
 * - **SVG で、ノードを Vue のスロットで描ける。** 色は CSS から引けるので
 *   「配色は `style.css` の 1 箇所」の規律が破れない。canvas（vis-network）だと
 *   描画コードが色の 2 箇所目になり、テーマの切り替えに自前で追従する羽目になる
 * - **`layouts.nodes` が `topologyPositions` と 1 対 1。** 既にある IPC と
 *   `world.json` の欄がそのまま使える
 *
 * **座標は人が置く**（`SimpleLayout`）。自動整列は入れていない —
 * 「なくてもよいかもしれない」が利用者の裁定で、規則的な配置は実機で
 * 2 度却下されている（円環・エゴ中心の放射とも「網ではなく図表に見える」）。
 * 埋めるのは**一度も置かれていない個体だけ**で、規則は `lib/kizunaSeed`。
 *
 * **失ったものが 1 つある** — このライブラリには**辺をドラッグで作る機構が無い**
 * （イベントに接続系が無い）。Spec 21 の 3 経路のうち**ハンドル引きが消える**。
 * カードの drop と役職ダイアログのチェックは残るので、辺を作れなくはならない。
 *
 * 辺の追加・削除はグラフ上の操作をそのままコアへ流す。
 * 自己ループと未登録先はコア側が拒否するので、ここでは事前検査しない
 * （検査を二重に持つと、片方だけ直したときに規則が食い違う）。
 */
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import {
  VNetworkGraph,
  defineConfigs,
  type Edges,
  type EventHandlers,
  type Layouts,
  type Nodes,
} from "v-network-graph";
import "v-network-graph/lib/style.css";

import { drawDirection } from "../lib/kizunaEdges";
import { seedPositions } from "../lib/kizunaSeed";
import { avatarHue, avatarInitial } from "../lib/avatar";
import { askConfirm } from "../composables/useConfirm";
import { useOrchestrator } from "../composables/useOrchestrator";
import { useUiSettings } from "../composables/useUiSettings";
import type { AgentId } from "../types";

const { t } = useI18n();

const orchestrator = useOrchestrator();
const { state } = orchestrator;
const { settings } = useUiSettings();

const graph = ref<InstanceType<typeof VNetworkGraph> | null>(null);
const canvas = ref<HTMLElement | null>(null);

/** ノードの見た目の寸法（SVG 座標）。`configs` と slot の両方が読む。 */
const NODE_RADIUS = 26;

/* ------------------------------------------------------------------ *
 * データ
 * ------------------------------------------------------------------ */

const nodes = computed<Nodes>(() => {
  const result: Nodes = {};
  for (const agent of state.agents) {
    result[agent.id] = { name: agent.name };
  }
  return result;
});

/**
 * 辺。**双方向の接続は 2 本ではなく、両端に矢を持つ 1 本にまとめる。**
 *
 * 2 本並べると、同じ 2 点を結ぶ線が重なって太い 1 本にしか見えないうえ、
 * 矢が両端に付いた状態と区別がつかない。だったら初めから 1 本で描いて、
 * 「双方向である」ことを形で示すほうが正しい。
 */
const edges = computed<Edges>(() => {
  const exists = (source: AgentId, target: AgentId) =>
    state.edges.some((e) => e.source === source && e.target === target);

  /** 左ペインでの並び順。**引けなければ末尾へ**（`NaN` を作らない）。 */
  const orderOf = (id: AgentId) =>
    state.agents.find((a) => a.id === id)?.order ?? Number.MAX_SAFE_INTEGER;

  const result: Edges = {};
  const seen = new Set<string>();

  for (const edge of state.edges) {
    const bidirectional = exists(edge.target, edge.source);
    const id = bidirectional
      ? [edge.source, edge.target].sort().join("<->")
      : `${edge.source}->${edge.target}`;
    if (seen.has(id)) continue;
    seen.add(id);

    const [source, target] = drawDirection(
      edge.source,
      edge.target,
      bidirectional,
      orderOf,
    );
    result[id] = { source, target, bidirectional };
  }

  return result;
});

/** 双方向にまとまった辺の本数。見出しの内訳に出す。 */
const bidirectionalCount = computed(
  () => Object.values(edges.value).filter((e) => e.bidirectional).length,
);

const running = (id: AgentId) =>
  state.agents.some((a) => a.id === id && a.status === "running");

/** 稼働中の個体から出ている辺（流れる破線にする）。 */
function edgeIsLive(edge: { source: string; target: string; bidirectional?: boolean }) {
  return running(edge.source) || (Boolean(edge.bidirectional) && running(edge.target));
}

/* ------------------------------------------------------------------ *
 * 配置（人が置く。埋めるのは未配置だけ）
 * ------------------------------------------------------------------ */

const layouts = reactive<Layouts>({ nodes: {} });

/**
 * 保存済みの座標を写し、未配置だけを埋める。
 *
 * **`world.json` を真実として一方向に流す。** ドラッグの結果は
 * `node:dragend` でコアへ返し、コアの投影がここへ戻ってくる。
 */
function syncLayouts(): void {
  const ids = state.agents.map((a) => a.id);
  const placed = state.topologyPositions;

  for (const id of ids) {
    const saved = placed[id];
    if (saved) layouts.nodes[id] = { ...saved };
  }
  Object.assign(layouts.nodes, seedPositions(ids, placed));

  // 消えた個体の座標は落とす（残すと辺の無い幽霊が描かれ続ける）。
  for (const id of Object.keys(layouts.nodes)) {
    if (!ids.includes(id as AgentId)) delete layouts.nodes[id];
  }
}

watch(
  () => [state.agents.map((a) => a.id).join(","), state.topologyPositions] as const,
  syncLayouts,
  { immediate: true, deep: true },
);

/* ------------------------------------------------------------------ *
 * 見た目
 * ------------------------------------------------------------------ */

/**
 * **形と太さだけをここで決め、色は CSS に置く。**
 *
 * `configs` に色を書くと SVG の属性として焼き付き、テーマの切り替えに
 * 追従しない（`var()` は属性値では解決されない）。だから塗りは
 * `<style>` の `.v-ng-*` 側で当てる — **色の置き場を 1 箇所に保つ**。
 */
const configs = defineConfigs({
  view: {
    scalingObjects: true,
    minZoomLevel: 0.3,
    maxZoomLevel: 2,
    autoPanAndZoomOnLoad: "fit-content",
  },
  node: {
    selectable: false,
    draggable: true,
    normal: { type: "circle", radius: NODE_RADIUS },
    hover: { type: "circle", radius: NODE_RADIUS },
    // 名前はスロットで描くので、組み込みのラベルは出さない。
    label: { visible: false },
    focusring: { visible: false },
  },
  edge: {
    selectable: true,
    normal: {
      width: (edge) => (edge.bidirectional ? 3 : 1.6),
      animate: (edge) => edgeIsLive(edge as never),
    },
    hover: { width: (edge) => (edge.bidirectional ? 4 : 2.4) },
    marker: {
      source: {
        // 引数は [edge, stroke] の組（このライブラリの marker だけ形が違う）。
        type: ([edge]) => (edge.bidirectional ? "arrow" : "none"),
        width: 4,
        height: 4,
      },
      target: { type: "arrow", width: 4, height: 4 },
    },
  },
});

function ringClass(id: AgentId): string {
  if (id === state.selectedAgentId) return "is-selected";
  const agent = state.agents.find((a) => a.id === id);
  switch (agent?.status) {
    case "running":
      return "is-running";
    case "failed":
      return "is-failed";
    case "starting":
    case "stopping":
      return "is-warn";
    default:
      return "is-idle";
  }
}

function nameOf(id: AgentId): string {
  return state.agents.find((a) => a.id === id)?.name ?? id;
}

/* ------------------------------------------------------------------ *
 * 操作
 * ------------------------------------------------------------------ */

/** 宛先の表示。id と表示名の併記（Spec 06 の規律）。 */
function agentLabel(id: string): string {
  const agent = state.agents.find((a) => a.id === id);
  return agent ? t("map.agentLabel", { id, name: agent.name }) : id;
}

/**
 * 辺を切る。
 *
 * 双方向を 1 本で描いている以上、その線を切れば**両方向とも切れる**のが
 * 見た目と一致する。片方だけ残すなら、設定ダイアログの接続先チェックで行う。
 *
 * 線は棚卸しで**唯一、確認なしで消える破壊的操作**だった（Spec 13 S4）。
 * 確認は既定 ON で、システム設定「線削除の確認」から切れる。
 */
async function removeEdge(edgeId: string): Promise<void> {
  const edge = edges.value[edgeId];
  if (!edge) return;

  if (settings.confirmEdgeDelete) {
    const arrow = edge.bidirectional ? "⇄" : "→";
    const ok = await askConfirm({
      title: t("map.confirmDeleteTitle"),
      message:
        `${agentLabel(edge.source)} ${arrow} ${agentLabel(edge.target)}` +
        (edge.bidirectional ? `\n${t("map.bidirectionalNote")}` : ""),
      confirmLabel: t("map.confirmDeleteLabel"),
      danger: true,
    });
    if (!ok) return;
  }

  const forward = state.agents.find((a) => a.id === edge.source);
  const backward = edge.bidirectional
    ? state.agents.find((a) => a.id === edge.target)
    : undefined;

  if (forward) {
    await orchestrator.setConnections(
      forward.id,
      forward.connectedAgents.filter((id) => id !== edge.target),
    );
  }
  if (backward) {
    await orchestrator.setConnections(
      backward.id,
      backward.connectedAgents.filter((id) => id !== edge.source),
    );
  }
}

const handlers: EventHandlers = {
  "node:click": ({ node }) => orchestrator.select(node as AgentId),
  // まとめ表示のときは `edge` が無く `edges` が来るので、単体のときだけ切る。
  "edge:click": ({ edge }) => {
    if (edge) void removeEdge(edge);
  },
  // ドラッグの結果を `world.json` へ返す。**移動した個体だけ**を書く。
  "node:dragend": (positions) => {
    for (const [id, position] of Object.entries(positions)) {
      void orchestrator.setTopologyPosition(id as AgentId, position);
    }
  },
};

/* ------------------------------------------------------------------ *
 * ペインの大きさが変わった後に Fit を掛け直す（2026-08-08 利用者要望）
 * ------------------------------------------------------------------ */

let observer: ResizeObserver | null = null;
let settle: ReturnType<typeof setTimeout> | null = null;

function fit(): void {
  graph.value?.fitToContents();
}

/**
 * 落ち着いてから 1 回だけ掛ける。**仕切りのドラッグ中は毎フレーム発火する**ので、
 * そのたびに掛けると視点が追いかけ続けて操作感が悪くなる（利用者の言葉も
 * 「大きさが変わった**後**に」）。
 */
function scheduleFit(): void {
  if (!settings.autoFitOnResize) return;
  if (settle) clearTimeout(settle);
  settle = setTimeout(() => {
    settle = null;
    fit();
  }, 120);
}

onMounted(() => {
  if (typeof ResizeObserver !== "function" || !canvas.value) return;
  observer = new ResizeObserver(scheduleFit);
  observer.observe(canvas.value);
});

onBeforeUnmount(() => {
  observer?.disconnect();
  if (settle) clearTimeout(settle);
});
</script>

<template>
  <div class="flex h-full flex-col">
    <!-- 高さは 4 ペイン共通の 38px 固定（AgentList のコメント参照）。 -->
    <header
      class="flex h-[38px] shrink-0 items-center gap-2 border-b border-line px-3 text-xs text-ink-dim"
    >
      <span class="font-medium text-ink">{{ $t("map.title") }}</span>
      <span>
        {{ $t("map.counts", { agents: state.agents.length, edges: Object.keys(edges).length }) }}
        <span v-if="bidirectionalCount" class="text-ink">
          {{ $t("map.bidirectional", { count: bidirectionalCount }) }}
        </span>
      </span>

      <span class="ml-auto flex items-center gap-1">
        <button
          type="button"
          class="rounded border border-line px-2 py-0.5 hover:bg-surface-2"
          :title="$t('map.fit')"
          @click="fit"
        >
          {{ $t("map.fit") }}
        </button>
        <button
          type="button"
          class="rounded border px-2 py-0.5 hover:bg-surface-2"
          :class="settings.autoFitOnResize ? 'border-accent text-accent' : 'border-line'"
          :title="$t('map.autoFit')"
          @click="settings.autoFitOnResize = !settings.autoFitOnResize"
        >
          {{ $t("map.autoFit") }}
        </button>
      </span>
    </header>

    <div ref="canvas" class="kizuna min-h-0 flex-1">
      <VNetworkGraph
        ref="graph"
        :nodes="nodes"
        :edges="edges"
        :layouts="layouts"
        :configs="configs"
        :event-handlers="handlers"
      >
        <defs>
          <clipPath id="kizuna-avatar" clipPathUnits="objectBoundingBox">
            <circle cx="0.5" cy="0.5" r="0.5" />
          </clipPath>
        </defs>

        <!--
          ノードの見た目。**運ぶのは「誰か」「選ばれているか」「動いているか」の
          3 つだけ。** モデル名・役職・トークンは左のカードに全部載っているので、
          ここに出すのは重複だった。

          **`data-kizuna-node` を自分で付ける。** Spec 21 の drop はこれを
          `closest` で辿る — ライブラリのクラス名を選択子に書くと、描画を
          差し替えたときに黙って壊れる（今回まさにそれを踏んだ）。
        -->
        <!--
          **当たり判定は下の CSS が与える**（`.kizuna-node { pointer-events: all }`）。
          ライブラリの既定ノードは、スロットへ渡ってくる `class`
          （`{ draggable, selectable }`）で `pointer-events` を得る作りだが、
          **その `class` は型宣言（`NodeSlotProps`）に載っていない** — 実行時には
          渡っているのに宣言が漏れている。宣言に無いものへ寄りかかると
          版が上がったときに黙って壊れるので、自分の CSS で与える。
          ドラッグとクリックのハンドラは**ライブラリが外側の `<g>` に付けている**
          ので、こちらが当たりさえすればイベントは届く。
        -->
        <template #override-node="{ nodeId, scale }">
          <g :data-kizuna-node="nodeId" :class="['kizuna-node', ringClass(nodeId as AgentId)]">
            <circle class="kizuna-ring" :r="NODE_RADIUS * scale" />

            <image
              v-if="state.icons[nodeId as AgentId]"
              :href="state.icons[nodeId as AgentId]!"
              :x="-NODE_RADIUS * scale"
              :y="-NODE_RADIUS * scale"
              :width="NODE_RADIUS * 2 * scale"
              :height="NODE_RADIUS * 2 * scale"
              clip-path="url(#kizuna-avatar)"
              preserveAspectRatio="xMidYMid slice"
            />
            <template v-else>
              <circle
                :r="(NODE_RADIUS - 2) * scale"
                :fill="avatarHue(nameOf(nodeId as AgentId))"
              />
              <text
                class="kizuna-initial"
                text-anchor="middle"
                dominant-baseline="central"
                :font-size="18 * scale"
              >
                {{ avatarInitial(nameOf(nodeId as AgentId)) }}
              </text>
            </template>

            <!-- 稼働中の印。**色ではなく点**（状態色と選択色の競合を避ける）。 -->
            <circle
              v-if="running(nodeId as AgentId)"
              class="kizuna-live"
              :cx="NODE_RADIUS * 0.72 * scale"
              :cy="-NODE_RADIUS * 0.72 * scale"
              :r="5 * scale"
            />

            <text
              class="kizuna-name"
              text-anchor="middle"
              :y="(NODE_RADIUS + 14) * scale"
              :font-size="11 * scale"
            >
              {{ nameOf(nodeId as AgentId) }}
            </text>
          </g>
        </template>
      </VNetworkGraph>
    </div>
  </div>
</template>

<style scoped>
/*
 * **色はここだけ。** `configs` へ書くと SVG の属性に焼き付いてテーマに
 * 追従しない（`var()` は属性値では解決されない）ので、塗りは CSS で当てる。
 * これが `v-network-graph`（SVG）を選んだ理由そのもの — canvas だと
 * この節が描画コードへ散る。
 */
/*
 * **これが無いと掴めない。** ライブラリの既定 CSS は
 * `.v-ng-node .draggable / .selectable` にだけ `pointer-events: all` を与える。
 * 自前のノードはそのクラスを持たないので、ここで自分に与える。
 * **見た目は正しいまま操作だけが死ぬ**種類の欠落で、実機でしか出ない。
 */
.kizuna :deep(.kizuna-node) {
  pointer-events: all;
  cursor: grab;
}
.kizuna :deep(.v-ng-canvas.dragging .kizuna-node) {
  cursor: grabbing;
}

.kizuna :deep(.kizuna-ring) {
  fill: var(--color-surface-1);
  stroke-width: 2;
  stroke: var(--color-line);
}
.kizuna :deep(.is-running .kizuna-ring) {
  stroke: var(--color-run);
}
.kizuna :deep(.is-failed .kizuna-ring) {
  stroke: var(--color-fail);
}
.kizuna :deep(.is-warn .kizuna-ring) {
  stroke: var(--color-warn);
}
/* 選択は**太さ + accent**。状態色が 4 種あるので、色だけで 5 つ目を足さない。 */
.kizuna :deep(.is-selected .kizuna-ring) {
  stroke: var(--color-accent);
  stroke-width: 4;
}

.kizuna :deep(.kizuna-initial) {
  fill: var(--color-surface-0);
  font-weight: 600;
}
.kizuna :deep(.kizuna-name) {
  fill: var(--color-ink);
  font-weight: 500;
}
.kizuna :deep(.kizuna-live) {
  fill: var(--color-run);
  stroke: var(--color-surface-0);
  stroke-width: 2;
}

.kizuna :deep(.v-ng-edge) {
  stroke: var(--color-accent);
}

/* 張られなかった drop への応答（Spec 21 D3）。**SVG に box-shadow は効かない**
   ので、Vue Flow 時代の影ではなく輪の拡大で返す。 */
.kizuna :deep(.kizuna-node.kizuna-pulse .kizuna-ring) {
  animation: kizuna-pulse 0.4s ease-out;
}
@keyframes kizuna-pulse {
  0% {
    stroke: var(--color-accent);
    stroke-width: 2;
  }
  50% {
    stroke: var(--color-accent);
    stroke-width: 8;
  }
  100% {
    stroke-width: 2;
  }
}
</style>
