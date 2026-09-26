<script setup lang="ts">
/**
 * 左ペインの「判断特化」（Spec 62 D2）。サーヴァントの一覧の下に置く。
 *
 * **判断役には起動が無い**（受信箱もターンも持たない関数）ので、起動のトグル・一括起動・
 * 状態の輪・グループ・Alt+↑↓ の選択・会話の宛先のどれにも入らない。代わりに出すのは
 * 「有効かどうか」と、無効ならその理由（コアの有効の述語 `JudgeStatus` をそのまま写す）。
 *
 * 行を押すと編集ダイアログが開く。**選択状態（`selectedAgentId`）は変えない。**
 */
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";

import { useJudgeDialog } from "../composables/useJudgeDialog";
import { useOrchestrator } from "../composables/useOrchestrator";
import { deriveId, judgeStatusText, judgesInOrder, nextJudgeOrder } from "../lib/judges";

const orchestrator = useOrchestrator();
const { state } = orchestrator;
const dialog = useJudgeDialog();
const { t } = useI18n();

const judges = computed(() => judgesInOrder(state.judges));

const creating = ref(false);
const newName = ref("");

async function submit(): Promise<void> {
  const name = newName.value.trim();
  if (!name) return;
  // 名前空間は 1 つ — サーヴァントの id とも衝突させない（コアも DUPLICATE_AGENT で拒む）。
  const id = deriveId(name, [...state.agents.map((a) => a.id), ...state.judges.map((j) => j.id)], "judge");
  const ok = await orchestrator.createJudge({ id, name, order: nextJudgeOrder(state.judges) });
  if (ok) {
    newName.value = "";
    creating.value = false;
    // 作ったらそのまま雛形を開く — 規則を書くまで使えない（無効の理由も画面に出る）。
    dialog.open(id);
  }
}

function statusLine(status: Parameters<typeof judgeStatusText>[0]) {
  const text = judgeStatusText(status);
  return { ...text, label: t(text.key, text.params) };
}
</script>

<template>
  <section class="mt-4" data-judge-list>
    <div class="mb-1 flex items-center gap-1.5 text-[11px] text-ink">
      <span class="font-semibold">{{ $t("judges.heading") }}</span>
      <span class="tabular-nums text-ink-dim">{{ $t("judges.count", { count: judges.length }) }}</span>
      <span class="flex-1" />
      <button
        class="rounded px-1 text-ink-dim hover:text-accent"
        :title="$t('judges.add')"
        :aria-label="$t('judges.add')"
        @click="creating = !creating"
      >
        <svg class="size-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true">
          <path d="M12 5v14M5 12h14" />
        </svg>
      </button>
    </div>

    <form v-if="creating" class="mb-2 rounded border border-dashed border-line p-2" @submit.prevent="submit">
      <input
        v-model="newName"
        autofocus
        :placeholder="$t('judges.namePlaceholder')"
        class="w-full rounded border border-line bg-surface-1 px-2 py-1 text-[11px] outline-none focus:border-accent"
        @keydown.escape.prevent="creating = false"
      />
      <div class="mt-1.5 flex justify-end gap-2 text-[11px]">
        <button type="button" class="rounded px-2 py-0.5 text-ink-dim hover:text-ink" @click="creating = false">
          {{ $t("agentList.cancel") }}
        </button>
        <button
          type="submit"
          class="rounded bg-accent px-2 py-0.5 font-medium text-surface-0 disabled:opacity-40"
          :disabled="!newName.trim()"
        >
          {{ $t("agentList.create") }}
        </button>
      </div>
    </form>

    <p v-if="!judges.length && !creating" class="px-1 py-1 text-[11px] leading-relaxed text-ink-dim">
      {{ $t("judges.empty") }}
    </p>

    <ul class="space-y-1.5">
      <li v-for="judge in judges" :key="judge.id">
        <button
          type="button"
          class="flex w-full items-start gap-2 rounded border border-line bg-surface-1 px-2 py-1.5 text-left text-[11px] transition-colors hover:border-accent"
          :data-judge-id="judge.id"
          :title="judge.id"
          @click="dialog.open(judge.id)"
        >
          <!-- 菱形 = 判断役（地図のノードと同じ形。サーヴァントの丸いアバターと分ける）。 -->
          <svg class="mt-0.5 size-3.5 shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round" aria-hidden="true"
               :class="statusLine(judge.status).active ? 'text-accent' : 'text-ink-dim'">
            <path d="M12 3 21 12 12 21 3 12Z" />
          </svg>
          <span class="min-w-0 flex-1">
            <span class="block truncate font-medium text-ink">{{ judge.name }}</span>
            <span
              class="block break-words"
              :class="statusLine(judge.status).active ? 'text-ink-dim' : 'text-warn'"
            >
              {{ statusLine(judge.status).label }}
            </span>
          </span>
        </button>
      </li>
    </ul>
  </section>
</template>
