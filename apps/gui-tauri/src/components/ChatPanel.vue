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

import ChatInput from "./ChatInput.vue";
import { avatarHue as hueOfName, avatarInitial } from "../lib/avatar";
import { collapseRows, type ChatRow } from "../lib/chatRows";
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

function timestamp(ms: number): string {
  return new Date(ms).toLocaleTimeString("ja-JP", {
    hour: "2-digit",
    minute: "2-digit",
  });
}

watch(
  () => rows.value.length,
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
</script>

<template>
  <div class="flex h-full flex-col">
    <header
      class="flex shrink-0 items-center gap-2 border-b border-line px-3 py-2 text-xs text-ink-dim"
    >
      <h2 class="font-semibold tracking-wide text-ink">会話</h2>
      <span class="tabular-nums">{{ rows.length }} 件</span>
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

    <div ref="scroller" class="min-h-0 flex-1 space-y-1.5 overflow-y-auto px-3 py-3">
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

          <div
            class="selectable px-3 py-2 text-[12px] leading-relaxed break-words whitespace-pre-wrap"
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

      <p v-if="!rows.length" class="py-10 text-center text-[11px] text-ink-dim">
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
