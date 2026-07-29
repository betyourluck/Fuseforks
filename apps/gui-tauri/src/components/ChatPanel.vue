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
import { avatarHue as hueOfName, avatarInitial } from "../lib/avatar";
import { collapseRows, type ChatRow } from "../lib/chatRows";
import { renderMarkdownCached } from "../lib/markdown";
import { useOrchestrator } from "../composables/useOrchestrator";
import type { AgentId, AgentMessage, Endpoint } from "../types";

const orchestrator = useOrchestrator();
const { state } = orchestrator;

const scroller = ref<HTMLElement | null>(null);
const filterAgentId = ref<AgentId | "">("");

/**
 * 送信の宛先。**1 体だけ**を指名する。
 *
 * # なぜ同報をやめたのか
 *
 * 以前は複数を選んで同時に話しかけられた。だが同報は全員のターンが**並列**に
 * 走るため、誰も他の答えを見ないまま応答する。仕切ろうとした個体は
 * 「もう答え終わっている」を知りようがなく、同じ相手が二度答える。
 *
 * 収束する形は **orchestrator-workers** — 進行役 1 体に頼み、その 1 体が
 * `ask_*` で順に委譲して答えを受け取り、まとめる。各エージェントはちょうど
 * 1 回ずつ話し、重複が構造的に起こらない（テストで固定してある）。
 * 確実なほうを既定の道にする。
 *
 * **コア側の同報機構は残っている**（`co_recipients` / 注記 / 表示集約）。
 * エージェント発の fan-out が今も使っており、剥がすとそちらが壊れる。
 * 「同じ問いを全員へ独立に投げて答えを比べる」用途は、将来それと分かる形で
 * 戻す余地がある（failures.md / data_contract 参照）。
 */
const target = ref<AgentId | null>(null);

watch(
  // 選択が無い間は先頭のエージェントを既定にする（一覧の初回ロードでも発火する）。
  () => state.selectedAgentId ?? state.agents[0]?.id ?? null,
  (id) => {
    target.value = id;
  },
  { immediate: true },
);

/** 宛先を指名する。左ペインの選択とも揃える（見ている相手と話す相手を一致させる）。 */
function selectTarget(id: AgentId): void {
  target.value = id;
  orchestrator.select(id);
}

/** 宛先のエージェント。 */
const targetAgent = computed(
  () => state.agents.find((a) => a.id === target.value) ?? null,
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
      return "システム";
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

/** 直前の発言と同じ話者か。連続していれば名前とアバターを省く。 */
function continuesPrevious(index: number): boolean {
  if (index === 0) return false;
  const previous = rows.value[index - 1].message;
  const current = rows.value[index].message;
  return (
    previous.from.kind === current.from.kind &&
    previous.from.kind !== "agent"
  ) ||
    (previous.from.kind === "agent" &&
      current.from.kind === "agent" &&
      previous.from.id === current.from.id);
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

function timestamp(ms: number): string {
  return new Date(ms).toLocaleTimeString("ja-JP", {
    hour: "2-digit",
    minute: "2-digit",
  });
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
  if (!canSend.value || !target.value) return;
  await orchestrator.send(target.value, content);
}

/**
 * 新規チャット。会話ログと全エージェントの履歴を消す（稼働・統計・
 * Memory.md は残る）。ログは現状メモリ内のみで復元不能なので確認必須。
 */
async function newChat(): Promise<void> {
  if (!rows.value.length) return;
  if (!confirm("新規チャットを開始しますか？\n会話ログと各エージェントの記憶（履歴）が消えます（復元できません）。\n長期記憶（Memory.md）と稼働状態は残ります。")) return;
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
        title="会話ログと各エージェントの履歴をリセットします（稼働状態と Memory.md は残ります）"
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
      <div
        v-for="({ message }, index) in rows"
        :key="message.id"
        class="flex gap-2"
        :class="isMine(message) ? 'flex-row-reverse' : 'flex-row'"
      >
        <!-- アバター。連続発言では場所だけ空けて揃える。
             画像が設定されていればそれを、無ければ頭文字の円を出す。 -->
        <div class="w-7 shrink-0">
          <template v-if="!continuesPrevious(index)">
            <img
              v-if="iconFor(message.from)"
              :src="iconFor(message.from)!"
              class="size-7 rounded-full object-cover ring-1 ring-line"
              :title="label(message.from)"
              :alt="label(message.from)"
            />
            <div
              v-else
              class="flex size-7 items-center justify-center rounded-full text-[11px] font-semibold text-surface-0"
              :style="{ backgroundColor: avatarHue(message.from) }"
              :title="label(message.from)"
            >
              {{ initial(message.from) }}
            </div>
          </template>
        </div>

        <div
          class="flex min-w-0 max-w-[78%] flex-col"
          :class="isMine(message) ? 'items-end' : 'items-start'"
        >
          <!-- 誰から誰へ。オーケストレーション画面なので宛先を落とさない。
               同報は「〇〇 他X名」に畳み、全宛先は hover で出す。 -->
          <p
            v-if="!continuesPrevious(index)"
            class="mb-0.5 flex gap-1 px-0.5 text-[10px] text-ink-dim"
            :title="toTitle(rows[index])"
          >
            <span class="font-medium text-ink">{{ label(message.from) }}</span>
            <span>→ {{ toLabel(rows[index]) }}</span>
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
            v-if="isRenderedAsMarkdown(message)"
            class="md-body selectable min-w-0 max-w-full rounded-2xl rounded-tl-sm bg-surface-2 px-3 py-2 text-[12px] leading-relaxed wrap-anywhere text-ink"
            @click="onMarkdownClick"
            v-html="markdownOf(message)"
          />
          <div
            v-else
            class="selectable min-w-0 max-w-full px-3 py-2 text-[12px] leading-relaxed wrap-anywhere whitespace-pre-wrap"
            :class="
              isMine(message)
                ? 'rounded-2xl rounded-tr-sm bg-accent text-surface-0'
                : 'rounded-2xl rounded-tl-sm bg-surface-2 text-ink'
            "
          >
            {{ message.content }}
          </div>

          <p class="mt-0.5 flex gap-1.5 px-0.5 text-[10px] text-ink-dim tabular-nums">
            <span>{{ timestamp(message.tsMs) }}</span>
            <span v-if="message.tokens">{{ message.tokens }} tok</span>
            <span :title="`転送 ${message.hop} 回目`">h{{ message.hop }}</span>
          </p>
        </div>
      </div>

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

    <!--
      宛先チップ。**1 体だけ**を指名する。
      複数へ同時に話しかける機能は外した — 全員のターンが並列に走り、
      誰も他の答えを見ないまま応答するため混乱する。まとめて動かしたいときは、
      進行役 1 体に頼んで `ask_*` で展開させる（orchestrator-workers）。
    -->
    <div
      class="flex shrink-0 flex-wrap items-center gap-1.5 border-t border-line px-3 pt-2 text-[10px]"
    >
      <span class="text-ink-dim">宛先</span>
      <button
        v-for="agent in state.agents"
        :key="agent.id"
        type="button"
        class="rounded-full border px-2 py-0.5 transition"
        :class="
          target === agent.id
            ? 'border-accent bg-accent/15 text-accent'
            : 'border-line text-ink-dim hover:border-ink-dim hover:text-ink'
        "
        :title="
          agent.status === 'running'
            ? `${agent.name} へ話しかける`
            : `${agent.name} は停止中（送信できません）`
        "
        @click="selectTarget(agent.id)"
      >
        <span
          class="mr-1 inline-block size-1.5 rounded-full align-middle"
          :class="agent.status === 'running' ? 'bg-run' : 'bg-ink-dim'"
        />{{ agent.name }}
      </button>
      <span class="ml-auto text-ink-dim">
        複数へ動かすときは、進行役 1 体に頼んでください
      </span>
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
