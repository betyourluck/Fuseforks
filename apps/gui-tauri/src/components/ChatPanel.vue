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
import { useOrchestrator } from "../composables/useOrchestrator";
import type { AgentId, AgentMessage, Endpoint } from "../types";

const orchestrator = useOrchestrator();
const { state } = orchestrator;

const scroller = ref<HTMLElement | null>(null);
const filterAgentId = ref<AgentId | "">("");

/** 送信先。選択中のエージェントを既定にする。 */
const sendTarget = computed(
  () => state.selectedAgentId ?? state.agents[0]?.id ?? null,
);

const sendTargetName = computed(
  () => state.agents.find((a) => a.id === sendTarget.value)?.name ?? "",
);

/** 送信可能か。停止中のエージェントへは投げられない。 */
const canSend = computed(
  () =>
    !!sendTarget.value &&
    state.agents.find((a) => a.id === sendTarget.value)?.status === "running",
);

/** 送信できない理由。押せないボタンに理由を添えないと、故障と区別がつかない。 */
const blockedReason = computed(() => {
  if (!sendTarget.value) return "エージェントを選択してください";
  return `${sendTargetName.value} が稼働していません`;
});

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
  return label(endpoint).slice(0, 1);
}

/**
 * アバターの色。名前から決めるので、同じエージェントは常に同じ色になる。
 * 乱数や登録順で決めると、再起動のたびに色が入れ替わって見分けの手がかりが消える。
 */
function avatarHue(endpoint: Endpoint): string {
  const name = label(endpoint);
  let hash = 0;
  for (const char of name) hash = (hash * 31 + char.codePointAt(0)!) % 360;
  return `oklch(0.62 0.13 ${hash})`;
}

const visible = computed(() => {
  if (!filterAgentId.value) return state.messages;
  return state.messages.filter(
    (m) =>
      (m.from.kind === "agent" && m.from.id === filterAgentId.value) ||
      (m.to.kind === "agent" && m.to.id === filterAgentId.value),
  );
});

/** 直前の発言と同じ話者か。連続していれば名前とアバターを省く。 */
function continuesPrevious(index: number): boolean {
  if (index === 0) return false;
  const previous = visible.value[index - 1];
  const current = visible.value[index];
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
  () => visible.value.length,
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
  if (!canSend.value || !sendTarget.value) return;
  await orchestrator.send(sendTarget.value, content);
}
</script>

<template>
  <div class="flex h-full flex-col">
    <header
      class="flex shrink-0 items-center gap-2 border-b border-line px-3 py-2 text-xs text-ink-dim"
    >
      <h2 class="font-semibold tracking-wide text-ink">会話</h2>
      <span class="tabular-nums">{{ visible.length }} 件</span>
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
        v-for="(message, index) in visible"
        :key="message.id"
        class="flex gap-2"
        :class="isMine(message) ? 'flex-row-reverse' : 'flex-row'"
      >
        <!-- アバター。連続発言では場所だけ空けて揃える。 -->
        <div class="w-7 shrink-0">
          <div
            v-if="!continuesPrevious(index)"
            class="flex size-7 items-center justify-center rounded-full text-[11px] font-semibold text-surface-0"
            :style="{ backgroundColor: avatarHue(message.from) }"
            :title="label(message.from)"
          >
            {{ initial(message.from) }}
          </div>
        </div>

        <div
          class="flex min-w-0 max-w-[78%] flex-col"
          :class="isMine(message) ? 'items-end' : 'items-start'"
        >
          <!-- 誰から誰へ。オーケストレーション画面なので宛先を落とさない。 -->
          <p
            v-if="!continuesPrevious(index)"
            class="mb-0.5 flex gap-1 px-0.5 text-[10px] text-ink-dim"
          >
            <span class="font-medium text-ink">{{ label(message.from) }}</span>
            <span>→ {{ label(message.to) }}</span>
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

      <p v-if="!visible.length" class="py-10 text-center text-[11px] text-ink-dim">
        まだ発話がありません。
      </p>
    </div>

    <ChatInput
      :disabled="!canSend"
      :placeholder="
        sendTarget ? `${sendTargetName} へ送信…` : 'エージェントを選択してください'
      "
      :blocked-reason="blockedReason"
      @send="send"
    />
  </div>
</template>
