<script setup lang="ts">
/**
 * 中央ペイン上部: ノードベースのサーヴァントの絆（旧「村の地図」）。
 *
 * トポロジーの真実は「どの辺があるか」だけであり、座標は world.json に保存する
 * 表示設定として分ける。未配置ノードだけはここで円環状に自動配置する。
 *
 * 辺の追加・削除はグラフ上の操作をそのままコアへ流す。
 * 自己ループと未登録先はコア側が拒否するので、ここでは事前検査しない
 * （検査を二重に持つと、片方だけ直したときに規則が食い違う）。
 */
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import {
  VueFlow,
  useVueFlow,
  type Connection,
  type Edge,
  type Node,
  type NodeDragEvent,
} from "@vue-flow/core";
import { Background } from "@vue-flow/background";
import { Controls } from "@vue-flow/controls";

import { compactNumber } from "../lib/format";
import { tieAddition } from "../lib/kizunaDrop";
import { drawDirection } from "../lib/kizunaEdges";
import { roleBadge } from "../lib/roleLabel";

import { avatarHue, avatarInitial } from "../lib/avatar";
import { askConfirm } from "../composables/useConfirm";
import { useOrchestrator } from "../composables/useOrchestrator";
import { useUiSettings } from "../composables/useUiSettings";
import { STATUS_LABEL_KEYS, type AgentId } from "../types";

const { t } = useI18n();

const orchestrator = useOrchestrator();
const { state } = orchestrator;
const { settings } = useUiSettings();
const { fitView } = useVueFlow();

/**
 * ペインの大きさが変わった後に Fit を掛け直す（2026-08-08 利用者要望）。
 *
 * **見ているのはコンテナの箱だけ。** ノードの移動でも辺の増減でも発火しない —
 * ドラッグ中に視点が動くと Spec 21 の drop が使う `elementFromPoint` の座標と
 * 画面がずれる。**Vue Flow に「追従し続ける」プロパティは無い**ので
 * （1.48.2 は初期化時 1 回の `fitViewOnInit` と命令的な `fitView()` だけ）、
 * リサイズという 1 点に絞って自前で繋ぐ。
 */
const canvas = ref<HTMLElement | null>(null);
let observer: ResizeObserver | null = null;
let settle: ReturnType<typeof setTimeout> | null = null;

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
    fitView();
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

/**
 * ノードに出す役職名。引けなければ `null` で**バッジごと描かない**
 * （`role_contract` 凍結 5 — 3 箇所は `roleLabel` の 1 実装を通す）。
 */
function roleOf(agent: { roleId: string | null }) {
  return roleBadge(agent.roleId, state.roles);
}

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

  /**
   * 左ペインでの並び順。**引けなければ末尾へ**（消えた個体の辺が残っていても
   * 描画順が揺れないように、`NaN` を作らない）。
   */
  const orderOf = (id: AgentId) =>
    state.agents.find((a) => a.id === id)?.order ?? Number.MAX_SAFE_INTEGER;

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

    // 描画方向は左ペインの並びに合わせる（規則と根拠は `lib/kizunaEdges.ts`）。
    const [source, target] = drawDirection(
      edge.source,
      edge.target,
      bidirectional,
      orderOf,
    );

    result.push({
      id,
      source,
      target,
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

/**
 * 辺を引いたときの処理。追加の規則は lib/kizunaDrop の 1 実装
 * （リストからのカード drop と共有。Spec 21）。
 */
async function onConnect(connection: Connection): Promise<void> {
  if (!connection.target) return;
  const next = tieAddition(state.agents, connection.source, connection.target);
  if (!next) return;
  await orchestrator.setConnections(connection.source, next);
}

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
 * 線は棚卸しで**唯一、確認なしで消える破壊的操作**だった（Spec 13 S4 —
 * 誤クリックで失った実害報告あり。接続は元に戻せない）。確認は既定 ON で、
 * システム設定「線削除の確認」から切れる。
 */
async function removeEdge(edge: Edge): Promise<void> {
  if (settings.confirmEdgeDelete) {
    const arrow = edge.data?.bidirectional ? "⇄" : "→";
    const ok = await askConfirm({
      title: t("map.confirmDeleteTitle"),
      message:
        `${agentLabel(edge.source)} ${arrow} ${agentLabel(edge.target)}` +
        (edge.data?.bidirectional ? `\n${t("map.bidirectionalNote")}` : ""),
      confirmLabel: t("map.confirmDeleteLabel"),
      danger: true,
    });
    if (!ok) return;
  }

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
        :title="$t('map.headingHelp')"
      >
        {{ $t("map.heading") }}
      </h2>
      <span>
        {{ $t("map.summary", { nodes: state.agents.length, edges: edges.length }) }}
        <span v-if="bidirectionalCount" class="text-ink">
          {{ $t("map.bidirectional", { count: bidirectionalCount }) }}
        </span>
      </span>
    </header>

    <div ref="canvas" class="min-h-0 flex-1">
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
              <!--
                役職バッジ（Spec 14）。名前行ではなくこの行へ置く — 名前とモデル名は
                どちらも truncate で幅を奪い合っており、3 つ目を足すと全部読めなくなる。
                この行は元からチップの並びなので、短い語（3〜5 字）が収まる。
                **状態より前**に置くのは、役職が「誰か」の側で状態が「今どうか」の側だから。
              -->
              <span
                v-if="roleOf(data.agent)"
                class="rounded border px-1 py-px leading-none"
                :class="roleOf(data.agent)!.color ? '' : 'border-line'"
                :style="
                  roleOf(data.agent)!.color
                    ? { borderColor: roleOf(data.agent)!.color, color: roleOf(data.agent)!.color }
                    : undefined
                "
                :title="$t('roles.badgeTitle', { name: roleOf(data.agent)!.name })"
              >
                {{ roleOf(data.agent)!.name }}
              </span>
              <span>{{ $t(STATUS_LABEL_KEYS[data.agent.status as keyof typeof STATUS_LABEL_KEYS]) }}</span>
              <span class="tabular-nums">
                {{ compactNumber(data.agent.totalTokens) }} tokens
              </span>
            </div>
          </div>
        </template>

        <!-- 点の色は style.css が与える（SVG の属性に var() は書けないため）。 -->
        <Background :gap="18" :size="1" />
        <!--
          フィット（全ノードを画面に収める）だけを出す。ズームはホイール・
          ピンチで足り、操作ボタンを増やすと地図の面積を奪う（ヘッダの
          「常駐してよいのは状態だけ」と同じ判断）。
        -->
        <Controls :show-zoom="false" :show-interactive="false">
          <!--
            自動 Fit のトグルは **Fit の下**（既定スロットは組み込みボタンの
            後に描かれる。`#top` は Fit の上になる）。ボタンを増やす判断が
            上の Controls のコメントと逆向きに見えるが、これは**操作ではなく
            状態の切り替え**で、押した後は押さないもの。
          -->
          <template #default>
            <button
              class="vue-flow__controls-button"
              :class="{ 'is-on': settings.autoFitOnResize }"
              :title="$t('map.autoFit')"
              :aria-label="$t('map.autoFit')"
              :aria-pressed="settings.autoFitOnResize"
              @click="settings.autoFitOnResize = !settings.autoFitOnResize"
            >
              <!--
                **`fill` は `currentColor`。** 受け取った原本は `#212121` 固定で、
                ライトテーマでもダークテーマでも同じ黒になり、`is-on` の accent も
                効かない（絵文字を恒久要素に使わない理由と同じ — 配色に追従しない
                図形を置かない）。
              -->
              <svg viewBox="0 0 20 20" fill="currentColor" aria-hidden="true">
                <path
                  d="M15.0027 4.49838L15.7121 5.23289C15.9998 5.53084 16.4746 5.53911 16.7726 5.25137C17.0705 4.96362 17.0788 4.48882 16.7911 4.19087L14.9701 2.30532C14.577 1.89823 13.9246 1.89823 13.5315 2.30532L11.7105 4.19087C11.4228 4.48882 11.431 4.96362 11.729 5.25137C12.0269 5.53911 12.5017 5.53084 12.7895 5.23289L13.5027 4.49433V7.25C13.5027 7.66421 13.8385 8 14.2527 8C14.667 8 15.0027 7.66421 15.0027 7.25V4.49838ZM3 5C3 3.89543 3.89543 3 5 3H9.25C9.66421 3 10 3.33579 10 3.75C10 4.16421 9.66421 4.5 9.25 4.5H5C4.72386 4.5 4.5 4.72386 4.5 5V15C4.5 15.2761 4.72386 15.5 5 15.5H9.25C9.66421 15.5 10 15.8358 10 16.25C10 16.6642 9.66421 17 9.25 17H5C3.89543 17 3 16.1046 3 15V5ZM15.7121 14.7671L15.0027 15.5016V12.75C15.0027 12.3358 14.667 12 14.2527 12C13.8385 12 13.5027 12.3358 13.5027 12.75V15.5057L12.7895 14.7671C12.5017 14.4692 12.0269 14.4609 11.729 14.7486C11.431 15.0364 11.4228 15.5112 11.7105 15.8091L13.5315 17.6947C13.9246 18.1018 14.577 18.1018 14.9701 17.6947L16.7911 15.8091C17.0788 15.5112 17.0705 15.0364 16.7726 14.7486C16.4746 14.4609 15.9998 14.4692 15.7121 14.7671Z"
                />
              </svg>
            </button>
          </template>
        </Controls>
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

/* 自動 Fit が入っているときだけ、枠と字を accent で示す（押した状態の印）。 */
.vue-flow__controls-button.is-on {
  border-color: var(--color-accent);
  color: var(--color-accent);
}
:deep(.vue-flow__controls-button:hover) {
  background-color: var(--color-surface-2);
}
:deep(.vue-flow__controls-button svg) {
  fill: var(--color-ink);
}
</style>
