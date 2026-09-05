<script setup lang="ts">
/**
 * 左ペイン: 登録済みエージェントのカードリスト。
 *
 * 新規作成はここに置く。エージェントが 0 体のとき、作成導線が
 * 別画面にあると「何もできないアプリ」に見えるため。
 */
import { computed, nextTick, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { VueDraggable } from "vue-draggable-plus";

import AgentCard from "./AgentCard.vue";
import AgentSettingsDialog from "./AgentSettingsDialog.vue";
import BatchWorkDirDialog from "./BatchWorkDirDialog.vue";
import GroupDialog from "./GroupDialog.vue";
import ModelTemplateDialog from "./ModelTemplateDialog.vue";
import { useHiddenGroups } from "../composables/useHiddenGroups";
import { useOrchestrator } from "../composables/useOrchestrator";
import {
  UNASSIGNED,
  batchEligible,
  commitDrop,
  sectionize,
  type Section,
} from "../lib/agentGroups";
import { inListOrder } from "../lib/agentNav";
import { batchAction, batchLabel } from "../lib/batchStart";
import { dropPoint, tieAddition } from "../lib/kizunaDrop";
import type { AgentGroup, AgentId, AgentSnapshot, AgentSpec, RoleId } from "../types";

const orchestrator = useOrchestrator();
const { state } = orchestrator;
const { t } = useI18n();

const showTemplates = ref(false);
const showBatchWorkDir = ref(false);
/** 設定ダイアログを開いているエージェント。`null` なら閉じている。 */
const configuring = ref<AgentId | null>(null);
const creating = ref(false);
const newName = ref("");

/** 表示順に並べたエージェント。 */
// 並び順は lib/agentNav.ts と共有する。**別々に整列すると、Alt+↑↓ の移動が
// 画面の並びと違う順で飛ぶ**（同じ規則を 2 箇所に書かない）。
const agents = computed(() => inListOrder(state.agents));

// ---- グループ（Spec 51）--------------------------------------------------------
//
// 一覧は「無所属 → グループの配列順」の区分けで、各区分けが 1 つの並び替えの箱。
// 全部に同じ `group` 名を与えて箱をまたげるようにする（Sortable の共有グループ）。
// **グループが 1 つも無い村では見出しを出さない**（今までの一覧と 1 ミリも変わらない）。

const hiddenGroups = useHiddenGroups();
const showGroups = ref(false);
/** 区分け。純関数 `sectionize` が唯一の規則。 */
const sections = computed(() => sectionize(state.agents, state.groups));
const hasGroups = computed(() => state.groups.length > 0);

function isCollapsed(section: Section<AgentSnapshot>): boolean {
  return section.group !== null && hiddenGroups.isHidden(section.group.id);
}

/** 見出しの ▶/■。**全体と同じ純関数に部分集合を渡すだけ**（凍結 7）。 */
function sectionBatch(section: Section<AgentSnapshot>) {
  const action = batchAction(section.agents);
  const eligible = section.agents.filter((a) => a.batchStart).length;
  return { action, view: batchLabel(action, eligible) };
}

/**
 * 見出しの ▶/■ の説明。全体のものと文が違う（グループ名を言う）ので、辞書キーを
 * ここで差し替える。`none` のときは全体と同じ理由文。
 */
function sectionTitle(section: Section<AgentSnapshot>): string {
  const { action, view } = sectionBatch(section);
  const name = section.group?.name ?? t("agentList.unassigned");
  if (action.mode === "start") {
    return t("agentList.groupBatchStart", { name, count: action.targets.length });
  }
  if (action.mode === "stop") {
    return t("agentList.groupBatchStop", { name, count: action.targets.length });
  }
  return view.titleKey === "agentList.batchTransitioning"
    ? t(view.titleKey)
    : t("agentList.groupBatchNoTargets");
}

async function runSectionBatch(section: Section<AgentSnapshot>): Promise<void> {
  const { action } = sectionBatch(section);
  if (action.mode === "none") return;
  await orchestrator.runBatch(action.targets, action.mode === "start");
}

/** 全体 ▶ の門（スイッチ）。見出しだけに置く（`GroupDialog` には置かない — 二重管理にしない）。 */
async function toggleGate(group: AgentGroup): Promise<void> {
  await orchestrator.upsertGroup({ ...group, batchStart: !group.batchStart });
}

/**
 * 選択が変わったら、そのカードを見える位置へ寄せる（Alt+↑↓ 用）。
 *
 * **`block: "nearest"` にするのは、既に見えているときに動かさないため** —
 * クリックで選んだときも同じ watch を通るので、毎回中央へ寄せると画面が跳ねる。
 * 9 体並ぶ村では一覧がスクロールするので、これが無いと選択だけ動いて見えない。
 */
watch(
  () => state.selectedAgentId,
  async (id) => {
    if (!id) return;
    await nextTick();
    document
      .querySelector(`[data-agent-id="${id}"]`)
      ?.scrollIntoView({ block: "nearest" });
  },
);

/** 稼働中の数。ヘッダの要約に出す。 */
const runningCount = computed(
  () => state.agents.filter((a) => a.status === "running").length,
);

/**
 * 一括ボタンが次に行う操作。規則は lib/batchStart.ts。対象は**個体のトグルと
 * グループのスイッチの 2 段**（`batchEligible`。Spec 51 凍結 7）— 非表示は見ない。
 */
const eligibleForBatch = computed(() => batchEligible(state.agents, state.groups));
const batch = computed(() => batchAction(eligibleForBatch.value));

/** 一括ボタンの記号と説明（押せないときは理由）。 */
const batchView = computed(() => batchLabel(batch.value, eligibleForBatch.value.length));

/** 一括起動 / 一括停止を実行する。 */
async function runBatch(): Promise<void> {
  if (batch.value.mode === "none") return;
  await orchestrator.runBatch(batch.value.targets, batch.value.mode === "start");
}

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

/** 新規作成で選ぶ役職。`null` = 役職なし（今までどおりの作成）。 */
const newRoleId = ref<RoleId | null>(null);

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
    workDir: null,
    maxToolIterations: null,
    // 新規作成の保存値は null（既定に従う）。UI の全 ON 表示は null の効果。
    enabledTools: null,
    hearsRoomLog: true,
    allowHandoff: true,
    // 計画の確認（Spec 43）は既定 OFF — 既存の村の plan の挙動を変えない。
    planReview: false,
    // 作ったら一括起動の対象に入れる。外すのが例外側（重いモデル・実験中）。
    batchStart: true,
    // 役職（Spec 14）。**設定の流し込みはコアがやる** — ここで前埋めしない。
    // このフォームは名前しか集めないので、上書きされて困る編集値が存在しない
    // （P2 で立てた D5 は、フォームがこの形である限り起きない）。
    roleId: newRoleId.value,
    // 所属（Spec 51）は作成後に設定ダイアログかドラッグで付ける。作成フォームには置かない。
    groupId: null,
  };

  const created = await orchestrator.createAgent(spec);
  newRoleId.value = null;
  if (created) {
    orchestrator.select(created.id);
    newName.value = "";
    creating.value = false;
  }
}

// ---- カードのドラッグ（並び替え + 地図への絆 drop。Spec 21）----
//
// Sortable の発火順は update:model-value → end（Spec 21 P0 実測 1）。
// 並び替えをその場で確定すると、地図で終わった drop を取り消せないので、
// update では保留するだけにして end のヒットテストで確定か破棄を決める。

/** Sortable の end イベントのうち、この画面が使う形。 */
interface DragEndEvent {
  item?: HTMLElement;
  from?: HTMLElement;
  to?: HTMLElement;
  oldIndex?: number;
  originalEvent?: MouseEvent | TouchEvent;
}

/**
 * ドラッグ中の並び替え結果を**箱ごとに**保留する（Spec 51 D6）。箱をまたぐと出た箱と
 * 入った箱の 2 回、同じ箱の中なら 1 回 `update:model-value` が飛ぶ。end で確定するまで
 * コアへは渡さない（Spec 21 の保留コミットの形は据え置き）。
 */
let pendingBySection: Record<string, AgentId[]> = {};

function holdReorder(section: Section<AgentSnapshot>, reordered: AgentSnapshot[]): void {
  const incoming = reordered.map((a) => a.id);
  // 畳んだ区分けにはカードが描かれていないので、Sortable が返すのは落ちたカード
  // だけ。**畳まれた中の個体を先に置き、落ちたカードを末尾へ**（凍結 8「末尾へ」）。
  pendingBySection[section.key] = isCollapsed(section)
    ? [...section.agents.map((a) => a.id).filter((id) => !incoming.includes(id)), ...incoming]
    : incoming;
}

/** `data-section-key` から区分けを引く。付いていなければ無所属の箱と読む。 */
function sectionOf(el: HTMLElement | undefined): Section<AgentSnapshot> | undefined {
  const key = el?.dataset.sectionKey ?? UNASSIGNED;
  return sections.value.find((s) => s.key === key);
}

/** ドラッグ開始時に測った地図の矩形。copy カーソルの判定に使う。 */
let mapRect: DOMRect | null = null;

function trackPointer(event: PointerEvent): void {
  if (!mapRect) return;
  const inside =
    event.clientX >= mapRect.left &&
    event.clientX <= mapRect.right &&
    event.clientY >= mapRect.top &&
    event.clientY <= mapRect.bottom;
  document.body.classList.toggle("kizuna-drop-target", inside);
}

function onDragStart(): void {
  // 矩形判定だけの安い検査（D2 の連続ヒットテストとは別物。Phase 1 の範囲）。
  mapRect = document.querySelector(".v-ng-container")?.getBoundingClientRect() ?? null;
  document.addEventListener("pointermove", trackPointer);
}

async function onDragEnd(evt: DragEndEvent): Promise<void> {
  document.removeEventListener("pointermove", trackPointer);
  document.body.classList.remove("kizuna-drop-target");
  mapRect = null;

  const pending = pendingBySection;
  pendingBySection = {};
  const fromSection = sectionOf(evt.from);
  const toSection = sectionOf(evt.to);
  const source =
    typeof evt.oldIndex === "number" ? fromSection?.agents[evt.oldIndex] : undefined;

  // 分身はイベント配送前に除去済みなので素の elementFromPoint でよい
  // （Spec 21 P0 実測 2）。closest は輪・名前等の子要素から辿るため。
  //
  // **選択子は自前の属性にする**（2026-08-13）。以前はここに描画ライブラリの
  // クラス名（`.vue-flow__node`）を書いており、**地図の描画を差し替えた瞬間に
  // 黙って壊れた** — 型検査にも lint にも掛からない種類の結合。
  // `data-kizuna-node` は `TopologyMap` が自分で付けているので、ライブラリを
  // 変えても意味が変わらない。**id もここから読む**（属性 1 つで両方足りる）。
  const point = dropPoint(evt.originalEvent);
  const node = point
    ? (document.elementFromPoint(point.x, point.y)?.closest("[data-kizuna-node]") ?? null)
    : null;

  if (!node) {
    // リスト内（または地図でもノードでもない場所）で終わった drop。
    // 並びと所属を**1 回で**確定する（Spec 51 凍結 8）。座標が取れなかったときも
    // こちら = 既存挙動。保留が 1 つも無ければ何も動いていない。
    if (!Object.keys(pending).length || !source || !toSection) return;
    const { order, regroup } = commitDrop(sections.value, pending, toSection.key, source.id);
    await orchestrator.commitDrop(order, regroup);
    return;
  }

  // 地図のノードで終わった drop — 並びも所属も確定しない。state を変えない
  // だけでは DOM が移動後の並びのまま残る（Spec 21 P0 実測 3）ので、
  // カードを元の箱の元の位置へ差し戻す（箱をまたいでいたら `evt.to` から外す）。
  if (Object.keys(pending).length && evt.item && evt.from && typeof evt.oldIndex === "number") {
    evt.item.parentElement?.removeChild(evt.item);
    evt.from.insertBefore(evt.item, evt.from.children[evt.oldIndex] ?? null);
  }

  const targetId = node.getAttribute("data-kizuna-node") as AgentId | null;
  if (!source || !targetId) return;

  const next = tieAddition(state.agents, source.id, targetId);
  if (!next) {
    // 接続済み（方向付き）か自分自身。無音にしない（D3）—
    // 「届いたが張られなかった」ことをノードのパルスで返す。
    node.classList.add("kizuna-pulse");
    window.setTimeout(() => node.classList.remove("kizuna-pulse"), 400);
    return;
  }
  await orchestrator.setConnections(source.id, next);
}
</script>

<template>
  <div class="agent-list flex h-full flex-col">
    <!--
      ヘッダの高さは 4 ペイン共通で 38px に固定する（タイトルバーと同じ）。
      **パディングで揃えない** — 中身の高さがペインごとに違う（ボタンを持つ側は
      テキストだけの側より 6px 高い）ので、padding を合わせても実効高は割れる。
    -->
    <header
      class="flex h-[38px] shrink-0 items-center gap-2 border-b border-line px-3 text-xs"
    >
      <h2 class="font-semibold tracking-wide">{{ $t("agentList.heading") }}</h2>

      <!--
        一括起動 / 一括停止。対象は各カードのトグル（batchStart）で選ぶ。
        起こせる相手が居れば ▶、対象が全員動いていれば ■ へ役が変わる。
        規則は lib/batchStart.ts（純関数・単体テスト付き）。
      -->
      <button
        class="rounded border px-1.5 py-0.5 leading-none transition-colors disabled:opacity-40"
        :class="
          batch.mode === 'stop'
            ? 'border-line text-warn hover:border-warn'
            : 'border-line text-run hover:border-run'
        "
        :disabled="batch.mode === 'none'"
        :title="$t(batchView.titleKey, batchView.titleParams ?? {})"
        @click="runBatch"
      >
        {{ batchView.icon }}
      </button>

      <span class="flex-1 text-ink-dim tabular-nums">
        {{ $t("agentList.runningSummary", { running: runningCount, total: state.agents.length }) }}
      </span>
      <!-- グループの管理（Spec 51 D6）。区分けは一覧を作る側の仕事なので、ここ。 -->
      <button
        class="flex items-center gap-1 rounded px-1 py-0.5 text-ink-dim transition-colors hover:text-accent focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-accent"
        :title="$t('agentList.manageGroups')"
        :aria-label="$t('agentList.manageGroups')"
        @click="showGroups = true"
      >
        <svg
          class="size-4"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.8"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <rect x="3" y="4" width="18" height="6" rx="1.5" />
          <rect x="3" y="14" width="18" height="6" rx="1.5" />
        </svg>
        <span class="agent-list-action-label">{{ $t("agentList.manageGroupsLabel") }}</span>
      </button>
      <button
        class="flex items-center gap-1 rounded px-1 py-0.5 text-ink-dim transition-colors hover:text-accent focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-accent"
        :title="$t('agentList.manageTemplates')"
        :aria-label="$t('agentList.manageTemplates')"
        @click="showTemplates = true"
      >
        <svg
          class="size-4"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.8"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M12 3a3.5 3.5 0 0 0-3.5 3.5c0 .7.2 1.4.6 2L12 12l2.9-3.5c.4-.6.6-1.3.6-2A3.5 3.5 0 0 0 12 3Z" />
          <path d="M8.5 8.5A3.5 3.5 0 0 0 5 12c0 .7.2 1.4.6 2L9 16.9l3-4.9" />
          <path d="M15.5 8.5A3.5 3.5 0 0 1 19 12c0 .7-.2 1.4-.6 2L15 16.9l-3-4.9" />
          <circle cx="12" cy="12" r="1.5" />
          <path d="M9 16.9 8.1 20 12 18.5l3.9 1.5-.9-3.1" />
        </svg>
        <span class="agent-list-action-label">{{ $t("agentList.modelRegistration") }}</span>
      </button>
      <button
        class="flex items-center gap-1 rounded px-1 py-0.5 text-ink-dim transition-colors hover:text-accent focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-accent"
        :title="$t('agentList.addServant')"
        :aria-label="$t('agentList.addServant')"
        @click="creating = !creating"
      >
        <svg
          class="size-4"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          aria-hidden="true"
        >
          <path d="M12 5v14M5 12h14" />
        </svg>
        <span class="agent-list-action-label">{{ $t("agentList.addServantLabel") }}</span>
      </button>
    </header>

    <form v-if="creating" class="border-b border-line p-3" @submit.prevent="submitNew">
      <input
        v-model="newName"
        :placeholder="$t('agentList.namePlaceholder')"
        autofocus
        class="w-full rounded border border-line bg-surface-1 px-2 py-1.5 outline-none focus:border-accent"
      />
      <!--
        役職（Spec 14）。選ぶと設定が入った状態で作られる。**選ばなくてもよい** —
        既定は「役職なし」で、それが今までどおりの作成経路。
        役職が 1 つも無い村では出さない（選べない選択肢を並べない）。
      -->
      <select
        v-if="state.roles.length"
        v-model="newRoleId"
        class="mt-1.5 w-full rounded border border-line bg-surface-1 px-2 py-1.5 outline-none focus:border-accent"
      >
        <option :value="null">{{ $t("agentList.noRole") }}</option>
        <option v-for="role in state.roles" :key="role.id" :value="role.id">
          {{ role.name }}
        </option>
      </select>
      <p v-if="!state.templates.length" class="mt-1.5 text-[11px] text-warn">
        {{ $t("agentList.noTemplates") }}
      </p>
      <div class="mt-2 flex justify-end gap-2">
        <button
          type="button"
          class="rounded px-2 py-1 text-ink-dim hover:text-ink"
          @click="creating = false"
        >
          {{ $t("agentList.cancel") }}
        </button>
        <button
          type="submit"
          class="rounded bg-accent px-3 py-1 font-medium text-surface-0 disabled:opacity-40"
          :disabled="!newName.trim()"
        >
          {{ $t("agentList.create") }}
        </button>
      </div>
    </form>

    <!--
      force-fallback を外さないこと。Tauri の既定 dragDropEnabled=true は
      Windows WebView2 でページ内のネイティブ DnD と衝突する報告があり、
      外すと並び替えごと動かなくなる（Spec 21。断定形への裏取りは P4 の実機で）。
      Spec 21 の絆 drop も fallback の座標を前提にしている。
    -->
    <!--
      区分けごとに 1 つの並び替えの箱（Spec 51 D6）。全部に同じ `group` 名を与えて
      箱をまたげるようにする。**1 本の箱に見出しを混ぜない** — Sortable の index は
      `draggable` な子だけを数えるので、見出しを配列に混ぜると splice の位置がずれる。
      グループが無い村では見出しを出さず、無所属の箱 1 つ = 今までの一覧。
    -->
    <div class="min-h-0 flex-1 overflow-y-auto p-3">
      <template v-for="section in sections" :key="section.key">
        <div
          v-if="hasGroups"
          class="mb-1 mt-2 flex items-center gap-1.5 text-[11px] first:mt-0"
          :class="section.group ? 'text-ink' : 'text-ink-dim'"
        >
          <!-- 目（表示/非表示）。無所属には置かない — 無所属は隠れない（凍結 6）。 -->
          <button
            v-if="section.group"
            class="rounded px-1 text-ink-dim hover:text-accent"
            :title="isCollapsed(section) ? $t('agentList.groupShow') : $t('agentList.groupHide')"
            :aria-label="isCollapsed(section) ? $t('agentList.groupShow') : $t('agentList.groupHide')"
            :aria-pressed="isCollapsed(section)"
            @click="hiddenGroups.toggle(section.group.id)"
          >
            <svg class="size-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M2 12s3.5-6 10-6 10 6 10 6-3.5 6-10 6-10-6-10-6Z" />
              <circle cx="12" cy="12" r="3" />
              <path v-if="isCollapsed(section)" d="M4 4l16 16" />
            </svg>
          </button>
          <span class="truncate font-semibold">
            {{ section.group?.name ?? $t("agentList.unassigned") }}
          </span>
          <span class="tabular-nums text-ink-dim">
            {{ $t("agentList.groupMembers", { count: section.agents.length }) }}
          </span>
          <span v-if="isCollapsed(section)" class="text-ink-dim">
            {{ $t("agentList.groupHidden") }}
          </span>
          <span class="flex-1" />
          <!-- 見出しの ▶/■。全体と同じ純関数に部分集合を渡す（凍結 7）。 -->
          <button
            class="rounded border px-1.5 py-0.5 leading-none transition-colors disabled:opacity-40"
            :class="
              sectionBatch(section).action.mode === 'stop'
                ? 'border-line text-warn hover:border-warn'
                : 'border-line text-run hover:border-run'
            "
            :disabled="sectionBatch(section).action.mode === 'none'"
            :title="sectionTitle(section)"
            @click="runSectionBatch(section)"
          >
            {{ sectionBatch(section).view.icon }}
          </button>
          <!-- 全体 ▶ の門。非表示とは独立（隠しても起きる）。無所属には無い。 -->
          <button
            v-if="section.group"
            type="button"
            role="switch"
            :aria-checked="section.group.batchStart"
            :title="$t('agentList.groupBatchGate') + ' — ' + $t('agentList.groupBatchGateHelp')"
            class="relative h-3.5 w-6 shrink-0 rounded-full transition-colors"
            :class="section.group.batchStart ? 'bg-accent' : 'bg-line'"
            @click="toggleGate(section.group)"
          >
            <span
              class="absolute top-0.5 size-2.5 rounded-full bg-surface-0 transition-transform"
              :class="section.group.batchStart ? 'translate-x-3' : 'translate-x-0.5'"
            />
          </button>
        </div>
        <VueDraggable
          :model-value="isCollapsed(section) ? [] : section.agents"
          tag="div"
          group="agents"
          :data-section-key="section.key"
          class="space-y-2"
          :class="isCollapsed(section) ? 'min-h-2 rounded border border-dashed border-line/60' : hasGroups ? 'min-h-6' : ''"
          :animation="150"
          :force-fallback="true"
          ghost-class="opacity-40"
          chosen-class="agent-card--dragging"
          filter="button, input, textarea"
          :prevent-on-filter="false"
          @update:model-value="(v: AgentSnapshot[]) => holdReorder(section, v)"
          @start="onDragStart"
          @end="onDragEnd"
        >
          <AgentCard
            v-for="agent in isCollapsed(section) ? [] : section.agents"
            :key="agent.id"
            :data-agent-id="agent.id"
            :agent="agent"
            :icon="state.icons[agent.id]"
            :last-tool="state.lastTool[agent.id]"
            :selected="agent.id === state.selectedAgentId"
            @select="orchestrator.select(agent.id)"
            @configure="configuring = agent.id"
            @toggle="(running) => orchestrator.toggleRunning(agent.id, running)"
            @batch-start="(included) => orchestrator.setBatchStart(agent.id, included)"
            @dismiss-error="orchestrator.dismissError(agent.id)"
          />
        </VueDraggable>
      </template>

      <p
        v-if="!agents.length"
        class="px-2 py-8 text-center text-[11px] leading-relaxed text-ink-dim"
      >
        {{ $t("agentList.emptyLine1") }}<br />
        {{ $t("agentList.emptyLine2") }}
      </p>
    </div>

    <!--
      一覧のフッター（Spec 29。**アプリのステータスバーとは別物** — この帯は
      左ペインの中に住む）。ヘッダと分けたのは置き場の都合ではなく**役の違い**:
      ヘッダの「モデル登録」「追加」は*作る・登録する*側、こちらは
      **既にいる個体をまとめて扱う**側（利用者裁定 2026-08-08）。
      一覧の直下にあるので、対象が目の前にある状態で押せる。
    -->
    <footer
      class="flex shrink-0 items-center gap-2 border-t border-line px-3 py-1.5 text-xs"
    >
      <button
        class="flex items-center gap-1 rounded px-1 py-0.5 text-ink-dim transition-colors hover:text-accent focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-accent"
        :title="$t('agentList.batchWorkDir')"
        :aria-label="$t('agentList.batchWorkDir')"
        @click="showBatchWorkDir = true"
      >
        <svg
          class="size-4"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.8"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M3 7a2 2 0 0 1 2-2h3.5l2 2H19a2 2 0 0 1 2 2v3" />
          <path d="M3 7v11a2 2 0 0 0 2 2h8" />
          <path d="m16 19 2 2 4-4" />
        </svg>
        <span class="agent-list-action-label">{{ $t("agentList.batchWorkDir") }}</span>
      </button>
    </footer>

    <ModelTemplateDialog v-if="showTemplates" @close="showTemplates = false" />
    <GroupDialog v-if="showGroups" @close="showGroups = false" />
    <BatchWorkDirDialog v-if="showBatchWorkDir" @close="showBatchWorkDir = false" />
    <AgentSettingsDialog
      v-if="configuring"
      :agent-id="configuring"
      @close="configuring = null"
    />
  </div>
</template>

<style>
.agent-list {
  container-type: inline-size;
  container-name: agent-list;
}

@container agent-list (max-width: 329px) {
  .agent-list-action-label {
    display: none;
  }
}

.agent-card--dragging {
  border-color: var(--color-accent);
}

/*
 * カード drop の視覚応答（Spec 21）。対象は地図側の要素だが、効果の持ち主は
 * この画面の drop 経路なので、ここに置く（TopologyMap の style は scoped）。
 */

/* ドラッグ中、地図の矩形に入ったら「落とせる」を示す（D2 の代替。矩形判定のみ）。 */
body.kizuna-drop-target,
body.kizuna-drop-target * {
  cursor: copy !important;
}

/* 張られなかった drop（接続済み・自分自身）への応答。無音にしない（D3）。 */
.kizuna-node.kizuna-pulse {
  animation: kizuna-pulse 0.4s ease-out;
}

@keyframes kizuna-pulse {
  0% {
    box-shadow: 0 0 0 0 var(--color-accent);
  }
  100% {
    box-shadow: 0 0 0 12px transparent;
  }
}
</style>
