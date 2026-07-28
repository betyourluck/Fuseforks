<script setup lang="ts">
/**
 * 3 ペインのルートレイアウト。
 *
 * 左: エージェント一覧 / 中央: 接続マップ / 右: 会話
 *
 * エージェントの設定は常駐ペインではなくモーダル（カードの ⚙ から開く）。
 * 設定は「たまに開いて書き換えるもの」で、会話やマップのように
 * 常に見ているものではない。常駐させると、いつも見るものの面積を奪う。
 *
 * # 行・列に `minmax(0, ...)` を使う理由
 *
 * `1fr` の**最小値は `auto`**、つまり中身の最小コンテンツ幅・高さである。
 * 中身が大きいとトラックが取り分を超えて伸び、グリッド全体が `h-full` を超過して
 * 下端や右端が画面外へ押し出される。実際に、会話が伸びると送信欄が画面外へ沈み、
 * 入力できなくなった。`minmax(0, 1fr)` で最小値を 0 に固定し、
 * はみ出しは各ペインの内部スクロールに引き受けさせる。
 */
import { computed, onMounted, ref } from "vue";

import AgentList from "./components/AgentList.vue";
import ChatPanel from "./components/ChatPanel.vue";
import ErrorBoundary from "./components/ErrorBoundary.vue";
import McpDialog from "./components/McpDialog.vue";
import OrdinanceDialog from "./components/OrdinanceDialog.vue";
import PaneSplitter from "./components/PaneSplitter.vue";
import TitleBar from "./components/TitleBar.vue";
import ToastHost from "./components/ToastHost.vue";
import TopologyMap from "./components/TopologyMap.vue";
import { useOrchestrator } from "./composables/useOrchestrator";
import { usePaneLayout } from "./composables/usePaneLayout";

const orchestrator = useOrchestrator();
const { state } = orchestrator;

const { layout, resize, reset } = usePaneLayout();

/** 村の条例ダイアログの表示状態。 */
const ordinanceOpen = ref(false);
/** MCP サーバー管理ダイアログの表示状態。 */
const mcpOpen = ref(false);

const columns = computed(
  () => `${layout.leftWidth}px 2px minmax(0, 1fr) 2px ${layout.rightWidth}px`,
);

onMounted(() => {
  void orchestrator.init();
});
</script>

<template>
  <div class="flex h-full flex-col overflow-hidden bg-surface-0 text-ink">
  <TitleBar @open-ordinance="ordinanceOpen = true" @open-mcp="mcpOpen = true" />
  <div
    class="grid min-h-0 flex-1 overflow-hidden"
    :style="{ gridTemplateColumns: columns }"
  >
    <!--
      各区画をエラー境界で包む。1 区画の描画失敗がアプリ全体を白紙にすると、
      再起動するまで何も読めなくなる（会話ログが消えて再起動が要る、という形で
      実際に起きた）。落ちた区画だけを差し替え、残りは生かす。
    -->

    <!-- 左ペイン: エージェント一覧 -->
    <aside class="min-w-0 overflow-hidden">
      <ErrorBoundary label="エージェント一覧">
        <AgentList />
      </ErrorBoundary>
    </aside>

    <PaneSplitter
      direction="col"
      label="エージェント一覧の幅"
      @delta="(px) => resize('leftWidth', px)"
      @reset="reset"
    />

    <!-- 中央ペイン: 接続マップ -->
    <main class="min-w-0 overflow-hidden">
      <ErrorBoundary label="接続マップ">
        <TopologyMap />
      </ErrorBoundary>
    </main>

    <!-- 右ペインは左端につまみがあるので、右へ動かすと幅が縮む。 -->
    <PaneSplitter
      direction="col"
      label="会話パネルの幅"
      @delta="(px) => resize('rightWidth', px, -1)"
      @reset="reset"
    />

    <!-- 右ペイン: 会話 -->
    <aside class="min-w-0 overflow-hidden">
      <ErrorBoundary label="会話">
        <ChatPanel />
      </ErrorBoundary>
    </aside>

    <ToastHost />

    <OrdinanceDialog v-if="ordinanceOpen" @close="ordinanceOpen = false" />

    <McpDialog v-if="mcpOpen" @close="mcpOpen = false" />

    <!--
      初期化中の覆い。空の 3 ペインを見せて「壊れている」と誤解させない。
      ただし失敗したときは覆いのまま据え置かない。読み込み中と初期化失敗が
      同じ見た目になると、待てば直るのか壊れているのかを区別する手段が消える。
    -->
    <div
      v-if="!state.ready"
      class="fixed inset-0 z-50 flex flex-col items-center justify-center gap-3 bg-surface-0 px-8 text-center"
    >
      <template v-if="state.initError">
        <p class="font-medium text-fail">オーケストレーターの起動に失敗しました</p>
        <p class="selectable max-w-lg text-[12px] text-ink-dim">
          [{{ state.initError.code }}] {{ state.initError.message }}
        </p>
        <p
          v-if="state.initError.detail"
          class="selectable max-w-lg text-[11px] text-ink-dim opacity-70"
        >
          {{ state.initError.detail }}
        </p>
        <button
          class="mt-2 rounded bg-accent px-4 py-1.5 text-[12px] font-medium text-surface-0"
          @click="orchestrator.init()"
        >
          再試行
        </button>
      </template>
      <p v-else class="text-ink-dim">オーケストレーターを起動しています…</p>
    </div>
  </div>
  </div>
</template>
