<script setup lang="ts">
/**
 * 中央ペイン上部: ノードベースの村の地図。
 *
 * トポロジーの真実は「どの辺があるか」だけであり、座標は world.json に保存する
 * 表示設定として分ける。未配置ノードだけはここで円環状に自動配置する。
 *
 * 辺の追加・削除はグラフ上の操作をそのままコアへ流す。
 * 自己ループと未登録先はコア側が拒否するので、ここでは事前検査しない
 * （検査を二重に持つと、片方だけ直したときに規則が食い違う）。
 */
import { computed } from "vue";
import {
  VueFlow,
  type Connection,
  type Edge,
  type Node,
  type NodeDragEvent,
} from "@vue-flow/core";
import { Background } from "@vue-flow/background";
import { Controls } from "@vue-flow/controls";

import { compactNumber } from "../lib/format";

import { avatarHue, avatarInitial } from "../lib/avatar";
import { useOrchestrator } from "../composables/useOrchestrator";
import { STATUS_LABELS, type AgentId } from "../types";

const orchestrator = useOrchestrator();
const { state } = orchestrator;

/** 円環配置の半径。ノード数に応じて広げ、重なりを避ける。 */
function radiusFor(count: number): number {
  return Math.max(140, count * 34);
}

const nodes = computed<Node[]>(() => {
  const list = [...state.agents].sort((a, b) => a.order - b.order);
  const radius = radiusFor(list.length);

  return list.map((agent, index) => {
    const angle = (index / Math.max(1, list.length)) * Math.PI * 2 - Math.PI / 2;
    const auto = {
      x: 320 + radius * Math.cos(angle),
      y: 220 + radius * Math.sin(angle),
    };

    return {
      id: agent.id,
      type: "default",
      position: state.topologyPositions[agent.id] ?? auto,
      data: { agent },
      class: agent.id === state.selectedAgentId ? "is-selected" : "",
    };
  });
});

/**
 * 辺。**双方向の接続は 2 本ではなく、両端に矢を持つ 1 本にまとめる。**
 *
 * 2 本並べると、同じ 2 点を結ぶ線が重なって太い 1 本にしか見えないうえ、
 * 矢が両端に付いた状態と区別がつかない。だったら初めから 1 本で描いて、
 * 「双方向である」ことを形で示すほうが正しい。
 */
const edges = computed<Edge[]>(() => {
  const exists = (source: AgentId, target: AgentId) =>
    state.edges.some((e) => e.source === source && e.target === target);

  const running = (id: AgentId) =>
    state.agents.some((a) => a.id === id && a.status === "running");

  const seen = new Set<string>();
  const result: Edge[] = [];

  for (const edge of state.edges) {
    const bidirectional = exists(edge.target, edge.source);
    // 双方向は向きを無視した鍵で正規化し、2 周目を弾く。
    const id = bidirectional
      ? `${[edge.source, edge.target].sort().join("<->")}`
      : `${edge.source}->${edge.target}`;
    if (seen.has(id)) continue;
    seen.add(id);

    result.push({
      id,
      source: edge.source,
      target: edge.target,
      data: { bidirectional },
      animated: running(edge.source) || (bidirectional && running(edge.target)),
      markerEnd: "arrowclosed",
      // 双方向のときだけ始端にも矢を付ける。
      markerStart: bidirectional ? "arrowclosed" : undefined,
    });
  }

  return result;
});

/** 双方向にまとまった辺の本数。見出しの内訳に出す。 */
const bidirectionalCount = computed(
  () => edges.value.filter((e) => e.data?.bidirectional).length,
);

async function onNodeDragStop(event: NodeDragEvent): Promise<void> {
  await orchestrator.setTopologyPosition(event.node.id, event.node.position);
}

/** 辺を引いたときの処理。既存の接続先へ追加してコアへ渡す。 */
async function onConnect(connection: Connection): Promise<void> {
  const source = state.agents.find((a) => a.id === connection.source);
  if (!source || !connection.target) return;

  if (source.connectedAgents.includes(connection.target)) return;
  await orchestrator.setConnections(source.id, [
    ...source.connectedAgents,
    connection.target,
  ]);
}

/**
 * 辺を切る。
 *
 * 双方向を 1 本で描いている以上、その線を切れば**両方向とも切れる**のが
 * 見た目と一致する。片方だけ残すなら、設定ダイアログの接続先チェックで行う。
 */
async function removeEdge(edge: Edge): Promise<void> {
  const forward = state.agents.find((a) => a.id === edge.source);
  const backward = edge.data?.bidirectional
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

/** ノードの縁の色を状態から決める。 */
function borderClass(status: string): string {
  switch (status) {
    case "running":
      return "border-run";
    case "failed":
      return "border-fail";
    case "starting":
    case "stopping":
      return "border-warn";
    default:
      return "border-line";
  }
}
</script>

<template>
  <div class="flex h-full flex-col">
    <!-- 高さは 4 ペイン共通の 38px 固定（AgentList のコメント参照）。 -->
    <header
      class="flex h-[38px] shrink-0 items-center gap-3 border-b border-line px-3 text-xs text-ink-dim"
    >
      <!--
        操作の説明はタイトルのホバーへ置く。**常時表示しない** — 覚えたあとは
        毎回同じ幅を占めるだけで、状態（ノード数・辺数）を読む邪魔になる。
        ヘッダに常駐してよいのは「今どうなっているか」で、「どう操作するか」ではない。
      -->
      <h2
        class="cursor-help font-semibold tracking-wide text-ink"
        title="ハンドルをドラッグで接続 / 辺をクリックで切断（双方向は両方向とも切れる）"
      >
        村の地図
      </h2>
      <span>
        {{ state.agents.length }} ノード / {{ edges.length }} 辺
        <span v-if="bidirectionalCount" class="text-ink">
          （うち双方向 {{ bidirectionalCount }}）
        </span>
      </span>
    </header>

    <div class="min-h-0 flex-1">
      <VueFlow
        :nodes="nodes"
        :edges="edges"
        :default-viewport="{ x: 0, y: 0, zoom: 0.9 }"
        :min-zoom="0.3"
        :max-zoom="2"
        fit-view-on-init
        @node-drag-stop="onNodeDragStop"
        @connect="onConnect"
        @edge-click="({ edge }) => removeEdge(edge)"
        @node-click="({ node }) => orchestrator.select(node.id)"
      >
        <!-- ノードの見た目。状態と負荷が一目で読めることだけを目的にしている。 -->
        <template #node-default="{ data }">
          <div
            class="min-w-[132px] rounded-md border-2 bg-surface-1 px-2.5 py-1.5 shadow-lg"
            :class="borderClass(data.agent.status)"
          >
            <div class="flex items-center gap-2">
              <!-- アバター。会話・一覧と同じ規則（画像 > 頭文字の円）。 -->
              <img
                v-if="state.icons[data.agent.id]"
                :src="state.icons[data.agent.id]!"
                class="size-8 shrink-0 rounded-full object-cover ring-1 ring-line"
                :alt="data.agent.name"
              />
              <div
                v-else
                class="flex size-8 shrink-0 items-center justify-center rounded-full text-[12px] font-semibold text-surface-0"
                :style="{ backgroundColor: avatarHue(data.agent.name) }"
              >
                {{ avatarInitial(data.agent.name) }}
              </div>

              <div class="min-w-0">
                <div class="truncate text-xs font-medium text-ink">
                  {{ data.agent.name }}
                </div>
                <div class="mt-0.5 truncate text-[10px] text-ink-dim">
                  {{ data.agent.model }}
                </div>
              </div>
            </div>
            <div class="mt-1 flex items-center gap-1.5 text-[10px] text-ink-dim">
              <span>{{ STATUS_LABELS[data.agent.status as keyof typeof STATUS_LABELS] }}</span>
              <span class="tabular-nums">
                {{ compactNumber(data.agent.totalTokens) }} tokens
              </span>
            </div>
          </div>
        </template>

        <Background :gap="18" :size="1" pattern-color="#3a4152" />
        <!--
          フィット（全ノードを画面に収める）だけを出す。ズームはホイール・
          ピンチで足り、操作ボタンを増やすと地図の面積を奪う（ヘッダの
          「常駐してよいのは状態だけ」と同じ判断）。
        -->
        <Controls :show-zoom="false" :show-interactive="false" />
      </VueFlow>
    </div>
  </div>
</template>

<style scoped>
:deep(.vue-flow__node.is-selected > div) {
  outline: 2px solid var(--color-accent);
  outline-offset: 2px;
}
:deep(.vue-flow__node) {
  cursor: pointer;
}

/* Controls の既定テーマは白地なので、アプリのダーク配色へ合わせる。 */
:deep(.vue-flow__controls) {
  box-shadow: none;
}
:deep(.vue-flow__controls-button) {
  width: 24px;
  height: 24px;
  background-color: var(--color-surface-1);
  border: 1px solid var(--color-line);
  border-radius: 4px;
}
:deep(.vue-flow__controls-button:hover) {
  background-color: var(--color-surface-2);
}
:deep(.vue-flow__controls-button svg) {
  fill: var(--color-ink);
}
</style>
