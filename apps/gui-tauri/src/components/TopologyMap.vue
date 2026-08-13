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

import { compactNumber } from "../lib/format";
import { drawDirection } from "../lib/kizunaEdges";
import { roleBadge } from "../lib/roleLabel";
import { seedPositions } from "../lib/kizunaSeed";
import { avatarHue, avatarInitial } from "../lib/avatar";
import { askConfirm } from "../composables/useConfirm";
import { useOrchestrator } from "../composables/useOrchestrator";
import { useUiSettings } from "../composables/useUiSettings";
import { STATUS_LABEL_KEYS, type AgentId } from "../types";

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
    // **`selected` を載せるのは `configs` が読むため。** 選択で半径が変わるので、
    // ここを持たせないと辺の端が旧い半径のまま刺さる。
    result[agent.id] = { name: agent.name, selected: agent.id === state.selectedAgentId };
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

/**
 * 点描の背景を視点へ追従させるための値。
 *
 * **SVG の中へ矩形を敷く形は採れない。** `grid` より上のレイヤーは視点の変換が
 * 掛かるので追従はするが、**`fitToContents` が内容の外接矩形にその矩形を含めて
 * しまい、変な位置に収まる**（実機で踏んだ）。かといって `background` レイヤーは
 * 画面固定でパンに追従しない。**どちらの層でも成立しない。**
 *
 * so 点は容器の CSS 背景として描き、パンとズームを自分で反映する。
 * こうすると内容の外接矩形に一切影響しない。
 */
const zoom = ref(1);
const pan = ref({ x: 0, y: 0 });

/** 点の間隔と大きさは旧 `<Background>` と同じ（18 / 1）。 */
const DOT_GAP = 18;
const DOT_RADIUS = 1;

const dotStyle = computed(() => {
  const gap = DOT_GAP * zoom.value;
  return {
    "--dot-r": `${DOT_RADIUS * zoom.value}px`,
    backgroundSize: `${gap}px ${gap}px`,
    backgroundPosition: `${pan.value.x}px ${pan.value.y}px`,
  };
});

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
    // **選択中は 1.5 倍。** ここで変えると辺の接続点も一緒に動く
    // （見た目だけ CSS で拡大すると、線がノードの内側へ食い込む）。
    normal: { type: "circle", radius: (node) => (node.selected ? NODE_RADIUS * 1.5 : NODE_RADIUS) },
    hover: { type: "circle", radius: (node) => (node.selected ? NODE_RADIUS * 1.5 : NODE_RADIUS) },
    // 名前はスロットで描くので、組み込みのラベルは出さない。
    label: { visible: false },
    focusring: { visible: false },
  },
  edge: {
    selectable: true,
    normal: {
      width: (edge) => (edge.bidirectional ? 3 : 1.6),
      // **稼働中の個体から出ている辺は動く破線**（旧実装の踏襲）。
      // `animate` だけでは足りない — 破線でない線を流しても見た目が変わらない
      // ので、`dasharray` と対で与える。実機で「動かない」と出たのがこれ。
      dasharray: (edge) => (edgeIsLive(edge as never) ? 6 : undefined),
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

/**
 * 右上のパネルに出す個体。**マウスを載せている間だけ**（2026-08-13 利用者裁定）。
 *
 * 選択で出しっぱなしにしていたが、**常に何かが浮いている**状態になり、
 * 重ねる意味（要るときだけ見える）が薄れていた。ホバーなら「見たいときだけ」が
 * 操作そのもので表せる。
 *
 * **`state.agents` から引き直す**ので、載せたまま個体が消えてもパネルは
 * 自動で閉じる（id を持ったまま古い値を描き続けない）。
 */
const hovered = ref<AgentId | null>(null);
const detail = computed(
  () => state.agents.find((a) => a.id === hovered.value) ?? null,
);

/**
 * パネルに出す役職名。引けなければ `null` で**バッジごと描かない**
 * （`role_contract` 凍結 5 — 表示の解決は `roleLabel` の 1 実装を通す）。
 */
function roleOf(agent: { roleId: string | null }) {
  return roleBadge(agent.roleId, state.roles);
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
  "node:pointerover": ({ node }) => {
    hovered.value = node as AgentId;
  },
  "node:pointerout": () => {
    hovered.value = null;
  },
  // 点描は容器の CSS 背景なので、視点の変化をこちらで写す。
  "view:zoom": (level) => {
    zoom.value = level;
  },
  "view:pan": (position) => {
    pan.value = { x: position.x, y: position.y };
  },
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
      <span class="font-medium text-ink">{{ $t("map.heading") }}</span>
      <span>
        {{ $t("map.summary", { nodes: state.agents.length, edges: Object.keys(edges).length }) }}
        <span v-if="bidirectionalCount" class="text-ink">
          {{ $t("map.bidirectional", { count: bidirectionalCount }) }}
        </span>
      </span>

    </header>

    <div ref="canvas" class="kizuna relative min-h-0 flex-1" :style="dotStyle">
      <!--
        選択中の個体の詳細。**ノードから外した 3 つ（役職・モデル・トークン）の
        行き先**で、地図の上に重ねる（2026-08-13 利用者要望）。

        **選んでいなければ出さない。**「未選択です」と書くと、常に何かが浮いて
        いる状態になり、重ねる意味（要るときだけ見える）が消える。

        擦りガラスは `@` 補完の候補と同じ作法 — トークンにアルファを掛けるので
        `color-mix` へ展開され、**ライトテーマの上書きに実行時で追従する**。

        `pointer-events-none` にするのは、**下のノードを掴む邪魔をしないため**。
        右上はノードが来うる場所で、情報のために操作を殺すのは交換として合わない。
      -->
      <aside
        v-if="detail"
        class="pointer-events-none absolute right-2 top-2 z-10 w-60 rounded-md bg-surface-1/25 px-3 py-2 text-sm backdrop-blur-sm"
      >
        <div class="flex items-center gap-2">
          <span class="truncate text-base font-semibold text-ink">{{ detail.name }}</span>
          <!-- 役職バッジ（Spec 14）。引けなければ**バッジごと描かない**。 -->
          <span
            v-if="roleOf(detail)"
            class="shrink-0 rounded border px-1 py-px leading-none"
            :class="roleOf(detail)!.color ? '' : 'border-line'"
            :style="
              roleOf(detail)!.color
                ? { borderColor: roleOf(detail)!.color, color: roleOf(detail)!.color }
                : undefined
            "
          >
            {{ roleOf(detail)!.name }}
          </span>
        </div>

        <div class="mt-1 truncate text-ink-dim">{{ detail.id }}</div>

        <dl class="mt-2 grid grid-cols-[auto_1fr] gap-x-2 gap-y-1 text-ink-dim">
          <dt>{{ $t("agentCard.model") }}</dt>
          <dd class="truncate text-ink">{{ detail.model }}</dd>

          <dt>{{ $t("map.status") }}</dt>
          <dd class="text-ink">
            {{ $t(STATUS_LABEL_KEYS[detail.status as keyof typeof STATUS_LABEL_KEYS]) }}
          </dd>

          <dt>{{ $t("agentCard.tokens") }}</dt>
          <dd class="tabular-nums text-ink">{{ compactNumber(detail.totalTokens) }}</dd>

          <dt>{{ $t("agentCard.connections") }}</dt>
          <dd class="tabular-nums text-ink">
            {{ $t("agentCard.connectionsCount", { count: detail.connectedAgents.length }) }}
          </dd>
        </dl>
      </aside>

      <!--
        表示の操作（左下）。**Vue Flow の `Controls` と同じ位置と作法**に戻した
        （2026-08-13 利用者要望）。ヘッダへ文字のボタンとして置いていたが、
        あそこは村の要約を出す場所で、操作を混ぜると読む面と押す面が混ざる。

        自動 Fit が Fit の**下**なのは元と同じ並び。ボタンを増やす判断は
        「操作ボタンは地図の面積を奪う」と逆向きに見えるが、これは**操作ではなく
        状態の切り替え**で、押した後は押さないもの。
      -->
      <div
        class="kizuna-controls absolute bottom-2 left-2 z-10 flex flex-col overflow-hidden rounded border border-line bg-surface-1/80 backdrop-blur"
      >
        <button type="button" :title="$t('map.fitTitle')" @click="fit">
          <!-- 四隅へ広げる = 全体を入れる。 -->
          <svg viewBox="0 0 20 20" fill="currentColor" aria-hidden="true">
            <path
              d="M3 7.75A.75.75 0 0 1 2.25 7V4A1.75 1.75 0 0 1 4 2.25h3a.75.75 0 0 1 0 1.5H4a.25.25 0 0 0-.25.25v3A.75.75 0 0 1 3 7.75Zm14 0a.75.75 0 0 1-.75-.75V4a.25.25 0 0 0-.25-.25h-3a.75.75 0 0 1 0-1.5h3A1.75 1.75 0 0 1 17.75 4v3a.75.75 0 0 1-.75.75Zm-14 4.5a.75.75 0 0 1 .75.75v3c0 .138.112.25.25.25h3a.75.75 0 0 1 0 1.5H4A1.75 1.75 0 0 1 2.25 16v-3a.75.75 0 0 1 .75-.75Zm14 0a.75.75 0 0 1 .75.75v3A1.75 1.75 0 0 1 16 17.75h-3a.75.75 0 0 1 0-1.5h3a.25.25 0 0 0 .25-.25v-3a.75.75 0 0 1 .75-.75Z"
            />
          </svg>
        </button>
        <button
          type="button"
          :class="{ 'is-on': settings.autoFitOnResize }"
          :title="$t('map.autoFit')"
          @click="settings.autoFitOnResize = !settings.autoFitOnResize"
        >
          <!--
            利用者提供の SVG。**`fill` は `currentColor` へ直してある** — 原本は
            `#212121` 固定で、ライトでもダークでも同じ黒になり `is-on` の accent も
            効かなかった（絵文字を恒久要素に使わない理由と同じ線引き）。
          -->
          <svg viewBox="0 0 20 20" fill="currentColor" aria-hidden="true">
            <path
              d="M15.0027 4.49838L15.7121 5.23289C15.9998 5.53084 16.4746 5.53911 16.7726 5.25137C17.0705 4.96362 17.0788 4.48882 16.7911 4.19087L14.9701 2.30532C14.577 1.89823 13.9246 1.89823 13.5315 2.30532L11.7105 4.19087C11.4228 4.48882 11.431 4.96362 11.729 5.25137C12.0269 5.53911 12.5017 5.53084 12.7895 5.23289L13.5027 4.49433V7.25C13.5027 7.66421 13.8385 8 14.2527 8C14.667 8 15.0027 7.66421 15.0027 7.25V4.49838ZM3 5C3 3.89543 3.89543 3 5 3H9.25C9.66421 3 10 3.33579 10 3.75C10 4.16421 9.66421 4.5 9.25 4.5H5C4.72386 4.5 4.5 4.72386 4.5 5V15C4.5 15.2761 4.72386 15.5 5 15.5H9.25C9.66421 15.5 10 15.8358 10 16.25C10 16.6642 9.66421 17 9.25 17H5C3.89543 17 3 16.1046 3 15V5ZM15.7121 14.7671L15.0027 15.5016V12.75C15.0027 12.3358 14.667 12 14.2527 12C13.8385 12 13.5027 12.3358 13.5027 12.75V15.5057L12.7895 14.7671C12.5017 14.4692 12.0269 14.4609 11.729 14.7486C11.431 15.0364 11.4228 15.5112 11.7105 15.8091L13.5315 17.6947C13.9246 18.1018 14.577 18.1018 14.9701 17.6947L16.7911 15.8091C17.0788 15.5112 17.0705 15.0364 16.7726 14.7486C16.4746 14.4609 15.9998 14.4692 15.7121 14.7671Z"
            />
          </svg>
        </button>
      </div>

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
        <template #override-node="{ nodeId, scale, config }">
          <g :data-kizuna-node="nodeId" :class="['kizuna-node', ringClass(nodeId as AgentId)]">
            <circle class="kizuna-ring" :r="config.radius * scale" />

            <image
              v-if="state.icons[nodeId as AgentId]"
              :href="state.icons[nodeId as AgentId]!"
              :x="-config.radius * scale"
              :y="-config.radius * scale"
              :width="config.radius * 2 * scale"
              :height="config.radius * 2 * scale"
              clip-path="url(#kizuna-avatar)"
              preserveAspectRatio="xMidYMid slice"
            />
            <template v-else>
              <circle
                :r="(config.radius - 2) * scale"
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
              :cx="config.radius * 0.72 * scale"
              :cy="-config.radius * 0.72 * scale"
              :r="5 * scale"
            />

            <text
              class="kizuna-name"
              text-anchor="middle"
              :y="(config.radius + 14) * scale"
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
  stroke-width: 4;
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
  stroke-width: 8;
}

/*
 * 選択したときの弾み。
 *
 * **大きさそのものは `configs.node.normal.radius` が 1.5 倍にしている** —
 * ここでやるのは行き過ぎて戻る演出だけ。半径を CSS の拡大で作ると
 * **辺の接続点が旧い半径のまま**になり、線がノードの内側へ食い込む。
 *
 * 起点を 0.667（= 1 / 1.5）にしてあるので、**前の大きさから膨らむ**ように見える。
 * 原点はノードの中心（スロットの (0,0) が中心）なので `transform-origin` は要らない。
 */
.kizuna :deep(.is-selected) {
  animation: kizuna-bounce 0.42s cubic-bezier(0.34, 1.56, 0.64, 1);
}
@keyframes kizuna-bounce {
  0% {
    transform: scale(0.667);
  }
  60% {
    transform: scale(1.1);
  }
  80% {
    transform: scale(0.97);
  }
  100% {
    transform: scale(1);
  }
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

/*
 * 点描の背景。旧 `<Background :gap="18" :size="1" />` の見え方をそのまま写す。
 * 位置と大きさは `dotStyle` が視点から与える。
 */
.kizuna {
  background-image: radial-gradient(
    var(--color-line) var(--dot-r, 1px),
    transparent var(--dot-r, 1px)
  );
}

.kizuna :deep(.v-ng-edge) {
  stroke: var(--color-accent);
}

/*
 * 左下の操作パネル。旧 `Controls` の寸法（24px 角）をそのまま踏襲する —
 * 位置も大きさも変わると、慣れた手が迷う。
 */
.kizuna-controls button {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  color: var(--color-ink);
}
.kizuna-controls button + button {
  border-top: 1px solid var(--color-line);
}
.kizuna-controls button:hover {
  background-color: var(--color-surface-2);
}
.kizuna-controls button svg {
  width: 14px;
  height: 14px;
}
/* 自動 Fit が入っているときだけ accent で示す（押した状態の印）。 */
.kizuna-controls button.is-on {
  color: var(--color-accent);
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
