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
import { useOrchestrator } from "../composables/useOrchestrator";
import type { AgentId, AgentMessage, Endpoint } from "../types";

const orchestrator = useOrchestrator();
const { state } = orchestrator;

const scroller = ref<HTMLElement | null>(null);
const filterAgentId = ref<AgentId | "">("");

/**
 * 送信の宛先。**複数選べる**（同報）。
 *
 * オーケストレーションの画面なので、「複数のエージェントへ同時に話しかけて
 * 並列に走らせる」が基本操作になる。左ペインの選択には追随するが、
 * 選択切替は「その相手と話す」という意図の表明なので、同報の組は作り直す。
 */
const targets = ref<AgentId[]>([]);

watch(
  // 選択が無い間は先頭のエージェントを既定にする（一覧の初回ロードでも発火する）。
  () => state.selectedAgentId ?? state.agents[0]?.id ?? null,
  (id) => {
    targets.value = id ? [id] : [];
  },
  { immediate: true },
);

/** 宛先チップの増減。外して宛先ゼロにもできる（送信ボタン側が理由を出す）。 */
function toggleTarget(id: AgentId): void {
  targets.value = targets.value.includes(id)
    ? targets.value.filter((t) => t !== id)
    : [...targets.value, id];
}

/** 停止中の宛先。混ざっていると送信できないので、名前で指摘する。 */
const stoppedTargets = computed(() =>
  targets.value
    .map((id) => state.agents.find((a) => a.id === id))
    .filter((a) => a && a.status !== "running"),
);

/** 送信可能か。停止中のエージェントへは投げられない。 */
const canSend = computed(
  () => targets.value.length > 0 && stoppedTargets.value.length === 0,
);

/** 送信できない理由。押せないボタンに理由を添えないと、故障と区別がつかない。 */
const blockedReason = computed(() => {
  if (!targets.value.length) return "宛先を選択してください";
  const names = stoppedTargets.value.map((a) => a!.name).join("、");
  return `${names} が稼働していません`;
});

/** 入力欄のプレースホルダ。宛先が多いときは名前を数に畳む。 */
const placeholder = computed(() => {
  if (!targets.value.length) return "宛先を選択してください";
  const names = targets.value
    .map((id) => state.agents.find((a) => a.id === id)?.name ?? id)
    .filter(Boolean);
  if (names.length <= 2) return `${names.join("、")} へ送信…`;
  return `${names.length} 体へ同報…`;
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

/** 表示の 1 行。同報では 1 通の代表 + 残りの宛先を束ねる。 */
interface Row {
  message: AgentMessage;
  /** 代表の宛先以外。同報でなければ空。 */
  extraTargets: Endpoint[];
}

/**
 * 同報されたユーザー発話を表示上 1 つに畳む。
 *
 * 同報はログ上「同じ内容のユーザー発話 × 宛先数」として記録される（配送の実体が
 * 宛先ごとに独立なため。これは正しい）。しかし表示でそのまま並べると、
 * 同じ文面の吹き出しが人数分連続して**壊れているように見える**。
 * 表示だけを畳み、宛先は「あなた → 〇〇 他X名」に集約する。
 *
 * 畳むのは**連続する**ユーザー発話で内容が同一のものだけ。時間を置いて
 * 同じ文を送り直した場合は別の発話として残る（送り直しの事実を消さない）。
 */
const rows = computed<Row[]>(() => {
  const result: Row[] = [];
  for (const message of visible.value) {
    const previous = result[result.length - 1];
    if (
      previous &&
      message.from.kind === "user" &&
      previous.message.from.kind === "user" &&
      previous.message.content === message.content
    ) {
      previous.extraTargets.push(message.to);
      continue;
    }
    result.push({ message, extraTargets: [] });
  }
  return result;
});

/** 宛先の表示。同報は「〇〇 他X名」。 */
function toLabel(row: Row): string {
  const first = label(row.message.to);
  if (!row.extraTargets.length) return first;
  return `${first} 他${row.extraTargets.length}名`;
}

/** 同報の全宛先（hover で確認できるように）。 */
function toTitle(row: Row): string {
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
  if (!canSend.value) return;
  await orchestrator.sendMany([...targets.value], content);
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

    <!-- 宛先チップ。複数選ぶと同報になり、各エージェントが並列に走る。 -->
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
          targets.includes(agent.id)
            ? 'border-accent bg-accent/15 text-accent'
            : 'border-line text-ink-dim hover:border-ink-dim hover:text-ink'
        "
        :title="
          agent.status === 'running'
            ? targets.includes(agent.id)
              ? '宛先から外す'
              : '宛先に加える'
            : `${agent.name} は停止中（宛先に含めると送信できません）`
        "
        @click="toggleTarget(agent.id)"
      >
        <span
          class="mr-1 inline-block size-1.5 rounded-full align-middle"
          :class="agent.status === 'running' ? 'bg-run' : 'bg-ink-dim'"
        />{{ agent.name }}
      </button>
    </div>

    <ChatInput
      :disabled="!canSend"
      :placeholder="placeholder"
      :blocked-reason="blockedReason"
      @send="send"
    />
  </div>
</template>
