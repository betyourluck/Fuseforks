<script setup lang="ts">
/**
 * 中央ペイン下部: エージェント間の会話ログ。
 *
 * 自動スクロールは「既に最下部に居るとき」だけ行う。
 * 無条件に追従させると、過去ログを読んでいる最中に引きずり戻されて読めなくなる。
 */
import { computed, nextTick, ref, watch } from "vue";

import { useOrchestrator } from "../composables/useOrchestrator";
import type { AgentId, Endpoint } from "../types";

const orchestrator = useOrchestrator();
const { state } = orchestrator;

const scroller = ref<HTMLElement | null>(null);
const draft = ref("");
const filterAgentId = ref<AgentId | "">("");

/** 送信先。選択中のエージェントを既定にする。 */
const sendTarget = computed(
  () => state.selectedAgentId ?? state.agents[0]?.id ?? null,
);

/** 送信可能か。停止中のエージェントへは投げられない。 */
const canSend = computed(() => {
  if (!sendTarget.value || !draft.value.trim()) return false;
  const agent = state.agents.find((a) => a.id === sendTarget.value);
  return agent?.status === "running";
});

/** エンドポイントの表示名。 */
function label(endpoint: Endpoint): string {
  switch (endpoint.kind) {
    case "user":
      return "ユーザー";
    case "system":
      return "システム";
    case "agent":
      return state.agents.find((a) => a.id === endpoint.id)?.name ?? endpoint.id;
  }
}

/** エンドポイントの表示色。 */
function tone(endpoint: Endpoint): string {
  return endpoint.kind === "user" ? "text-accent" : "text-ink";
}

const visible = computed(() => {
  if (!filterAgentId.value) return state.messages;
  return state.messages.filter(
    (m) =>
      (m.from.kind === "agent" && m.from.id === filterAgentId.value) ||
      (m.to.kind === "agent" && m.to.id === filterAgentId.value),
  );
});

function timestamp(ms: number): string {
  return new Date(ms).toLocaleTimeString("ja-JP", { hour12: false });
}

watch(
  () => state.messages.length,
  async () => {
    const el = scroller.value;
    if (!el) return;
    // 追従するのは、ユーザーが既に末尾を見ているときだけ。
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 60;
    if (!atBottom) return;
    await nextTick();
    el.scrollTop = el.scrollHeight;
  },
);

async function send(): Promise<void> {
  if (!canSend.value || !sendTarget.value) return;
  const content = draft.value;
  draft.value = "";
  await orchestrator.send(sendTarget.value, content);
}
</script>

<template>
  <div class="flex h-full flex-col">
    <header
      class="flex items-center gap-2 border-b border-line px-3 py-2 text-xs text-ink-dim"
    >
      <h2 class="font-semibold tracking-wide text-ink">会話ログ</h2>
      <span class="tabular-nums">{{ visible.length }} 件</span>
      <select
        v-model="filterAgentId"
        class="ml-auto rounded border border-line bg-surface-1 px-1.5 py-0.5 outline-none focus:border-accent"
      >
        <option value="">すべて</option>
        <option v-for="agent in state.agents" :key="agent.id" :value="agent.id">
          {{ agent.name }}
        </option>
      </select>
    </header>

    <div ref="scroller" class="min-h-0 flex-1 overflow-y-auto px-3 py-2">
      <article
        v-for="message in visible"
        :key="message.id"
        class="selectable border-b border-line/50 py-1.5 last:border-0"
      >
        <div class="flex items-baseline gap-2 text-[11px]">
          <span class="font-medium" :class="tone(message.from)">
            {{ label(message.from) }}
          </span>
          <span class="text-ink-dim">→</span>
          <span :class="tone(message.to)">{{ label(message.to) }}</span>

          <span class="ml-auto flex gap-2 text-ink-dim tabular-nums">
            <span v-if="message.tokens">{{ message.tokens }} tok</span>
            <span :title="`転送 ${message.hop} 回目`">h{{ message.hop }}</span>
            <span>{{ timestamp(message.tsMs) }}</span>
          </span>
        </div>
        <p class="mt-0.5 whitespace-pre-wrap break-words leading-relaxed">
          {{ message.content }}
        </p>
      </article>

      <p v-if="!visible.length" class="py-8 text-center text-[11px] text-ink-dim">
        まだ発話がありません。
      </p>
    </div>

    <form
      class="flex items-center gap-2 border-t border-line px-3 py-2"
      @submit.prevent="send"
    >
      <input
        v-model="draft"
        :placeholder="
          sendTarget
            ? `${state.agents.find((a) => a.id === sendTarget)?.name ?? ''} へ送信…`
            : 'エージェントを選択してください'
        "
        :disabled="!sendTarget"
        class="flex-1 rounded border border-line bg-surface-1 px-2 py-1.5 outline-none focus:border-accent disabled:opacity-40"
      />
      <button
        type="submit"
        :disabled="!canSend"
        :title="canSend ? '送信' : '宛先が稼働していません'"
        class="rounded bg-accent px-3 py-1.5 font-medium text-surface-0 disabled:opacity-40"
      >
        送信
      </button>
    </form>
  </div>
</template>
