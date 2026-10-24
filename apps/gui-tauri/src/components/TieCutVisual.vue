<script setup lang="ts">
/**
 * 絆を切るダイアログの絵（2026-10-24）。確認ダイアログ（`ConfirmHost`）は個体を知らないので、
 * アバターと向きの絵はこの部品が描き、`askChoice` の `lead` / `visual` として渡す。
 *
 * - `pair`: 見出しの下の「A ⇄ B」（片方向の線なら「A → B」。アバター + id（表示名））
 * - `both`: 両方向とも切る（上下 2 本の矢と ×）
 * - `one`: 片方だけ切る（A –×→ B）
 *
 * アバターは 2 段（設定された画像 → 無ければ色と頭文字）。規則は `lib/avatar.ts` の注記のとおり。
 */
import { avatarHue, avatarInitial } from "../lib/avatar";
import { useOrchestrator } from "../composables/useOrchestrator";
import type { AgentId } from "../types";

const props = withDefaults(
  defineProps<{
    mode: "pair" | "both" | "one";
    from: AgentId;
    to: AgentId;
    /** `pair` の矢印。双方向なら ⇄、片方向なら →。 */
    bidirectional?: boolean;
  }>(),
  { bidirectional: true },
);

const { state } = useOrchestrator();

/** 表示名。引けなければ id（判断役は切る対象に来ないが、消えた直後の 1 回に備える）。 */
function nameOf(id: AgentId): string {
  return state.agents.find((a) => a.id === id)?.name ?? id;
}
</script>

<template>
  <!-- 見出しの下の「A ⇄ B」。 -->
  <div v-if="props.mode === 'pair'" class="flex flex-wrap items-center gap-x-2 gap-y-1 text-[12px] text-ink-dim">
    <template v-for="(id, index) in [props.from, props.to]" :key="id">
      <span v-if="index === 1" class="text-ink" aria-hidden="true">{{ props.bidirectional ? "⇄" : "→" }}</span>
      <span class="inline-flex items-center gap-1.5">
        <img
          v-if="state.icons[id]"
          :src="state.icons[id]!"
          alt=""
          class="size-5 shrink-0 rounded-full object-cover"
        />
        <span
          v-else
          class="flex size-5 shrink-0 items-center justify-center rounded-full text-[10px] font-semibold text-surface-0"
          :style="{ backgroundColor: avatarHue(nameOf(id)) }"
        >
          {{ avatarInitial(nameOf(id)) }}
        </span>
        <span class="selectable">{{ $t("map.agentLabel", { id, name: nameOf(id) }) }}</span>
      </span>
    </template>
  </div>

  <!-- 両方向とも切る: 上の → と下の ← を、どちらも × で断つ。 -->
  <svg v-else-if="props.mode === 'both'" viewBox="0 0 36 20" class="h-5 w-9 text-fail" aria-hidden="true">
    <g fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
      <path d="M3 6h9M24 6h9M29 3l4 3-4 3" />
      <path d="M3 14h9M24 14h9M7 11l-4 3 4 3" stroke-dasharray="2.5 2" />
      <path d="M15.5 3.5l5 5M20.5 3.5l-5 5M15.5 11.5l5 5M20.5 11.5l-5 5" />
    </g>
  </svg>

  <!-- 片方だけ切る: A –×→ B。 -->
  <span v-else class="inline-flex items-center gap-0.5" aria-hidden="true">
    <template v-for="(id, index) in [props.from, props.to]" :key="id">
      <svg v-if="index === 1" viewBox="0 0 22 12" class="h-3 w-[22px] text-fail">
        <g fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
          <path d="M1 6h5M16 6h5M18 3l3 3-3 3" />
          <path d="M8.5 3.5l5 5M13.5 3.5l-5 5" />
        </g>
      </svg>
      <img
        v-if="state.icons[id]"
        :src="state.icons[id]!"
        alt=""
        class="size-4 shrink-0 rounded-full object-cover"
      />
      <span
        v-else
        class="flex size-4 shrink-0 items-center justify-center rounded-full text-[9px] font-semibold text-surface-0"
        :style="{ backgroundColor: avatarHue(nameOf(id)) }"
      >
        {{ avatarInitial(nameOf(id)) }}
      </span>
    </template>
  </span>
</template>
