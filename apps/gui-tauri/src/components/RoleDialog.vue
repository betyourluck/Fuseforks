<script setup lang="ts">
/**
 * 役職ダイアログ。タイトルバーの「役職」から開く（Spec 14 P3）。
 *
 * **役職は 2 役を兼ねる**（雛形とラベル）が、この画面が編集するのは雛形のほう。
 * ラベル側（バッジ・顔ぶれ）は `role_id` から表示名を引くだけで、ここには出ない。
 *
 * # なぜシステム設定ではなく条例の隣か（D1・2026-08-04 実機で差し戻し）
 *
 * 初版はシステム設定の左メニューへ入れた。理由は「`world.json` に住むから
 * 村の共有設定の棚」だったが、**それは保存先で分類したということ**で、
 * Spec 13 が rev で潰したのと同じ誤り（保存先は分類ではない）。
 *
 * 正しい境界は**村の内容物か、アプリの設定か**。条例と役職は「この村が
 * どういう村か」を決めるもので、言語やトークン制限のような「アプリがどう
 * 振る舞うか」ではない。`world.json` に住むのは条例も言語も同じで、
 * そこは分類の根拠にならない。
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

import CodeEditor from "./CodeEditor.vue";

import * as ipc from "../lib/ipc";
import { formatError } from "../lib/errorText";
import { askConfirm } from "../composables/useConfirm";
import { useOrchestrator } from "../composables/useOrchestrator";
import type { Role, RoleColor } from "../types";

const emit = defineEmits<{ (e: "close"): void }>();

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

/**
 * バッジの色（Spec 14）。**閉じた列挙**で、色値は `style.css` の
 * `--color-role-*` にしかない。自由入力にすると暗い背景に暗い色を選べてしまい、
 * 読めないバッジが作れる — 明度と彩度を固定して色相だけ変える形にしてある。
 *
 * 並びは色相の順。**Rust 側の `RoleColor` と 1 対 1**（手動同期の契約）。
 */
const ROLE_COLORS: RoleColor[] = [
  "red",
  "orange",
  "amber",
  "green",
  "teal",
  "blue",
  "violet",
  "pink",
];

/** 色見本の CSS 値。`roleLabel.ts` と同じ規則で組む（生の色を書かない）。 */
function swatch(color: RoleColor): string {
  return `var(--color-role-${color})`;
}

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
    color: null,
    defaults: {
      construct: "",
      modelTemplateId: null,
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
  <div
    class="fixed inset-0 z-40 flex items-center justify-center bg-scrim"
    @click.self="emit('close')"
  >
    <div
      class="flex h-[560px] w-[680px] flex-col overflow-hidden rounded-lg border border-line bg-surface-1 shadow-2xl"
    >
      <header class="flex shrink-0 items-center gap-2 border-b border-line px-3 py-2.5 text-xs">
        <h2 class="flex-1 font-semibold">{{ $t("roles.title") }}</h2>
        <button
          class="px-1 text-ink-dim hover:text-ink"
          :aria-label="$t('roles.close')"
          @click="emit('close')"
        >
          ✕
        </button>
      </header>

      <!--
        説明の帯。**条例 / 共通 MCP / スケジュールと同じ形**（見出しの下に
        `bg-surface-0` の 1 段）。ここだけ本文内に見出しと説明を置いていたのを
        揃えた（2026-08-04 実機の指摘）。`h2` と同じ語の `h3` も落とす —
        タイトルが「役職」なのに本文の先頭でもう一度「役職」と書いていた。
      -->
      <p
        class="shrink-0 border-b border-line bg-surface-0 px-3 py-2 text-[11px] text-ink-dim"
      >
        {{ $t("roles.help") }}
      </p>

      <div class="min-h-0 flex-1 overflow-y-auto p-3 text-[11px]">
        <!-- エラーの書式もスケジュール／MCP と揃える（selectable = 貼って報告できる）。 -->
        <p
          v-if="error"
          class="selectable mb-2 rounded border border-fail/50 bg-surface-0 p-2 text-[11px] text-fail"
        >
          {{ error }}
        </p>

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
                <p class="truncate">
                  <span
                    class="rounded border px-1 py-px text-[10px] leading-none"
                    :class="role.color ? '' : 'border-line text-ink-dim'"
                    :style="
                      role.color
                        ? { borderColor: swatch(role.color), color: swatch(role.color) }
                        : undefined
                    "
                  >
                    {{ role.name }}
                  </span>
                </p>
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

          <div class="mb-2">
            <span class="mb-0.5 block text-ink-dim">{{ $t("roles.fields.color") }}</span>
            <div class="flex flex-wrap items-center gap-1.5">
              <button
                type="button"
                class="rounded border px-1.5 py-0.5 text-[10px] leading-none"
                :class="
                  draft.color === null ? 'border-accent text-accent' : 'border-line text-ink-dim'
                "
                @click="draft.color = null"
              >
                {{ $t("roles.fields.noColor") }}
              </button>
              <!--
                色見本は**バッジそのものの見た目**で出す — 小さな丸で選ばせると、
                選んだ結果が一覧や地図でどう見えるかが選ぶ時点で分からない。
              -->
              <button
                v-for="c in ROLE_COLORS"
                :key="c"
                type="button"
                class="rounded border px-1.5 py-0.5 text-[10px] leading-none"
                :style="{ borderColor: swatch(c), color: swatch(c) }"
                :class="draft.color === c ? 'ring-1' : 'opacity-60'"
                :title="c"
                @click="draft.color = c"
              >
                {{ draft.name || $t("roles.fields.colorSample") }}
              </button>
            </div>
            <span class="mt-0.5 block text-ink-dim">{{ $t("roles.fields.colorHelp") }}</span>
          </div>

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

          <div class="mb-2">
            <span class="mb-0.5 block text-ink-dim">{{ $t("roles.defaults.construct") }}</span>
            <div class="h-48 overflow-hidden rounded border border-line">
              <CodeEditor
                v-model="draft.defaults.construct"
                class="h-full"
                language="markdown"
              />
            </div>
          </div>

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

          <!-- 参照 RAG は雛形に入れない（Spec 18 D10）。フォルダの絶対パスは
               端末ごとに違い、雛形に入れると村を配ったとき壊れた参照を配る —
               work_dir を入れなかった理由と同じ。 -->
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

        <p class="mt-4 border-t border-line pt-2 text-ink-dim">{{ $t("roles.villageScope") }}</p>
  
      </div>
    </div>
  </div>
</template>
