<script setup lang="ts">
/**
 * 中央ペイン下段: 村の黒板（共有作業メモの読み取り専用ビュー）。
 *
 * 実体はエージェントの共通 work_dir にある `blackboard/` フォルダ。書くのは
 * エージェント（file ツール）と人で、**GUI からの書き込み経路は作らない**
 * （条例の「書いてよいのは自分の付箋だけ」を GUI が迂回しない）。
 *
 * 更新は pull のみ — 表示中の定期再読 + 手動更新。コアはファイル変更を
 * 監視せず、モデルへの push 注入もしない（黒板の運用と同じ形）。
 */
import { computed, onMounted, onUnmounted, ref } from "vue";

import {
  clearBlackboard,
  deleteBlackboardNote,
  listBlackboard,
  readOrdinance,
  toErrorPayload,
} from "../lib/ipc";
import { useI18n } from "vue-i18n";

import { noteKey, useBlackboardCollapse } from "../composables/useBlackboardCollapse";
import { askConfirm } from "../composables/useConfirm";
import { useOrchestrator } from "../composables/useOrchestrator";
import {
  isOrphan,
  kanbanNotes,
  stateDictKey,
  isKnownState,
  type KanbanColumn,
  type MarkedNote,
  type NoteBadge,
} from "../lib/blackboardLanes";
import { hasBlackboardSection } from "../lib/blackboardOrdinance";
import { formatError } from "../lib/errorText";
import { renderMarkdown } from "../lib/markdown";
import type { BlackboardNote, BottomTab, ErrorPayload } from "../types";
import BottomPaneTabs from "./BottomPaneTabs.vue";

defineProps<{ activeTab: BottomTab }>();

const emit = defineEmits<{
  (e: "selectTab", tab: BottomTab): void;
  (e: "openOrdinance"): void;
}>();

const orchestrator = useOrchestrator();
/** 付箋の畳み。部品の外（モジュール + localStorage）に持つ — タブを離れても残す。 */
const collapse = useBlackboardCollapse();

const notes = ref<BlackboardNote[]>([]);

/**
 * 付箋を**仕事の状態（フォルダ）で列に**分け、**持ち主の手番をバッジに**する
 * （`lib/blackboardLanes`。Spec 54）。
 *
 * 列は付箋の置き場（`BlackboardNote.state`）が決める。持ち主の手番は**投影**
 * （typing / status / 波 / workDir）から毎回派生させてバッジにするだけなので、
 * 付箋にもワイヤにも持ち主の状態の欄は無い — 「あなたの手番」は作業状況タブと同じ
 * `PlanWaveState` を読んでいる。消してよいのは「完了」列だけで、そこにだけ一括の
 * 消し口を置く。
 */
const board = computed(() =>
  kanbanNotes(notes.value, {
    agents: orchestrator.state.agents,
    typing: orchestrator.state.typing,
    waves: orchestrator.state.planWaves,
  }),
);

type Section = {
  key: string;
  column: KanbanColumn<BlackboardNote> | null;
  notes: MarkedNote<BlackboardNote>[] | BlackboardNote[];
};

/**
 * 画面の並び: まとめ（あれば）→ 5 つの状態の列（0 枚でも出す）→ 状態なし（あれば）→
 * その他（フォルダごと・あれば）。列順は純関数が持つ（コアの返却順は別のもの）。
 */
const sections = computed<Section[]>(() => [
  ...(board.value.summary.length
    ? [{ key: "summary", column: null, notes: board.value.summary }]
    : []),
  ...board.value.columns.map((column) => ({
    key: column.kind === "unfiled" ? "unfiled" : `${column.kind}:${column.state}`,
    column,
    notes: column.notes,
  })),
]);

/** 列の見出し。辞書の鍵は状態ごとに実行時に組む（走査テストが列挙と突き合わせる）。 */
function columnLabel(section: Section): string {
  const column = section.column;
  if (column === null) return t(`blackboard.state.${stateDictKey("summary")}`);
  if (column.kind === "unfiled") return t(`blackboard.state.${stateDictKey("unfiled")}`);
  if (column.kind === "other") return t("blackboard.state.other", { name: column.state });
  return t(`blackboard.state.${stateDictKey(column.state as never)}`);
}

function columnTitle(section: Section): string {
  const column = section.column;
  if (column === null) return t(`blackboard.stateTitle.${stateDictKey("summary")}`);
  if (column.kind === "unfiled") return t(`blackboard.stateTitle.${stateDictKey("unfiled")}`);
  if (column.kind === "other") return t("blackboard.stateTitle.other", { name: column.state });
  return t(`blackboard.stateTitle.${stateDictKey(column.state as never)}`);
}

/** 列の印の色。`doing` は動いている色、残りは線の色。 */
function columnDot(section: Section): string {
  const state = section.column?.state;
  if (state === "doing") return "bg-run";
  return "bg-line";
}

function isDone(section: Section): boolean {
  return section.column?.kind === "state" && section.column.state === "done";
}

/** 持ち主の手番のバッジ。辞書の鍵を返す（訳語は持たない）。内訳は既存の `reason.*`。 */
function badgeKey(badge: NoteBadge): string {
  return badge === "active" || badge === "yourTurn"
    ? `blackboard.badge.${badge}`
    : `blackboard.reason.${badge}`;
}

function marksOf(note: BlackboardNote | MarkedNote<BlackboardNote>) {
  return "marks" in note ? note.marks : null;
}

/** `isKnownState` は列挙の網に載せるためだけに参照する（列は純関数が決める）。 */
void isKnownState;
const error = ref<ErrorPayload | null>(null);
/** 初回の読みが済むまで「空」と断定しない（一瞬の空表示のちらつき防止）。 */
const loaded = ref(false);

/** 表示中の自動再読の間隔。ローカルの一覧 + 読みだけなので軽い。 */
const REFRESH_MS = 10_000;
let timer: number | undefined;

const { t } = useI18n();

/**
 * 削除の実行中。**押している間はどのボタンも押せなくする** —
 * 10 秒ごとの自動再読と重なると、消えた付箋の行をもう一度押せてしまう。
 */
const busy = ref(false);

/**
 * 付箋を 1 枚ごみ箱へ移す。**確認は出さない。**
 *
 * 消し先がごみ箱なので取り消せる（`file` ツールの remove と同じ規律で、
 * 完全削除の経路は持たない）。**取り消せる操作に確認を積むと、
 * 取り消せない操作の確認まで軽く読まれる。**
 */
async function remove(note: BlackboardNote): Promise<void> {
  if (busy.value) return;
  busy.value = true;
  try {
    await deleteBlackboardNote(note.dir, note.name, note.state);
    error.value = null;
  } catch (err) {
    error.value = toErrorPayload(err);
  } finally {
    busy.value = false;
    await refresh();
  }
}

/**
 * 付箋を全部ごみ箱へ移す。**確認を出す。**
 *
 * 1 枚ずつと違い、押し間違いの代償が枚数ぶん。**件数を文面に入れる** —
 * 「全部」だけでは何枚あるか分からないまま押すことになる。
 */
async function clearAll(): Promise<void> {
  if (busy.value || notes.value.length === 0) return;
  const ok = await askConfirm({
    title: t("blackboard.confirmClearTitle"),
    message: t("blackboard.confirmClearMessage", { count: notes.value.length }),
    confirmLabel: t("blackboard.confirmClearLabel"),
    danger: true,
  });
  if (!ok) return;

  busy.value = true;
  try {
    await clearBlackboard();
    error.value = null;
  } catch (err) {
    error.value = toErrorPayload(err);
  } finally {
    busy.value = false;
    await refresh();
  }
}

/**
 * 「完了」列の付箋だけをごみ箱へ移す。**確認を出す**（一括なので）。
 *
 * 全消しと同じ IPC は使わない — `clear_blackboard` は列を知らないので、
 * 一覧が返した `dir` / `state` / `name` で 1 枚ずつ `delete_blackboard_note` を呼ぶ
 * （Spec 54 凍結 8。新しい IPC は無い）。途中で失敗したら残りは消さずに止め、
 * エラーを出して読み直す。
 */
async function clearDone(): Promise<void> {
  const targets = board.value.columns.find((c) => c.kind === "state" && c.state === "done")
    ?.notes ?? [];
  if (busy.value || targets.length === 0) return;
  const ok = await askConfirm({
    title: t("blackboard.confirmClearDoneTitle"),
    message: t("blackboard.confirmClearDoneMessage", { count: targets.length }),
    confirmLabel: t("blackboard.confirmClearDoneLabel"),
    danger: true,
  });
  if (!ok) return;

  busy.value = true;
  try {
    for (const note of targets) {
      await deleteBlackboardNote(note.dir, note.name, note.state);
    }
    error.value = null;
  } catch (err) {
    error.value = toErrorPayload(err);
  } finally {
    busy.value = false;
    await refresh();
  }
}

/**
 * 条例に黒板の節が無いか（2026-09-17）。**読めて、無いと分かったときだけ真。**
 *
 * 黒板の規約をサーヴァントへ伝える経路は条例だけなので、節が無い村では付箋が 1 枚も
 * 書かれず、この画面は理由を言わずに空のまま残る。読むのは**付箋が 0 枚のときだけ**
 * （付箋がある村は規約が伝わっている）。読めなかったときは何も言わない —
 * 「分からない」を「無い」と書かない。
 */
const ordinanceLacksBoard = ref(false);

async function checkOrdinance(): Promise<void> {
  if (notes.value.length > 0) {
    ordinanceLacksBoard.value = false;
    return;
  }
  try {
    ordinanceLacksBoard.value = !hasBlackboardSection(await readOrdinance());
  } catch {
    ordinanceLacksBoard.value = false;
  }
}

async function refresh(): Promise<void> {
  try {
    notes.value = await listBlackboard();
    collapse.prune(notes.value);
    error.value = null;
    await checkOrdinance();
  } catch (err) {
    error.value = toErrorPayload(err);
  } finally {
    loaded.value = true;
  }
}

onMounted(() => {
  void refresh();
  timer = window.setInterval(() => void refresh(), REFRESH_MS);
});
onUnmounted(() => window.clearInterval(timer));

/** 複数の work_dir が混在するときだけ由来を出す。通常は 1 つで無音。 */
const showDir = computed(() => new Set(notes.value.map((n) => n.dir)).size > 1);

function formatTime(ms: number): string {
  if (ms === 0) return "";
  const date = new Date(ms);
  const today = new Date();
  const sameDay =
    date.getFullYear() === today.getFullYear() &&
    date.getMonth() === today.getMonth() &&
    date.getDate() === today.getDate();
  return sameDay ? date.toLocaleTimeString() : date.toLocaleString();
}
</script>

<template>
  <div class="flex h-full flex-col">
    <!--
      高さは 4 ペイン共通の 38px 固定（AgentList のコメント参照）。
      下線は引かない — VS Code のパネル（問題 / 出力）と同じで、タブと中身の
      間に境界線を置かない（2026-08-02 利用者指定。下段タブの 2 ペイン共通）。
    -->
    <header
      class="flex h-[38px] shrink-0 items-center gap-3 px-3 text-xs text-ink-dim"
    >
      <BottomPaneTabs :active="activeTab" @select="emit('selectTab', $event)" />
      <span v-if="notes.length">{{ $t("blackboard.noteCount", { count: notes.length }) }}</span>
      <!--
        一括削除。**確認を出す**（全部まとめて消えるので、押し間違いの代償が
        1 枚とは桁で違う）。アイコンはチャット入力の表示クリアと同じ消しゴム —
        同じ「消す」の絵を 2 つ持たない。
        ただし**あちらは表示だけ・こちらは実体**なので、確認の文面で言い切る。
      -->
      <button
        class="ml-auto grid size-6 place-items-center rounded text-ink-dim transition-colors hover:text-fail focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-accent disabled:opacity-40 disabled:hover:text-ink-dim"
        :disabled="notes.length === 0 || busy"
        :title="$t('blackboard.clearAllTitle')"
        :aria-label="$t('blackboard.clearAll')"
        @click="clearAll"
      >
        <svg
          class="size-4"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="m15 5 5 5-8 8H7l-4-4z" />
          <path d="M21 20h-11" />
        </svg>
      </button>
      <button
        class="grid size-6 place-items-center rounded text-ink-dim transition-colors hover:text-ink focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-accent"
        :title="$t('blackboard.refreshTitle')"
        :aria-label="$t('blackboard.refresh')"
        @click="refresh"
      >
        <svg
          class="size-4"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <path d="M21 12a9 9 0 1 1-3-6.7" />
          <path d="M21 3v6h-6" />
        </svg>
      </button>
    </header>

    <div
      v-if="error"
      class="flex flex-1 items-center justify-center px-6 text-center text-xs text-fail"
    >
      {{ formatError(error) }}
    </div>

    <!--
      まとめ（あれば）→ 5 つの状態の列 → 状態なし → その他、の順に縦へ並べる（Spec 54）。
      5 つの列の見出しは空でも出す — 「どれを消すか」を列で読ませるのが目的なので、
      列の形が毎回同じであることが情報になる。付箋の並びは列の中でもコアの順
      （孤児だけ先頭）。列順は純関数が持ち、コアの返却順（state の文字列順）とは別。

      **付箋が 0 枚でも列を出す**（2026-09-16 利用者裁定）。初回の読みが済むまでは
      0 枚の列が見え、読めたあと 0 枚なら空の文言に**差し替えて**いた — 読み込みの
      前後で画面の形が変わり「チラッとカテゴリが見える」になっていた。差し替えをやめ、
      空の文言は列の下に足す。読み込みの前後で同じ形なので、ちらつきは構造で消える。
    -->
    <div v-else class="min-h-0 flex-1 overflow-y-auto px-3 py-2">
      <section v-for="section in sections" :key="section.key" class="mb-3" :data-column="section.key">
        <header
          class="mb-1.5 flex h-6 items-center gap-2 text-[11px] text-ink-dim"
          :title="columnTitle(section)"
        >
          <span
            :class="['inline-block size-2 shrink-0 rounded-sm', columnDot(section)]"
            aria-hidden="true"
          />
          <span class="font-semibold text-ink">{{ columnLabel(section) }}</span>
          <span>{{ $t("blackboard.noteCount", { count: section.notes.length }) }}</span>
          <!--
            列の消し口は「完了」にだけ置く（Spec 54 凍結 8）。他の列の付箋は仕事が
            終わっていないので、まとめて消す導線を出さない（個別のごみ箱は残る）。
          -->
          <button
            v-if="isDone(section)"
            class="ml-auto grid size-5 place-items-center rounded text-ink-dim transition-colors hover:text-fail focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-accent disabled:opacity-40 disabled:hover:text-ink-dim"
            :disabled="section.notes.length === 0 || busy"
            :title="$t('blackboard.clearDoneTitle')"
            :aria-label="$t('blackboard.clearDone')"
            @click="clearDone"
          >
            <svg
              class="size-3.5"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
              aria-hidden="true"
            >
              <path d="m15 5 5 5-8 8H7l-4-4z" />
              <path d="M21 20h-11" />
            </svg>
          </button>
        </header>

      <article
        v-for="note in section.notes"
        :key="noteKey(note)"
        class="mb-2 rounded-lg border border-line/50 bg-surface-1"
      >
        <!--
          見出し行を押すと本文を畳む ⇄ 開く（ごみ箱は `.stop` で別）。畳んでも
          見出し行（名前・バッジ・時刻・ごみ箱）は残るので、列の中での枚数と
          「消せるか」は畳んだままでも読める。
        -->
        <header
          :class="[
            'flex cursor-pointer items-baseline gap-2 px-3 py-1.5 text-[11px] select-none',
            collapse.isCollapsed(note) ? '' : 'border-b border-line/50',
          ]"
          @click="collapse.toggle(note)"
        >
          <button
            class="grid size-4 shrink-0 self-center place-items-center rounded text-ink-dim transition-colors hover:text-ink focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-accent"
            :aria-expanded="!collapse.isCollapsed(note)"
            :title="$t(collapse.isCollapsed(note) ? 'blackboard.expand' : 'blackboard.collapse')"
            :aria-label="$t(collapse.isCollapsed(note) ? 'blackboard.expand' : 'blackboard.collapse')"
            @click.stop="collapse.toggle(note)"
          >
            <svg
              :class="['size-3 transition-transform', collapse.isCollapsed(note) ? '-rotate-90' : '']"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
              aria-hidden="true"
            >
              <path d="m6 9 6 6 6-6" />
            </svg>
          </button>
          <span class="font-semibold text-ink">{{ note.name }}</span>
          <!--
            バッジは 2 種。持ち主の手番（1 枚に 1 つ。孤児は赤・手番は accent・動いているは
            run 色）と、同名が複数の状態に並ぶ重複（赤）。列は状態、印は観測 — 2 つを
            1 つの列に混ぜない（Spec 54 D3）。
          -->
          <template v-if="marksOf(note)">
            <span
              :class="[
                'shrink-0 rounded px-1.5 py-px text-[10px]',
                isOrphan(marksOf(note)!.badge)
                  ? 'bg-fail/15 text-fail'
                  : marksOf(note)!.badge === 'yourTurn'
                    ? 'bg-accent/15 text-accent'
                    : marksOf(note)!.badge === 'active'
                      ? 'bg-run/15 text-run'
                      : 'bg-line/60 text-ink-dim',
              ]"
              :data-badge="marksOf(note)!.badge"
              >{{ $t(badgeKey(marksOf(note)!.badge)) }}</span
            >
            <span
              v-if="marksOf(note)!.duplicate"
              class="shrink-0 rounded bg-fail/15 px-1.5 py-px text-[10px] text-fail"
              :title="$t('blackboard.badgeTitle.duplicate')"
              data-badge="duplicate"
              >{{ $t("blackboard.badge.duplicate") }}</span
            >
          </template>
          <span v-if="showDir" class="truncate text-ink-dim" :title="note.dir">{{
            note.dir
          }}</span>
          <span v-if="note.modifiedMs" class="ml-auto shrink-0 text-ink-dim">{{
            formatTime(note.modifiedMs)
          }}</span>
          <!--
            個別削除。**確認を出さない**のは、消し先が**ごみ箱**だから
            （`file` ツールの remove と同じ規律で、完全削除の経路は無い）。
            取り消せる操作に確認を積むと、取り消せない操作の確認まで軽く読まれる。
            日時が無い付箋でも押せるよう、位置は `ml-auto` を持つ側と分けてある。
          -->
          <button
            :class="[
              'grid size-5 shrink-0 place-items-center rounded text-ink-dim transition-colors hover:text-fail focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-accent disabled:opacity-40',
              note.modifiedMs ? '' : 'ml-auto',
            ]"
            :disabled="busy"
            :title="$t('blackboard.deleteTitle', { name: note.name })"
            :aria-label="$t('blackboard.deleteTitle', { name: note.name })"
            @click.stop="remove(note)"
          >
            <svg
              class="size-3.5"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
              aria-hidden="true"
            >
              <path d="M3 6h18" />
              <path d="M8 6V4h8v2" />
              <path d="M19 6l-1 14H6L5 6" />
              <path d="M10 11v6M14 11v6" />
            </svg>
          </button>
        </header>
        <!--
          renderMarkdown は html:false で生 HTML をエスケープするので、
          エージェントが書いた内容を v-html へ挿しても任意タグは注入されない
          （ChatPanel の会話バブルと同じ前提）。
        -->
        <div
          v-show="!collapse.isCollapsed(note)"
          class="md-body selectable px-3 py-2 text-[12px] leading-relaxed wrap-anywhere text-ink"
          v-html="renderMarkdown(note.content)"
        />
      </article>
      </section>

      <p v-if="loaded && notes.length === 0" class="px-3 py-4 text-center text-xs text-ink-dim">
        {{ $t("blackboard.empty") }}
      </p>
      <!-- 条例に黒板の節が無い村では、待っても付箋は書かれない。理由と次の手を名指しする。 -->
      <div
        v-if="loaded && notes.length === 0 && ordinanceLacksBoard"
        data-ordinance-hint
        class="mx-3 mb-4 rounded border border-warn/40 bg-surface-1 px-3 py-2 text-center text-xs text-ink"
      >
        <p>{{ $t("blackboard.noOrdinanceSection") }}</p>
        <button
          class="mt-2 rounded bg-accent px-3 py-1 text-[11px] font-medium text-surface-0"
          @click="emit('openOrdinance')"
        >
          {{ $t("blackboard.openOrdinance") }}
        </button>
      </div>
    </div>
  </div>
</template>
