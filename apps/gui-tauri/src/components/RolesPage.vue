<script setup lang="ts">
/**
 * 役職ページ（Spec 14 P3）。システム設定の左メニューから開く。
 *
 * **役職は 2 役を兼ねる**（雛形とラベル）が、この画面が編集するのは雛形のほう。
 * ラベル側（バッジ・顔ぶれ）は `role_id` から表示名を引くだけで、ここには出ない。
 *
 * # なぜタイトルバーではなくここか（D1・2026-08-04 利用者裁定）
 *
 * 役職は「村の共有設定」の棚で、トークン制限・言語と同じ性質
 * （`world.json` に住み、村を配ると付いて回る）。タイトルバーは
 * 条例 / MCP / 予定 / システム設定の 4 つで既に埋まりつつあり、5 つ目を足さずに済む。
 *
 * # 編集しても既存のサーヴァントは変わらない
 *
 * `RoleDefaults` は**新規作成のときだけ**コピーされる（`role_contract` 凍結 4）。
 * ここで中身を直しても既に居る個体の設定は 1 欄も動かない。変わるのは
 * **表示名を参照しているバッジと顔ぶれだけ**。画面にもその旨を書く —
 * 書かないと「直したのに反映されない」と読まれる。
 */
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";

import * as ipc from "../lib/ipc";
import { formatError } from "../lib/errorText";
import { askConfirm } from "../composables/useConfirm";
import { useOrchestrator } from "../composables/useOrchestrator";
import type { Role } from "../types";

const { t } = useI18n();
const { state, refreshAll } = useOrchestrator();

/**
 * 同梱ツールの一覧。`AgentSettingsDialog` の `BUNDLED_TOOLS` と**同じ集合**で、
 * どちらも Rust 側の `BUNDLED_TOOL_NAMES` と手動同期する契約。
 *
 * ここでは作業フォルダの有無で伏せない — 役職は個体ではないので `work_dir` を
 * 持たず（`role_contract` 凍結 2）、どのツールが実際に提示されるかは
 * 作られた個体の設定で決まる。
 */
const BUNDLED_TOOLS = ["remember", "grep", "fd", "diff", "sd", "yq", "file"] as const;

const busy = ref(false);
const error = ref("");

/** 編集中の下書き。`null` なら一覧だけを見ている状態。 */
const draft = ref<Role | null>(null);
/** 下書きが新規か（保存ボタンの文言と、id の編集可否が変わる）。 */
const isNew = ref(false);

const roles = computed(() => state.roles);

/** 空の役職。id は名前から起こさず、明示的に入力させる（改名で参照が切れないため）。 */
function blank(): Role {
  return {
    id: "",
    name: "",
    description: "",
    defaults: {
      construct: "",
      modelTemplateId: null,
      ragSources: [],
      enabledTools: null,
      maxToolIterations: null,
    },
  };
}

function startNew(): void {
  draft.value = blank();
  isNew.value = true;
  error.value = "";
}

function startEdit(role: Role): void {
  // 深いコピー。参照のまま編集すると、取消しても一覧の表示が戻らない。
  draft.value = JSON.parse(JSON.stringify(role)) as Role;
  isNew.value = false;
  error.value = "";
}

function cancel(): void {
  draft.value = null;
  error.value = "";
}

/** id は識別子として使うので、英数字と `-` `_` に限る（Rust 側と同じ規律）。 */
const idIsValid = computed(() => /^[A-Za-z0-9_-]{1,64}$/.test(draft.value?.id ?? ""));
const canSave = computed(
  () => !busy.value && idIsValid.value && (draft.value?.name.trim().length ?? 0) > 0,
);

/** ツールのチェック状態。`enabledTools: null` = 「既定に従う」で全 ON 表示。 */
function toolChecked(tool: string): boolean {
  const list = draft.value?.defaults.enabledTools;
  return list === null || list === undefined ? true : list.includes(tool);
}

function setTool(tool: string, checked: boolean): void {
  if (!draft.value) return;
  const current = draft.value.defaults.enabledTools ?? [...BUNDLED_TOOLS];
  draft.value.defaults.enabledTools = checked
    ? [...new Set([...current, tool])]
    : current.filter((name) => name !== tool);
}

function setRagSource(source: string, enabled: boolean): void {
  if (!draft.value) return;
  const current = draft.value.defaults.ragSources;
  draft.value.defaults.ragSources = enabled
    ? [...new Set([...current, source])]
    : current.filter((name) => name !== source);
}

async function save(): Promise<void> {
  if (!draft.value || !canSave.value) return;
  busy.value = true;
  error.value = "";
  try {
    await ipc.upsertRole(draft.value);
    await refreshAll();
    draft.value = null;
  } catch (e) {
    error.value = formatError(ipc.toErrorPayload(e));
  } finally {
    busy.value = false;
  }
}

async function remove(role: Role): Promise<void> {
  // 破壊的操作なので確認する（地図の線と同じ規律）。**ただし文面が違う** —
  // 役職の削除でサーヴァントは壊れない（中身はコピー済み）ので、
  // 「消えるのはバッジだけ」を明記して過剰な不安を作らない。
  const ok = await askConfirm({
    title: t("roles.confirmDeleteTitle", { name: role.name }),
    message: t("roles.confirmDeleteBody"),
    confirmLabel: t("roles.delete"),
    // danger は立てない。**元に戻せない操作ではあるが、サーヴァントは壊れない** —
    // 中身はコピー済みで、消えるのはバッジと顔ぶれの表示だけ。赤くして
    // フォーカスを逃がすのは、実害の大きさに見合わない脅かしになる。
  });
  if (!ok) return;

  busy.value = true;
  error.value = "";
  try {
    await ipc.deleteRole(role.id);
    await refreshAll();
    if (draft.value?.id === role.id) draft.value = null;
  } catch (e) {
    error.value = formatError(ipc.toErrorPayload(e));
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div>
    <h3 class="mb-1 text-xs font-semibold text-ink">{{ $t("roles.heading") }}</h3>
    <p class="mb-3 text-ink-dim">{{ $t("roles.help") }}</p>

    <p v-if="error" class="mb-2 rounded border border-fail px-2 py-1 text-fail">{{ error }}</p>

    <!-- 一覧 -->
    <template v-if="!draft">
      <p v-if="!roles.length" class="mb-3 text-ink-dim">{{ $t("roles.empty") }}</p>
      <ul v-else class="mb-3 space-y-1">
        <li
          v-for="role in roles"
          :key="role.id"
          class="flex items-center gap-2 rounded border border-line px-2 py-1.5"
        >
          <div class="min-w-0 flex-1">
            <p class="truncate font-medium text-ink">{{ role.name }}</p>
            <p class="truncate text-ink-dim">
              {{ role.description || $t("roles.noDescription") }}
            </p>
          </div>
          <button class="rounded px-2 py-0.5 text-ink-dim hover:text-accent" @click="startEdit(role)">
            {{ $t("roles.edit") }}
          </button>
          <button
            class="rounded px-2 py-0.5 text-ink-dim hover:text-fail"
            :disabled="busy"
            @click="remove(role)"
          >
            {{ $t("roles.delete") }}
          </button>
        </li>
      </ul>

      <button
        class="rounded bg-accent px-3 py-1 font-medium text-surface-0 disabled:opacity-40"
        :disabled="busy"
        @click="startNew"
      >
        {{ $t("roles.add") }}
      </button>
    </template>

    <!-- 編集フォーム -->
    <template v-else>
      <label class="mb-2 block">
        <span class="mb-0.5 block text-ink-dim">{{ $t("roles.fields.id") }}</span>
        <input
          v-model="draft.id"
          :disabled="!isNew"
          class="w-full rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent disabled:opacity-50"
        />
        <span v-if="draft.id && !idIsValid" class="mt-0.5 block text-fail">
          {{ $t("roles.fields.idInvalid") }}
        </span>
        <span v-else class="mt-0.5 block text-ink-dim">{{ $t("roles.fields.idHelp") }}</span>
      </label>

      <label class="mb-2 block">
        <span class="mb-0.5 block text-ink-dim">{{ $t("roles.fields.name") }}</span>
        <input
          v-model="draft.name"
          class="w-full rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent"
        />
        <span class="mt-0.5 block text-ink-dim">{{ $t("roles.fields.nameHelp") }}</span>
      </label>

      <label class="mb-2 block">
        <span class="mb-0.5 block text-ink-dim">{{ $t("roles.fields.description") }}</span>
        <input
          v-model="draft.description"
          class="w-full rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent"
        />
        <span class="mt-0.5 block text-ink-dim">{{ $t("roles.fields.descriptionHelp") }}</span>
      </label>

      <p class="mb-1 mt-4 text-xs font-semibold text-ink">{{ $t("roles.defaults.heading") }}</p>
      <p class="mb-2 text-ink-dim">{{ $t("roles.defaults.help") }}</p>

      <label class="mb-2 block">
        <span class="mb-0.5 block text-ink-dim">{{ $t("roles.defaults.construct") }}</span>
        <textarea
          v-model="draft.defaults.construct"
          rows="6"
          class="w-full rounded border border-line bg-surface-1 px-2 py-1 font-mono outline-none focus:border-accent"
        ></textarea>
      </label>

      <label class="mb-2 block">
        <span class="mb-0.5 block text-ink-dim">{{ $t("roles.defaults.modelTemplate") }}</span>
        <select
          v-model="draft.defaults.modelTemplateId"
          class="w-full rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent"
        >
          <option :value="null">{{ $t("roles.defaults.noOpinion") }}</option>
          <option v-for="tpl in state.templates" :key="tpl.id" :value="tpl.id">
            {{ tpl.name }}
          </option>
        </select>
      </label>

      <label class="mb-2 block">
        <span class="mb-0.5 block text-ink-dim">{{ $t("roles.defaults.maxToolIterations") }}</span>
        <input
          v-model.number="draft.defaults.maxToolIterations"
          type="number"
          min="1"
          max="255"
          :placeholder="$t('roles.defaults.noOpinion')"
          class="w-full rounded border border-line bg-surface-1 px-2 py-1 outline-none focus:border-accent"
        />
      </label>

      <p class="mb-1 mt-3 text-ink-dim">{{ $t("roles.defaults.tools") }}</p>
      <div class="mb-2 grid grid-cols-2 gap-x-3 gap-y-1">
        <label v-for="tool in BUNDLED_TOOLS" :key="tool" class="flex items-center gap-1.5">
          <input
            type="checkbox"
            :checked="toolChecked(tool)"
            @change="setTool(tool, ($event.target as HTMLInputElement).checked)"
          />
          <span class="font-mono">{{ tool }}</span>
        </label>
      </div>

      <template v-if="state.ragSources.length">
        <p class="mb-1 mt-3 text-ink-dim">{{ $t("roles.defaults.ragSources") }}</p>
        <div class="mb-2 space-y-1">
          <label v-for="source in state.ragSources" :key="source" class="flex items-center gap-1.5">
            <input
              type="checkbox"
              :checked="draft.defaults.ragSources.includes(source)"
              @change="setRagSource(source, ($event.target as HTMLInputElement).checked)"
            />
            <span>{{ source }}</span>
          </label>
        </div>
      </template>

      <p class="mt-3 text-ink-dim">{{ $t("roles.editNote") }}</p>

      <div class="mt-3 flex justify-end gap-2">
        <button class="rounded px-2 py-1 text-ink-dim hover:text-ink" @click="cancel">
          {{ $t("roles.cancel") }}
        </button>
        <button
          class="rounded bg-accent px-3 py-1 font-medium text-surface-0 disabled:opacity-40"
          :disabled="!canSave"
          @click="save"
        >
          {{ busy ? $t("roles.saving") : $t("roles.save") }}
        </button>
      </div>
    </template>

    <p class="mt-4 border-t border-line pt-2 text-ink-dim">{{ $t("settings.villageScope") }}</p>
  </div>
</template>
