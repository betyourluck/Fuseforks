<script setup lang="ts">
/**
 * 設定ファイル（`SKILL.md` / `Memory.md` / `Construct.md`）のインラインエディタ。
 *
 * プレビューは Markdown を**描画せず原文のまま**表示する。これは手抜きではなく選択で、
 * ここで編集する対象はプロンプトそのものなので、モデルへ渡る文字列と
 * 画面上の見た目が一致していることのほうが価値が高い。
 * （HTML 描画を挟むと、任意テキストの描画による XSS 面も抱え込む。）
 */
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";

import CodeEditor from "./CodeEditor.vue";
import { askConfirm } from "../composables/useConfirm";
import { useOrchestrator } from "../composables/useOrchestrator";
import {
  CONFIG_FILE_LABELS,
  CONFIG_FILE_TEMPLATES,
  type AgentId,
  type ConfigFileKind,
} from "../types";

const props = defineProps<{
  agentId: AgentId;
  editable: boolean;
}>();

const { t } = useI18n();
const orchestrator = useOrchestrator();

const kind = ref<ConfigFileKind>("skill");
const content = ref("");
const original = ref("");
const loading = ref(false);
const saving = ref(false);

/** 未保存の変更があるか。タブ切り替え時の警告に使う。 */
const dirty = () => content.value !== original.value;

/** 種別ごとの案内文。mcp.json だけは Markdown ではなく JSON。 */
const placeholder = computed(() =>
  kind.value === "mcp"
    ? t("editor.mcpPlaceholder")
    : kind.value === "run"
      ? t("editor.runPlaceholder")
      : t("editor.markdownPlaceholder"),
);

async function load(): Promise<void> {
  loading.value = true;
  const text = await orchestrator.readConfig(props.agentId, kind.value);
  content.value = text ?? "";
  original.value = content.value;
  loading.value = false;
}

/** この種別のひな型。無ければ `null`（Markdown の 3 つ）。 */
const template = computed(() => CONFIG_FILE_TEMPLATES[kind.value] ?? null);

/**
 * ひな型を出せるのは**未入力のときだけ**。
 *
 * 既に何か書かれているところへ出すと、押したときに何が起きるかが読めない
 * （上書きなのか追記なのか）。**空のときしか出さなければ、押した結果は 1 つ。**
 */
const canUseTemplate = computed(
  () => props.editable && template.value !== null && content.value.trim() === "",
);

/** ひな型をテキストエリアへ入れる。**保存はしない** — 中身を見てから人が押す。 */
function insertTemplate(): void {
  if (!canUseTemplate.value || template.value === null) return;
  content.value = template.value;
}

/**
 * 保存できるか。**保存ボタンと `Ctrl+S` が同じ述語を見る。**
 *
 * 条件を 2 箇所に書くと、片方だけがすり抜ける形が生まれる（`Ctrl+S` だけが
 * 保存中に二重で走る / 壊れた JSON を通す）。ボタンの `:disabled` はこれの否定。
 */
const canSave = computed(() => dirty() && !saving.value);

/** `Ctrl+S`。押せないときは何もしない（ボタンが `disabled` のときと同じ）。 */
function saveFromEditor(): void {
  if (canSave.value) void save();
}

async function save(): Promise<void> {
  saving.value = true;
  const ok = await orchestrator.writeConfig(props.agentId, kind.value, content.value);
  if (ok) original.value = content.value;
  saving.value = false;
}

/** タブを切り替える。未保存なら確認する。 */
async function switchTo(next: ConfigFileKind): Promise<void> {
  if (next === kind.value) return;
  if (
    dirty() &&
    !(await askConfirm({
      title: t("editor.discardSwitchTitle"),
      message: t("editor.discardSwitchMessage"),
      confirmLabel: t("editor.discardSwitchConfirm"),
      cancelLabel: t("editor.keepEditing"),
      danger: true,
    }))
  ) {
    return;
  }
  kind.value = next;
}

watch([() => props.agentId, kind], load, { immediate: true });

// 編集モードを抜けたら未保存分を捨てて原文へ戻す。
// 編集を残したまま読み取り専用に見せると、保存済みだと誤解させる。
watch(
  () => props.editable,
  (editable) => {
    if (!editable) content.value = original.value;
  },
);
</script>

<template>
  <section class="flex min-h-0 flex-1 flex-col">
    <div class="flex items-center gap-1 border-b border-line px-3 py-1.5">
      <button
        v-for="(fileLabel, fileKind) in CONFIG_FILE_LABELS"
        :key="fileKind"
        class="rounded px-2 py-1 text-[11px] transition-colors"
        :class="
          kind === fileKind
            ? 'bg-surface-2 text-ink'
            : 'text-ink-dim hover:text-ink'
        "
        @click="switchTo(fileKind as ConfigFileKind)"
      >
        {{ fileLabel }}
      </button>

      <button
        v-if="canUseTemplate"
        class="ml-auto rounded border border-line px-2 py-0.5 text-[11px] text-ink-dim hover:border-accent hover:text-accent"
        @click="insertTemplate"
      >
        {{ $t("editor.insertTemplate") }}
      </button>
      <span v-if="dirty()" class="ml-auto text-[11px] text-warn">{{ $t("editor.unsaved") }}</span>
    </div>

    <div class="min-h-0 flex-1 p-3">
      <p v-if="loading" class="text-[11px] text-ink-dim">{{ $t("editor.loading") }}</p>

      <CodeEditor
        v-else-if="editable"
        v-model="content"
        class="h-full"
        :language="kind === 'mcp' || kind === 'run' ? 'json' : 'markdown'"
        :placeholder="placeholder"
        @save="saveFromEditor"
      />

      <pre
        v-else
        class="selectable h-full w-full overflow-auto rounded border border-line/50 bg-surface-1 p-2 font-mono text-[12px] leading-relaxed whitespace-pre-wrap break-words text-ink-dim"
        >{{ content || $t("editor.notCreated") }}</pre
      >
    </div>

    <div v-if="editable" class="flex justify-end gap-2 border-t border-line px-3 py-2">
      <button
        class="rounded px-2 py-1 text-[11px] text-ink-dim hover:text-ink disabled:opacity-40"
        :disabled="!dirty()"
        @click="content = original"
      >
        {{ $t("editor.discard") }}
      </button>
      <button
        class="rounded bg-accent px-3 py-1 text-[11px] font-medium text-surface-0 disabled:opacity-40"
        :disabled="!canSave"
        @click="save"
      >
        {{ saving ? $t("editor.saving") : $t("editor.save") }}
      </button>
    </div>
  </section>
</template>
