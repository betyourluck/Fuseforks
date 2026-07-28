<script setup lang="ts">
/**
 * 左ペインのエージェントカード 1 枚。
 *
 * メタデータは KV リストで並べる。ラベル幅を固定しているのは、
 * カードが縦に積まれたとき値の位置が揃わないと一覧として読めないため。
 */
import { computed } from "vue";

import { avatarHue, avatarInitial } from "../lib/avatar";
import { STATUS_LABELS, type AgentSnapshot } from "../types";

const props = defineProps<{
  agent: AgentSnapshot;
  selected: boolean;
  /** 設定済みアイコンの object URL。無ければ頭文字の円を出す。 */
  icon?: string | null;
}>();

const emit = defineEmits<{
  (e: "select"): void;
  (e: "configure"): void;
  (e: "toggle", running: boolean): void;
  (e: "move", direction: -1 | 1): void;
}>();

/** 状態に対応する表示色。停止・稼働・失敗が一目で分かることを優先する。 */
const statusColor = computed(() => {
  switch (props.agent.status) {
    case "running":
      return "bg-run";
    case "starting":
    case "stopping":
      return "bg-warn";
    case "failed":
      return "bg-fail";
    default:
      return "bg-line";
  }
});

/** 稼働中とみなす状態。トグルの見た目はこれで決める。 */
const isOn = computed(
  () => props.agent.status === "running" || props.agent.status === "starting",
);

/** 秒数を `1h 02m 03s` 形式にする。 */
const uptime = computed(() => {
  const total = props.agent.uptimeSecs;
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}h ${pad(m)}m ${pad(s)}s` : `${m}m ${pad(s)}s`;
});

/** 大きな数を読みやすく丸める。 */
const tokens = computed(() => props.agent.totalTokens.toLocaleString("ja-JP"));
</script>

<template>
  <article
    class="cursor-pointer rounded-md border px-3 py-2.5 transition-colors"
    :class="
      selected
        ? 'border-accent bg-surface-2'
        : 'border-line bg-surface-1 hover:bg-surface-2'
    "
    @click="emit('select')"
  >
    <header class="flex items-center gap-2">
      <!-- アバター + 状態。状態ドットはアバターの右下に重ね、面積を取らずに両方見せる。 -->
      <div class="relative shrink-0">
        <img
          v-if="icon"
          :src="icon"
          class="size-8 rounded-full object-cover ring-1 ring-line"
          :alt="agent.name"
        />
        <div
          v-else
          class="flex size-8 items-center justify-center rounded-full text-[12px] font-semibold text-surface-0"
          :style="{ backgroundColor: avatarHue(agent.name) }"
        >
          {{ avatarInitial(agent.name) }}
        </div>
        <span
          class="absolute -right-0.5 -bottom-0.5 size-2.5 rounded-full ring-2 ring-surface-1"
          :class="statusColor"
          :title="STATUS_LABELS[agent.status]"
        />
      </div>
      <h3 class="min-w-0 flex-1 truncate font-medium">{{ agent.name }}</h3>

      <!-- 設定。カード本体のクリック（選択）と混ざらないよう伝播を止める。 -->
      <button
        class="shrink-0 rounded px-1 py-0.5 text-ink-dim hover:text-accent"
        title="設定を開く"
        @click.stop="emit('configure')"
      >
        ⚙
      </button>

      <!-- 並び替え。ドラッグ&ドロップより、押した回数だけ確実に動く方式を採る。 -->
      <div class="flex flex-col leading-none opacity-60 hover:opacity-100">
        <button
          class="px-1 text-[10px] hover:text-accent"
          title="上へ"
          @click.stop="emit('move', -1)"
        >
          ▲
        </button>
        <button
          class="px-1 text-[10px] hover:text-accent"
          title="下へ"
          @click.stop="emit('move', 1)"
        >
          ▼
        </button>
      </div>

      <!-- 実行・停止トグル -->
      <button
        role="switch"
        :aria-checked="isOn"
        :title="isOn ? '停止する' : '実行する'"
        class="relative h-5 w-9 shrink-0 rounded-full transition-colors"
        :class="isOn ? 'bg-run' : 'bg-line'"
        @click.stop="emit('toggle', !isOn)"
      >
        <span
          class="absolute top-0.5 size-4 rounded-full bg-surface-0 transition-all"
          :class="isOn ? 'left-4.5' : 'left-0.5'"
        />
      </button>
    </header>

    <dl class="mt-2 grid grid-cols-[72px_minmax(0,1fr)] gap-x-2 gap-y-1 text-[11px]">
      <dt class="text-ink-dim">モデル</dt>
      <dd class="truncate" :title="agent.model">{{ agent.model }}</dd>

      <dt class="text-ink-dim">稼働時間</dt>
      <dd class="tabular-nums">{{ uptime }}</dd>

      <dt class="text-ink-dim">トークン</dt>
      <dd class="tabular-nums">{{ tokens }}</dd>

      <dt class="text-ink-dim">RAG</dt>
      <dd class="truncate">
        {{ agent.ragSources.length ? agent.ragSources.join(", ") : "—" }}
      </dd>

      <dt class="text-ink-dim">接続</dt>
      <dd class="tabular-nums">{{ agent.connectedAgents.length }} 件</dd>
    </dl>

    <!-- 失敗理由はカード内に残す。トースト任せにすると閉じた瞬間に原因が消える。 -->
    <p
      v-if="agent.lastError"
      class="mt-2 rounded border border-fail/40 bg-fail/10 px-2 py-1 text-[11px] text-fail"
      :title="agent.lastError.detail ?? undefined"
    >
      [{{ agent.lastError.code }}] {{ agent.lastError.message }}
    </p>
  </article>
</template>
