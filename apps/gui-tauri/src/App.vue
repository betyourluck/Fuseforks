<script setup lang="ts">
/**
 * 3 ペインのルートレイアウト。
 *
 * 左: エージェント一覧 / 中央: トポロジー + 会話ログ / 右: 設定とエディタ
 *
 * グリッドの列幅を固定値にしているのは、ペインごとに情報の密度が違うため。
 * 左は一覧、右はフォームで、どちらも横に伸びても読みやすくならない。
 * 中央のグラフだけが面積を必要とするので、そこに残りを全部渡す。
 */
import { onMounted } from "vue";

import AgentList from "./components/AgentList.vue";
import InspectorPanel from "./components/InspectorPanel.vue";
import MessageLog from "./components/MessageLog.vue";
import ToastHost from "./components/ToastHost.vue";
import TopologyMap from "./components/TopologyMap.vue";
import { useOrchestrator } from "./composables/useOrchestrator";

const orchestrator = useOrchestrator();
const { state } = orchestrator;

onMounted(() => {
  void orchestrator.init();
});
</script>

<template>
  <div class="grid h-full grid-cols-[320px_1fr_380px] bg-surface-0 text-ink">
    <!-- 左ペイン: エージェント一覧 -->
    <aside class="min-w-0 border-r border-line">
      <AgentList />
    </aside>

    <!-- 中央ペイン: グラフィカルマップ（上） + 会話ログ（下） -->
    <main class="grid min-w-0 grid-rows-[1fr_minmax(200px,34%)]">
      <section class="min-h-0 border-b border-line">
        <TopologyMap />
      </section>
      <section class="min-h-0">
        <MessageLog />
      </section>
    </main>

    <!-- 右ペイン: 設定とエディタ -->
    <aside class="min-w-0 border-l border-line">
      <InspectorPanel />
    </aside>

    <ToastHost />

    <!-- 初期化中の覆い。空の 3 ペインを見せて「壊れている」と誤解させない。 -->
    <div
      v-if="!state.ready"
      class="fixed inset-0 z-50 flex items-center justify-center bg-surface-0 text-ink-dim"
    >
      オーケストレーターを起動しています…
    </div>
  </div>
</template>
