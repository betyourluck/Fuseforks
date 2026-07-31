<script setup lang="ts">
/**
 * 右ペイン: 会話。吹き出し形式（LINE 型）で読む。
 *
 * # なぜ吹き出しか
 *
 * 前身は表形式のログだったが、**誰が誰に言ったか**を目で追う作業が重かった。
 * 吹き出しは左右で話者を分け、連続する発言をまとめられるので、
 * 会話としての流れがそのまま形になる。
 *
 * # ただし宛先は落とさない
 *
 * 一般的なチャット UI は 1 対 1 を前提にするが、ここは**オーケストレーション**の
 * 画面である。エージェント同士の発話は「誰から誰へ」が本質的な情報なので、
 * 吹き出しの外側に宛先と hop を残す。会話らしさのために情報を捨てない。
 */
import { computed, nextTick, ref, watch } from "vue";
import { openUrl } from "@tauri-apps/plugin-opener";

import ChatInput from "./ChatInput.vue";
import GroundingNote from "./GroundingNote.vue";
import { avatarHue as hueOfName, avatarInitial } from "../lib/avatar";
import {
  buildTimeline,
  collapseRows,
  sameEndpoint,
  type ChatRow,
  type TimelineEntry,
} from "../lib/chatRows";
import { groundingView, type GroundingView } from "../lib/grounding";
import { renderMarkdownCached } from "../lib/markdown";
import { useOrchestrator } from "../composables/useOrchestrator";
import type { AgentId, AgentMessage, Endpoint } from "../types";

const orchestrator = useOrchestrator();
const { state } = orchestrator;

const scroller = ref<HTMLElement | null>(null);
const filterAgentId = ref<AgentId | "">("");

/** 送信の宛先。左ペインまたは接続マップで選んだ 1 体。 */
const targetAgent = computed(
  () => state.agents.find((a) => a.id === state.selectedAgentId) ?? null,
);

/** 送信可能か。停止中のエージェントへは投げられない。 */
const canSend = computed(() => targetAgent.value?.status === "running");

/** 送信できない理由。押せないボタンに理由を添えないと、故障と区別がつかない。 */
const blockedReason = computed(() => {
  if (!targetAgent.value) return "宛先を選択してください";
  return `${targetAgent.value.name} が稼働していません`;
});

/** 入力欄のプレースホルダ。 */
const placeholder = computed(() =>
  targetAgent.value ? `${targetAgent.value.name} へ送信…` : "宛先を選択してください",
);

/** エンドポイントの表示名。 */
function label(endpoint: Endpoint): string {
  switch (endpoint.kind) {
    case "user":
      return "あなた";
    case "system":
      // 「システム」ではなくアプリ名を名乗る。入退室通知などは
      // 匿名の機械音声ではなく、場（Concordia）そのものの声として出す。
      return "Concordia";
    case "agent":
      return state.agents.find((a) => a.id === endpoint.id)?.name ?? endpoint.id;
  }
}

/** アバターに出す 1 文字。 */
function initial(endpoint: Endpoint): string {
  return avatarInitial(label(endpoint));
}

/** アバターの色。規則は 3 画面共通（lib/avatar.ts）。 */
function avatarHue(endpoint: Endpoint): string {
  return hueOfName(label(endpoint));
}

/** エージェントに設定されたアイコン。無ければ頭文字の円にフォールバックする。 */
function iconFor(endpoint: Endpoint): string | null {
  return endpoint.kind === "agent" ? (state.icons[endpoint.id] ?? null) : null;
}

const visible = computed(() => {
  if (!filterAgentId.value) return state.messages;
  return state.messages.filter(
    (m) =>
      (m.from.kind === "agent" && m.from.id === filterAgentId.value) ||
      (m.to.kind === "agent" && m.to.id === filterAgentId.value),
  );
});

/**
 * 表示行。同報・fan-out で複製された発話は 1 行に畳まれる。
 * 規則は lib/chatRows.ts（純関数、単体テスト付き）。
 */
const rows = computed<ChatRow[]>(() => collapseRows(visible.value));

/**
 * 表示中のツール実行。エージェント絞り込みに追従させる。
 *
 * 発話だけを見ていると、エージェントが何をしたかは分からない —
 * ツールの結果はプロンプトの中で消えるので、会話ログには現れない。
 * 「黙って副作用だけ起きた」状態と区別できるようにするための行。
 */
const visibleToolRuns = computed(() =>
  filterAgentId.value
    ? state.toolRuns.filter((run) => run.agentId === filterAgentId.value)
    : state.toolRuns,
);

/** 発話とツール実行を時系列に混ぜた 1 本の並び。規則は lib/chatRows.ts。 */
const timeline = computed<TimelineEntry[]>(() =>
  buildTimeline(rows.value, visibleToolRuns.value),
);

/** 直前の発話と同じ話者・同じ宛先か（タイムライン上の位置で見る）。 */
function continuesTimeline(index: number): boolean {
  const current = timeline.value[index];
  if (current.kind !== "message") return false;
  // 直前の発話行を探す。間にツール行が挟まっても連続とみなす。
  for (let i = index - 1; i >= 0; i -= 1) {
    const previous = timeline.value[i];
    if (previous.kind !== "message") continue;
    const a = previous.row.message.from;
    const b = current.row.message.from;
    if (a.kind !== b.kind) return false;
    const sameFrom = a.kind !== "agent" || (b.kind === "agent" && a.id === b.id);
    // 宛先が変わったら名前行を省かない。宛先ごとに文面が違う fan-out は
    // 連続する別々の吹き出しになり、送り手だけで判定すると 2 通目以降の
    // 「→ 宛先」が画面から消える（実機の plan で発生。この画面は
    // 「宛先を落とさない」が方針なのに、ここで落ちていた）。
    return sameFrom && sameEndpoint(previous.row.message.to, current.row.message.to);
  }
  return false;
}

/** ツール実行の主体名。 */
function toolActor(agentId: AgentId): string {
  return state.agents.find((a) => a.id === agentId)?.name ?? agentId;
}

/**
 * 応答を作っている最中のエージェント。末尾に「入力中…」バブルを出す。
 * エージェントで絞り込んでいる間は、その相手の分だけを出す。
 */
const typingAgents = computed(() =>
  state.agents.filter(
    (a) =>
      state.typing[a.id] &&
      (!filterAgentId.value || a.id === filterAgentId.value),
  ),
);

/** 宛先の表示。同報は「〇〇 他X名」。 */
function toLabel(row: ChatRow): string {
  const first = label(row.message.to);
  if (!row.extraTargets.length) return first;
  return `${first} 他${row.extraTargets.length}名`;
}

/** 同報の全宛先（hover で確認できるように）。 */
function toTitle(row: ChatRow): string {
  if (!row.extraTargets.length) return "";
  return [row.message.to, ...row.extraTargets].map(label).join("、");
}

/** 自分（ユーザー）の発言か。右寄せにする。 */
function isMine(message: AgentMessage): boolean {
  return message.from.kind === "user";
}

/**
 * エージェントの発言だけ Markdown で描画する。
 * ユーザー発言・システム通知は入力どおりのプレーン表示を保つ —
 * 「自分が打った文字列」が描画で変形すると、送った内容の確認ができなくなる。
 */
function isRenderedAsMarkdown(message: AgentMessage): boolean {
  return message.from.kind === "agent";
}

/** 描画済み Markdown（ID キャッシュ付き。発話は不変なので安全）。 */
function markdownOf(message: AgentMessage): string {
  return renderMarkdownCached(message.id, message.content);
}

/**
 * Markdown 内リンクのクリックを横取りし、外部ブラウザで開く。
 *
 * webview 内でそのまま遷移するとアプリ画面ごとリンク先へ置き換わる。
 * `renderMarkdown` が `target="_blank"` を付けているため既定では何も
 * 起きないが、それでは「押しても無反応」なので、ここで opener へ渡す。
 * 開くのは http / https のみ（LLM 出力由来のリンクを信頼しない）。
 */
function onMarkdownClick(event: MouseEvent): void {
  const anchor = (event.target as HTMLElement).closest("a");
  if (!anchor) return;
  event.preventDefault();
  const href = anchor.getAttribute("href") ?? "";
  if (/^https?:\/\//i.test(href)) {
    void openUrl(href);
  }
}

/**
 * 発話に添える接地の来歴（Spec 05 Phase 4）。規則は lib/grounding.ts。
 * 接地していない発話では null になり、欄ごと出ない。
 */
function grounding(message: AgentMessage): GroundingView | null {
  return groundingView(message);
}

function timestamp(ms: number): string {
  return new Date(ms).toLocaleTimeString("ja-JP", {
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** コピー直後の合図を出している発話 ID。1.4 秒で戻す。 */
const copiedId = ref<string | null>(null);
let copiedTimer: ReturnType<typeof setTimeout> | undefined;

/**
 * 発話の本文をクリップボードへコピーする。
 *
 * コピーするのは描画前の**生テキスト**（Markdown ソース）。見た目の HTML を
 * 渡すと、貼り付け先（エディタ・チャット）で再利用できない。
 * クリップボード API を WebView が拒否した場合は textarea 経由の旧手法へ
 * 退避する（プラグイン依存を増やさないための二段構え）。
 */
async function copyMessage(message: AgentMessage): Promise<void> {
  try {
    await navigator.clipboard.writeText(message.content);
  } catch {
    const area = document.createElement("textarea");
    area.value = message.content;
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.appendChild(area);
    area.select();
    try {
      document.execCommand("copy");
    } finally {
      area.remove();
    }
  }
  copiedId.value = message.id;
  clearTimeout(copiedTimer);
  copiedTimer = setTimeout(() => (copiedId.value = null), 1400);
}

watch(
  // 入力中バブルの出入りでも追従する。バブルが末尾に生えた瞬間に
  // 見切れると、「入力中」という一番知りたい情報が見えない。
  () => rows.value.length + typingAgents.value.length,
  async () => {
    const el = scroller.value;
    if (!el) return;
    // 追従するのは、ユーザーが既に末尾を見ているときだけ。
    // 無条件に追従させると、過去を読んでいる最中に引きずり戻されて読めなくなる。
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
    if (!atBottom) return;
    await nextTick();
    el.scrollTop = el.scrollHeight;
  },
);

async function send(content: string): Promise<void> {
  if (!canSend.value || !state.selectedAgentId) return;
  await orchestrator.send(state.selectedAgentId, content);
}

/**
 * 新規チャット。会話ログと全エージェントの履歴を消す（稼働・統計・
 * Memory.md は残る）。ログは現状メモリ内のみで復元不能なので確認必須。
 */
async function newChat(): Promise<void> {
  if (!rows.value.length) return;
  if (!confirm("新規チャットを開始しますか？\n会話ログと各サーヴァントの記憶（履歴）が消えます（復元できません）。\n長期記憶（Memory.md）と稼働状態は残ります。")) return;
  await orchestrator.newChat();
}
</script>

<template>
  <div class="flex h-full flex-col">
    <header
      class="flex shrink-0 items-center gap-2 border-b border-line px-3 py-2 text-xs text-ink-dim"
    >
      <h2 class="font-semibold tracking-wide text-ink">会話</h2>
      <span class="tabular-nums">{{ rows.length }} 件</span>
      <button
        class="rounded border border-line px-1.5 py-0.5 text-[10px] text-ink-dim transition hover:border-accent hover:text-accent disabled:opacity-40"
        :disabled="!rows.length"
        title="会話ログと各サーヴァントの履歴をリセットします（稼働状態と Memory.md は残ります）"
        @click="newChat"
      >
        新規チャット
      </button>
      <select
        v-model="filterAgentId"
        class="ml-auto min-w-0 rounded border border-line bg-surface-1 px-1.5 py-0.5 outline-none focus:border-accent"
      >
        <option value="">すべて</option>
        <option v-for="agent in state.agents" :key="agent.id" :value="agent.id">
          {{ agent.name }}
        </option>
      </select>
    </header>

    <!--
      overflow-x-hidden は保険。中身は下の折り返し規則（wrap-anywhere /
      md-body の overflow-wrap: anywhere）で必ず折れるので、通常ここで
      何かが隠れることはない。長い URL やパスが 1 トークンで来たとき、
      会話全体に横スクロールバーが生える事故だけを構造で封じる。
    -->
    <div
      ref="scroller"
      class="min-h-0 flex-1 space-y-1.5 overflow-x-hidden overflow-y-auto px-3 py-3"
    >
      <template v-for="(entry, index) in timeline" :key="entry.key">
      <!--
        ツール実行の 1 行。発話ではないので吹き出しにせず、淡色の細い行にする。

        出す理由は、**エージェントが何をしたかが会話ログに現れない**こと。
        ツールの結果はプロンプトの中で消えるので、発話だけを見ていると
        「黙って副作用だけ起きた」状態と区別がつかない。時系列に混ぜるのは、
        「調べて」と頼まれた個体が grep を 3 回叩いてから答えた、という
        因果がそこにしか現れないため。
      -->
      <div
        v-if="entry.kind === 'tool'"
        class="flex items-center gap-1.5 pl-9 text-[10px] text-ink-dim"
      >
        <span
          class="inline-block size-1.5 shrink-0 rounded-full"
          :class="entry.run.ok ? 'bg-run' : 'bg-fail'"
        />
        <span class="truncate">
          {{ toolActor(entry.run.agentId) }} が
          <span class="font-mono text-ink">{{ entry.run.tool }}</span>
          を実行{{ entry.run.ok ? "" : "（失敗）" }}
        </span>
        <span class="ml-auto shrink-0 tabular-nums">{{ timestamp(entry.run.tsMs) }}</span>
      </div>

      <div
        v-else
        class="flex gap-2"
        :class="isMine(entry.row.message) ? 'flex-row-reverse' : 'flex-row'"
      >
        <!-- アバター。連続発言では場所だけ空けて揃える。
             画像が設定されていればそれを、無ければ頭文字の円を出す。
             Concordia 発（入退室通知など）はブランドマークを出す —
             エージェントの発言ではなく、場そのものの声だと一目で分かるように。 -->
        <div class="w-7 shrink-0">
          <template v-if="!continuesTimeline(index)">
            <img
              v-if="iconFor(entry.row.message.from)"
              :src="iconFor(entry.row.message.from)!"
              class="size-7 rounded-full object-cover ring-1 ring-line"
              :title="label(entry.row.message.from)"
              :alt="label(entry.row.message.from)"
            />
            <div
              v-else-if="entry.row.message.from.kind === 'system'"
              class="flex size-7 items-center justify-center rounded-full bg-surface-2 ring-1 ring-line"
              :title="label(entry.row.message.from)"
            >
              <!-- TitleBar と同じブランドマーク（3 ノードの三角形）。 -->
              <svg
                width="16"
                height="16"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="1.8"
                class="text-accent"
                aria-hidden="true"
              >
                <circle cx="12" cy="5" r="2.4" />
                <circle cx="5" cy="18" r="2.4" />
                <circle cx="19" cy="18" r="2.4" />
                <path d="M10.6 7 6.4 16M13.4 7l4.2 9M7.4 18h9.2" />
              </svg>
            </div>
            <div
              v-else
              class="flex size-7 items-center justify-center rounded-full text-[11px] font-semibold text-surface-0"
              :style="{ backgroundColor: avatarHue(entry.row.message.from) }"
              :title="label(entry.row.message.from)"
            >
              {{ initial(entry.row.message.from) }}
            </div>
          </template>
        </div>

        <div
          class="flex min-w-0 max-w-[78%] flex-col"
          :class="isMine(entry.row.message) ? 'items-end' : 'items-start'"
        >
          <!-- 誰から誰へ。オーケストレーション画面なので宛先を落とさない。
               同報は「〇〇 他X名」に畳み、全宛先は hover で出す。 -->
          <p
            v-if="!continuesTimeline(index)"
            class="mb-0.5 flex gap-1 px-0.5 text-[10px] text-ink-dim"
            :title="toTitle(entry.row)"
          >
            <span class="font-medium text-ink">{{ label(entry.row.message.from) }}</span>
            <span>→ {{ toLabel(entry.row) }}</span>
          </p>

          <!--
            エージェント発言は Markdown 描画（md で返すことが多い）。
            renderMarkdown は html:false で生 HTML をエスケープするので、
            LLM 出力を v-html へ挿しても任意タグは注入されない。

            折り返しは wrap-anywhere（overflow-wrap: anywhere）。break-words では
            足りない — バブルは shrink-to-fit（列が items-start/end）なので幅の
            算出が min-content を基準に走り、break-word は min-content を
            「単語全体の幅」のまま縮めないため、URL やパスのような長い 1 トークンが
            バブルごと列の外へ押し広げる。anywhere は任意の位置を折り返し候補に
            数えるので min-content が縮み、幅の算出段階から収まる。
            min-w-0 は flex item の暗黙の min-width:auto（= min-content）を外し、
            max-w-full は親列（max-w-[78%]）を超えない上限。三点で 1 セット。
          -->
          <div
            v-if="isRenderedAsMarkdown(entry.row.message)"
            class="md-body selectable min-w-0 max-w-full rounded-2xl rounded-tl-sm bg-surface-2 px-3 py-2 text-[12px] leading-relaxed wrap-anywhere text-ink"
            @click="onMarkdownClick"
            v-html="markdownOf(entry.row.message)"
          />
          <div
            v-else
            class="selectable min-w-0 max-w-full px-3 py-2 text-[12px] leading-relaxed wrap-anywhere whitespace-pre-wrap"
            :class="
              isMine(entry.row.message)
                ? 'rounded-2xl rounded-tr-sm bg-accent text-surface-0'
                : 'rounded-2xl rounded-tl-sm bg-surface-2 text-ink'
            "
          >
            {{ entry.row.message.content }}
          </div>

          <!-- 接地の来歴。規則は lib/grounding.ts、見た目は GroundingNote.vue。
               接地していない発話（大多数）では null になり、欄ごと出ない。 -->
          <GroundingNote v-if="grounding(entry.row.message)" :view="grounding(entry.row.message)!" />

          <!-- hop（転送回数）は常時表示から時刻の hover へ退避した。
               診断情報であって日常の閲覧には要らない（情報は消さず場所を変える）。
               空いた席にコピーを置く。コピーするのは生テキスト。 -->
          <p class="mt-0.5 flex items-center gap-1.5 px-0.5 text-[10px] text-ink-dim tabular-nums">
            <span :title="`転送 ${entry.row.message.hop} 回目（h${entry.row.message.hop}）`">{{
              timestamp(entry.row.message.tsMs)
            }}</span>
            <span v-if="entry.row.message.tokens">{{ entry.row.message.tokens }} tokens</span>
            <button
              type="button"
              class="rounded px-0.5 leading-none transition-colors"
              :class="
                copiedId === entry.row.message.id
                  ? 'text-run'
                  : 'text-ink-dim hover:text-accent'
              "
              :title="
                copiedId === entry.row.message.id ? 'コピーしました' : '本文をコピー'
              "
              @click.stop="copyMessage(entry.row.message)"
            >
              {{ copiedId === entry.row.message.id ? "✓" : "⧉" }}
            </button>
          </p>
        </div>
      </div>
      </template>

      <!-- 入力中バブル。応答の生成中（LLM 呼び出し + ツール実行）に出す。 -->
      <div v-for="agent in typingAgents" :key="`typing-${agent.id}`" class="flex gap-2">
        <div class="w-7 shrink-0">
          <img
            v-if="state.icons[agent.id]"
            :src="state.icons[agent.id]!"
            class="size-7 rounded-full object-cover ring-1 ring-line"
            :title="agent.name"
            :alt="agent.name"
          />
          <div
            v-else
            class="flex size-7 items-center justify-center rounded-full text-[11px] font-semibold text-surface-0"
            :style="{ backgroundColor: hueOfName(agent.name) }"
            :title="agent.name"
          >
            {{ avatarInitial(agent.name) }}
          </div>
        </div>
        <div
          class="typing-bubble rounded-2xl rounded-tl-sm bg-surface-2 px-3 py-2.5"
          :title="`${agent.name} が応答を作成しています`"
        >
          <span class="typing-dot" /><span class="typing-dot" /><span class="typing-dot" />
        </div>
      </div>

      <p v-if="!rows.length && !typingAgents.length" class="py-10 text-center text-[11px] text-ink-dim">
        まだ発話がありません。
      </p>
    </div>

    <ChatInput
      :disabled="!canSend"
      :placeholder="placeholder"
      :blocked-reason="blockedReason"
      @send="send"
    />
  </div>
</template>

<style scoped>
/* 入力中バブルの 3 点アニメーション。位相を 1/6 周期ずつずらして波にする。 */
.typing-bubble {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}
.typing-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background-color: var(--color-ink-dim);
  animation: typing-wave 1.2s ease-in-out infinite;
}
.typing-dot:nth-child(2) {
  animation-delay: 0.2s;
}
.typing-dot:nth-child(3) {
  animation-delay: 0.4s;
}
@keyframes typing-wave {
  0%,
  60%,
  100% {
    opacity: 0.35;
    transform: translateY(0);
  }
  30% {
    opacity: 1;
    transform: translateY(-3px);
  }
}
</style>
