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
import { CONFIG_FILE_LABELS, type AgentId, type ConfigFileKind } from "../types";

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
  kind.value === "mcp" ? t("editor.mcpPlaceholder") : t("editor.markdownPlaceholder"),
);

async function load(): Promise<void> {
  loading.value = true;
  const text = await orchestrator.readConfig(props.agentId, kind.value);
  content.value = text ?? "";
  original.value = content.value;
  loading.value = false;
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

      <span v-if="dirty()" class="ml-auto text-[11px] text-warn">{{ $t("editor.unsaved") }}</span>
    </div>

    <div class="min-h-0 flex-1 p-3">
      <p v-if="loading" class="text-[11px] text-ink-dim">{{ $t("editor.loading") }}</p>

      <CodeEditor
        v-else-if="editable"
        v-model="content"
        class="h-full"
        :language="kind === 'mcp' ? 'json' : 'markdown'"
        :placeholder="placeholder"
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
        :disabled="!dirty() || saving"
        @click="save"
      >
        {{ saving ? $t("editor.saving") : $t("editor.save") }}
      </button>
    </div>
  </section>
</template>
