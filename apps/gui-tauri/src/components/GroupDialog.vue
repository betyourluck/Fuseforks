<script setup lang="ts">
/**
 * グループの管理（Spec 51 D6）。作成・改名・削除だけ — **スイッチ（全体 ▶ の門）と
 * 表示/非表示は一覧の見出しに置き、ここには置かない**（二重管理にしない。査読 1-9）。
 *
 * `RoleDialog` と同じ二層（一覧 ⇄ 下書き）。下書きは捨てられる前提なので dirty 確認は
 * 付けない。**id はコアが発行する**ので、作成フォームは名前だけ。
 */
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import { formatError } from "../lib/errorText";
import * as ipc from "../lib/ipc";
import { askConfirm } from "../composables/useConfirm";
import { useOrchestrator } from "../composables/useOrchestrator";
import type { AgentGroup } from "../types";

const emit = defineEmits<{ (e: "close"): void }>();
const { t } = useI18n();
const orchestrator = useOrchestrator();
const { state, refreshAll } = orchestrator;

const busy = ref(false);
const error = ref("");
/** 編集中の名前。`null` なら一覧だけを見ている状態。 */
const draftName = ref<string | null>(null);
/** 改名の対象。`null` なら新規作成。 */
const editing = ref<AgentGroup | null>(null);

const groups = computed(() => state.groups);
const canSave = computed(() => !busy.value && (draftName.value?.trim().length ?? 0) > 0);

/** 各グループの所属数（見出しと同じ数え方 — 引けない id は数えない）。 */
function memberCount(group: AgentGroup): number {
  return state.agents.filter((a) => a.groupId === group.id).length;
}

function startNew(): void {
  editing.value = null;
  draftName.value = "";
  error.value = "";
}

function startRename(group: AgentGroup): void {
  editing.value = group;
  draftName.value = group.name;
  error.value = "";
}

function cancel(): void {
  draftName.value = null;
  editing.value = null;
  error.value = "";
}

async function save(): Promise<void> {
  if (draftName.value === null || !canSave.value) return;
  busy.value = true;
  error.value = "";
  try {
    if (editing.value) {
      await ipc.upsertGroup({ ...editing.value, name: draftName.value.trim() });
    } else {
      await ipc.createGroup(draftName.value.trim());
    }
    await refreshAll();
    cancel();
  } catch (e) {
    error.value = formatError(ipc.toErrorPayload(e));
  } finally {
    busy.value = false;
  }
}

async function remove(group: AgentGroup): Promise<void> {
  // 破壊的操作なので確認する。**文面は「サーヴァントは消えない」** — 消えるのは
  // 見出しだけで、個体は無所属の並びへ戻る（凍結 3）。
  const ok = await askConfirm({
    title: t("groups.confirmDeleteTitle", { name: group.name }),
    message: t("groups.confirmDeleteBody", { count: memberCount(group) }),
    confirmLabel: t("groups.delete"),
  });
  if (!ok) return;
  busy.value = true;
  error.value = "";
  try {
    await ipc.deleteGroup(group.id);
    await refreshAll();
    if (editing.value?.id === group.id) cancel();
  } catch (e) {
    error.value = formatError(ipc.toErrorPayload(e));
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div
    class="fixed inset-0 z-40 flex items-center justify-center bg-scrim"
    @click.self="emit('close')"
  >
    <div
      class="flex h-[420px] w-[520px] flex-col overflow-hidden rounded-lg border border-line bg-surface-1 shadow-2xl"
    >
      <header class="flex shrink-0 items-center gap-2 border-b border-line px-3 py-2.5 text-xs">
        <h2 class="flex-1 font-semibold">{{ $t("groups.title") }}</h2>
        <button
          class="px-1 text-ink-dim hover:text-ink"
          :aria-label="$t('groups.close')"
          @click="emit('close')"
        >
          ✕
        </button>
      </header>
      <p class="shrink-0 border-b border-line bg-surface-0 px-3 py-2 text-[11px] text-ink-dim">
        {{ $t("groups.help") }}
      </p>
      <div class="min-h-0 flex-1 overflow-y-auto p-3 text-[11px]">
        <p
          v-if="error"
          class="selectable mb-2 rounded border border-fail/50 bg-surface-0 p-2 text-[11px] text-fail"
        >
          {{ error }}
        </p>

        <template v-if="draftName === null">
          <p v-if="!groups.length" class="mb-3 text-ink-dim">{{ $t("groups.empty") }}</p>
          <ul v-else class="mb-3 space-y-1">
            <li
              v-for="group in groups"
              :key="group.id"
              class="flex items-center gap-2 rounded border border-line px-2 py-1.5"
            >
              <span class="min-w-0 flex-1 truncate">{{ group.name }}</span>
              <span class="tabular-nums text-ink-dim">
                {{ $t("groups.members", { count: memberCount(group) }) }}
              </span>
              <button class="rounded px-2 py-0.5 text-ink-dim hover:text-accent" @click="startRename(group)">
                {{ $t("groups.rename") }}
              </button>
              <button
                class="rounded px-2 py-0.5 text-ink-dim hover:text-fail"
                :disabled="busy"
                @click="remove(group)"
              >
                {{ $t("groups.delete") }}
              </button>
            </li>
          </ul>
          <button
            class="rounded bg-accent px-3 py-1 font-medium text-surface-0 disabled:opacity-40"
            :disabled="busy"
            @click="startNew"
          >
            {{ $t("groups.add") }}
          </button>
        </template>

        <form v-else @submit.prevent="save">
          <label class="mb-2 block">
            <span class="mb-0.5 block text-ink-dim">{{ $t("groups.name") }}</span>
            <input
              v-model="draftName"
              autofocus
              :placeholder="$t('groups.namePlaceholder')"
              class="w-full rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent"
            />
            <span class="mt-0.5 block text-ink-dim">{{ $t("groups.nameHelp") }}</span>
          </label>
          <div class="mt-3 flex justify-end gap-2">
            <button type="button" class="rounded px-2 py-1 text-ink-dim hover:text-ink" @click="cancel">
              {{ $t("groups.cancel") }}
            </button>
            <button
              type="submit"
              class="rounded bg-accent px-3 py-1 font-medium text-surface-0 disabled:opacity-40"
              :disabled="!canSave"
            >
              {{ busy ? $t("groups.saving") : editing ? $t("groups.save") : $t("groups.create") }}
            </button>
          </div>
        </form>

        <p class="mt-4 border-t border-line pt-2 text-ink-dim">{{ $t("groups.villageScope") }}</p>
      </div>
    </div>
  </div>
</template>
