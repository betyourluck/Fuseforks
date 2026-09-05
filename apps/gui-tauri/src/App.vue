<script setup lang="ts">
/**
 * 3 ペインのルートレイアウト。
 *
 * 上: タイトルバー / 左: エージェント一覧 / 中央: サーヴァントの絆 / 右: 会話 /
 * 下: ステータスバー（時計）
 *
 * エージェントの設定は常駐ペインではなくモーダル（カードの鉛筆から開く）。
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
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { getCurrentWindow } from "@tauri-apps/api/window";

import AgentList from "./components/AgentList.vue";
import BlackboardPane from "./components/BlackboardPane.vue";
import ChatPanel from "./components/ChatPanel.vue";
import ErrorBoundary from "./components/ErrorBoundary.vue";
import McpDialog from "./components/McpDialog.vue";
import OrdinanceDialog from "./components/OrdinanceDialog.vue";
import PlanWavePane from "./components/PlanWavePane.vue";
import ScheduleDialog from "./components/ScheduleDialog.vue";
import SettingsDialog from "./components/SettingsDialog.vue";
import StatsView from "./components/StatsView.vue";
import CommandApprovalDialog from "./components/CommandApprovalDialog.vue";
import RoleDialog from "./components/RoleDialog.vue";
import { settleSelection, visibleAgents } from "./lib/agentGroups";
import { useHiddenGroups } from "./composables/useHiddenGroups";
import PaneSplitter from "./components/PaneSplitter.vue";
import StatusBar from "./components/StatusBar.vue";
import TitleBar from "./components/TitleBar.vue";
import ConfirmHost from "./components/ConfirmHost.vue";
import ToastHost from "./components/ToastHost.vue";
import TopologyMap from "./components/TopologyMap.vue";
import { agentNavDelta, isNavigableFocus, nextAgentId } from "./lib/agentNav";
import { closeConfirmLines } from "./lib/closeConfirm";
import { formatError } from "./lib/errorText";
import { askConfirm } from "./composables/useConfirm";
import { useOrchestrator } from "./composables/useOrchestrator";
import { usePaneLayout } from "./composables/usePaneLayout";
import { useUiSettings } from "./composables/useUiSettings";
import type { BottomTab } from "./types";

const { t } = useI18n();
const orchestrator = useOrchestrator();
const { state } = orchestrator;
const hiddenGroups = useHiddenGroups();

/**
 * 選択の落ち着き先（Spec 51 D3）。隠した瞬間に選択中だった個体は見えている先頭へ、
 * 可視 0 体なら `null`（= 空の村と同じ状態。宛先欄は「宛先を選択してください」）。
 * 個体・グループ・非表示の集合のどれが動いても同じ規則で数え直す —
 * `useOrchestrator` の「選択が一覧に無ければ先頭」を可視の集合に対して行う形。
 */
watch(
  () =>
    [
      state.agents.map((a) => `${a.id}:${a.groupId ?? ""}`).join(","),
      state.groups.map((g) => g.id).join(","),
      hiddenGroups.hidden.join(","),
    ] as const,
  () => {
    const visible = visibleAgents(state.agents, state.groups, hiddenGroups.hidden);
    const next = settleSelection(state.selectedAgentId, visible);
    if (next !== state.selectedAgentId) orchestrator.select(next);
  },
);
const { settings } = useUiSettings();

const { layout, resize, reset } = usePaneLayout();

/**
 * 中央下段のタブ。既定は作業状況（タブ化以前からの表示を保つ）。
 * 保存しない — タブは「いま見たいもの」で、レイアウト寸法と違い次回も
 * 同じとは限らない。
 */
const bottomTab = ref<BottomTab>("waves");
/**
 * 全画面の切り替え（Spec 39 D4）。`"village"` = 3 ペイン / `"stats"` = 統計画面。
 * **保存しない**（`bottomTab` と同じ）— 起動は必ず村から。「開いただけで統計が
 * 出る」形にすると、次に来た人が村の状態を見る前に数字を見る。
 */
const view = ref<"village" | "stats">("village");

/** 村の条例ダイアログの表示状態。 */
const ordinanceOpen = ref(false);
/** 役職ダイアログの表示状態（Spec 14）。条例と同じ「村の内容物」の棚。 */
const rolesOpen = ref(false);
const commandApprovalOpen = ref(false);
/** MCP サーバー管理ダイアログの表示状態。 */
const mcpOpen = ref(false);
/** 予定（スケジュール実行）ダイアログの表示状態。 */
const schedulesOpen = ref(false);
/** システム設定ダイアログの表示状態（Spec 13）。 */
const settingsOpen = ref(false);

const columns = computed(
  () => `${layout.leftWidth}px 2px minmax(0, 1fr) 2px ${layout.rightWidth}px`,
);

/** 中央ペインの上下分割（Spec 08）。上段 = サーヴァントの絆 / 下段 = 黒板・作業状況のタブ。 */
const centerRows = computed(
  () => `minmax(0, 1fr) 2px ${layout.bottomHeight}px`,
);

/**
 * 閉じる前の確認（2026-08-08 利用者要望）。**既定 ON で、ON なら常に聞く。**
 *
 * `preventDefault()` のあと **`destroy()` を呼ぶ** — `close()` だと
 * 同じイベントがもう一度発火して**無限ループ**になる
 * （capability に `core:window:allow-destroy` を足したのはこのため）。
 *
 * ここに置くのは、**中身が村の状態を要る**から（稼働中の数・扉の開閉）。
 * 判定は純関数（`closeConfirmLines`）に出し、ここは訳語を引いて繋ぐだけ。
 */
async function registerCloseGuard(): Promise<void> {
  const win = getCurrentWindow();
  await win.onCloseRequested(async (event) => {
    if (!settings.confirmClose) return;
    event.preventDefault();

    const lines = closeConfirmLines({
      runningAgents: state.agents.filter((a) => a.status === "running").length,
      mcpListening: state.mcpHost?.listening === true,
    });
    const ok = await askConfirm({
      title: t("closeConfirm.title"),
      message: lines.map((line) => t(line.key, line.params ?? {})).join("\n"),
      confirmLabel: t("closeConfirm.confirm"),
      cancelLabel: t("closeConfirm.cancel"),
      // **飛行中のターンは戻らない。** 初期フォーカスを取り消し側へ寄せる。
      danger: true,
    });
    if (ok) await win.destroy();
  });
}

/**
 * Alt + ↑↓ でサーヴァント一覧の選択を移す（マイルストーン 8）。
 *
 * **窓ごと聴くのは、入力欄にフォーカスがあるときも効かせるため**（利用者の要件 —
 * 「入力欄にフォーカスしている状態で切り替えられないと、あまり意味がない」）。
 * ゆえに**どこで効かせないか**は `isNavigableFocus` の閉じた許容が決める。
 *
 * 規則は `lib/agentNav.ts` の純関数 3 本で、ここは配線だけ。
 */
function onNavKey(event: KeyboardEvent): void {
  const delta = agentNavDelta(event);
  if (delta === null) return;
  if (!isNavigableFocus(document.activeElement)) return;

  // 隠したグループの個体は巡らない（Spec 51 D3。可視 0 体なら null で何もしない）。
  const next = nextAgentId(
    visibleAgents(state.agents, state.groups, hiddenGroups.hidden),
    state.selectedAgentId,
    delta,
  );
  if (next === null) return;
  // 入力欄では Alt+↑↓ が段落移動になる環境があるので、拾ったら必ず止める。
  event.preventDefault();
  orchestrator.select(next);
}

onMounted(() => {
  void orchestrator.init();
  // 失敗しても画面は使える（確認が付かないだけ）。閉じられなくなるほうが害が大きい。
  void registerCloseGuard().catch(() => undefined);
  window.addEventListener("keydown", onNavKey);
});

onBeforeUnmount(() => window.removeEventListener("keydown", onNavKey));
</script>

<template>
  <div class="flex h-full flex-col overflow-hidden bg-surface-0 text-ink">
  <TitleBar
    :stats-active="view === 'stats'"
    @open-ordinance="ordinanceOpen = true"
    @open-roles="rolesOpen = true"
    @open-command-approval="commandApprovalOpen = true"
    @open-mcp="mcpOpen = true"
    @open-schedules="schedulesOpen = true"
    @open-settings="settingsOpen = true"
  />
  <!--
    統計画面（Spec 39 D4）は 3 ペインを丸ごと差し替える。**グリッドは v-show で
    DOM に残す** — v-if で外すと入力欄の途中の文・スクロール位置・選択中の個体が
    捨てられる。StatsView は v-if（開いている間だけ sessionStats を叩く）。
  -->
  <StatsView v-if="view === 'stats'" @close="view = 'village'" />
  <div
    v-show="view === 'village'"
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
      <ErrorBoundary :label="$t('app.regions.agentList')">
        <AgentList />
      </ErrorBoundary>
    </aside>

    <PaneSplitter
      direction="col"
      :label="$t('app.splitters.left')"
      @delta="(px) => resize('leftWidth', px)"
      @reset="reset"
    />

    <!--
      中央ペイン: 上段 = サーヴァントの絆（空間の今）/ 下段 = タブ切り替え。
      下段は 黒板（共有作業メモ）と 作業状況（Spec 08 の波ペイン — 時間の痕）。
    -->
    <main
      class="grid min-w-0 overflow-hidden"
      :style="{ gridTemplateRows: centerRows }"
    >
      <section class="min-h-0 overflow-hidden">
        <ErrorBoundary :label="$t('app.regions.map')">
          <TopologyMap />
        </ErrorBoundary>
      </section>

      <!-- 仕切りは下段ペインの上端にあるので、下へ動かすと高さが縮む。 -->
      <PaneSplitter
        direction="row"
        :label="$t('app.splitters.bottom')"
        @delta="(px) => resize('bottomHeight', px, -1)"
        @reset="reset"
      />

      <section class="min-h-0 overflow-hidden">
        <ErrorBoundary
          :label="bottomTab === 'waves' ? $t('app.regions.waves') : $t('app.regions.blackboard')"
        >
          <PlanWavePane
            v-if="bottomTab === 'waves'"
            :active-tab="bottomTab"
            @select-tab="bottomTab = $event"
          />
          <BlackboardPane
            v-else
            :active-tab="bottomTab"
            @select-tab="bottomTab = $event"
          />
        </ErrorBoundary>
      </section>
    </main>

    <!-- 右ペインは左端につまみがあるので、右へ動かすと幅が縮む。 -->
    <PaneSplitter
      direction="col"
      :label="$t('app.splitters.right')"
      @delta="(px) => resize('rightWidth', px, -1)"
      @reset="reset"
    />

    <!-- 右ペイン: 会話 -->
    <aside class="min-w-0 overflow-hidden">
      <ErrorBoundary :label="$t('app.regions.chat')">
        <ChatPanel />
      </ErrorBoundary>
    </aside>
  </div>

  <!--
    **全画面の層はグリッドの外に置く**（Spec 39 P3 の退行の処方）。
    トースト・確認ダイアログ・各ダイアログ・初期化の覆いはどれも `fixed` だが、
    **`display: none` の親の中では `fixed` でも描画されない** — 統計画面へ
    切り替えるとグリッドが `v-show` で消えるので、内側に置くと「開いたのに
    見えず、村へ戻ると現れる」になる。**確認ダイアログが出ないと窓が閉じられない**
    （`askConfirm` の Promise が解決しない）ので、実害はダイアログの見え方に留まらない。
  -->
  <ToastHost />
  <!-- 確認ダイアログ（はい／いいえ）。ブラウザの confirm() は使わない —
       WebView のネイティブダイアログは localhost を名乗る。 -->
  <ConfirmHost />

  <OrdinanceDialog v-if="ordinanceOpen" @close="ordinanceOpen = false" />

  <RoleDialog v-if="rolesOpen" @close="rolesOpen = false" />
  <CommandApprovalDialog
    v-if="commandApprovalOpen"
    @close="commandApprovalOpen = false"
  />

  <McpDialog v-if="mcpOpen" @close="mcpOpen = false" />

  <ScheduleDialog v-if="schedulesOpen" @close="schedulesOpen = false" />

  <SettingsDialog v-if="settingsOpen" @close="settingsOpen = false" />

  <!--
    初期化中の覆い。空の 3 ペインを見せて「壊れている」と誤解させない。
    バックエンドの初期化（MCP 接続を含む）はバックグラウンドで走っており、
    10 秒を超えることがある — その間ここがブロック画面として立つ。
    Vue がマウントするまでの最初の一瞬は index.html 側の同じ見た目の
    スプラッシュが出ており、継ぎ目なくこの覆いへ引き継がれる。

    ただし失敗したときは覆いのまま据え置かない。読み込み中と初期化失敗が
    同じ見た目になると、待てば直るのか壊れているのかを区別する手段が消える。
  -->
  <div
    v-if="!state.ready"
    class="fixed inset-0 z-50 flex flex-col items-center justify-center gap-3 bg-surface-0 px-8 text-center"
  >
    <template v-if="state.initError">
      <p class="font-medium text-fail">{{ $t("app.bootFailed") }}</p>
      <p class="selectable max-w-lg text-[12px] text-ink-dim">
        {{ formatError(state.initError) }}
      </p>
      <p
        v-if="state.initError.detail"
        class="selectable max-w-lg text-[11px] text-ink-dim opacity-70"
      >
        {{ state.initError.detail }}
      </p>
      <button
        v-if="state.initError.code !== 'BOOT_FAILED'"
        class="mt-2 rounded bg-accent px-4 py-1.5 text-[12px] font-medium text-surface-0"
        @click="orchestrator.init()"
      >
        {{ $t("app.retry") }}
      </button>
    </template>
    <template v-else>
      <span class="boot-spinner" aria-hidden="true" />
      <p class="text-ink-dim">{{ $t("app.booting") }}</p>
      <p class="text-[11px] text-ink-dim opacity-60">
        {{ $t("app.bootingHint") }}
      </p>
    </template>
  </div>

  <!--
    フッターのステータスバー。今は時計だけ。**グリッドの外**（flex 列の末尾）に
    置くのは、3 ペインの分割規則（`minmax(0, 1fr)` と仕切り）に巻き込まないため。
    `shrink-0` なので、縮むのは上のグリッド側。
  -->
  <StatusBar
    :stats-active="view === 'stats'"
    @toggle-stats="view = view === 'stats' ? 'village' : 'stats'"
  />
  </div>
</template>
