<script setup lang="ts">
/**
 * 村の条例の編集ダイアログ。タイトルバーの 📜 から開く。
 *
 * 条例はワークスペース全体（= 村）の規則で、全エージェントのシステムプロンプト
 * 最上段に入る。規則の序列は「ベンダーの憲法（モデル側） > 村の条例 >
 * 各エージェントの個別設定」。全エージェント共通なので、モデル間の憲法差
 * （振る舞いの既定値の違い）を吸収する正規化層としても働く。
 *
 * 保存すると**次の発話から**全エージェントに反映される（再起動不要）。
 */
import { onMounted, ref } from "vue";
import { Translation as I18nT, useI18n } from "vue-i18n";

import CodeEditor from "./CodeEditor.vue";
import * as ipc from "../lib/ipc";
import { askConfirm } from "../composables/useConfirm";
import { useOrchestrator } from "../composables/useOrchestrator";

const emit = defineEmits<{ (e: "close"): void }>();

const { t } = useI18n();
const orchestrator = useOrchestrator();

const text = ref("");
/** コアから読み出した最新の保存内容。dirty 判定の基準。 */
const saved = ref("");
const loading = ref(true);
const saving = ref(false);
const loadError = ref("");

onMounted(async () => {
  try {
    const content = await ipc.readOrdinance();
    text.value = content;
    saved.value = content;
  } catch (error) {
    // 読めない状態で空のまま保存させると、既存の条例を空文字で潰しうる。
    // 編集を塞いで理由を見せる。
    loadError.value = t("ordinance.loadError", {
      error: String((error as { message?: string }).message ?? error),
    });
  } finally {
    loading.value = false;
  }
});

async function save(): Promise<void> {
  saving.value = true;
  try {
    const done = await orchestrator.saveOrdinance(text.value);
    if (done) saved.value = text.value;
  } finally {
    saving.value = false;
  }
}

async function requestClose(): Promise<void> {
  if (
    text.value !== saved.value &&
    !(await askConfirm({
      title: t("ordinance.discardCloseTitle"),
      message: t("ordinance.discardCloseMessage"),
      confirmLabel: t("ordinance.discardCloseConfirm"),
      cancelLabel: t("ordinance.keepEditing"),
      danger: true,
    }))
  ) {
    return;
  }
  emit("close");
}
</script>

<template>
  <div
    class="fixed inset-0 z-40 flex items-center justify-center bg-scrim"
    @click.self="requestClose"
  >
    <div
      class="flex h-[560px] w-[680px] flex-col overflow-hidden rounded-lg border border-line bg-surface-1 shadow-2xl"
    >
      <header class="flex shrink-0 items-center gap-2 border-b border-line px-3 py-2.5 text-xs">
        <h2 class="flex-1 font-semibold">{{ $t("ordinance.title") }}</h2>
        <button class="px-1 text-ink-dim hover:text-ink" @click="requestClose">✕</button>
      </header>

      <I18nT
        keypath="ordinance.description"
        tag="p"
        class="shrink-0 border-b border-line bg-surface-0 px-3 py-2 text-[11px] text-ink-dim"
        scope="global"
      >
        <template #rules>
          <strong class="text-ink">{{ $t("ordinance.descriptionRules") }}</strong>
        </template>
      </I18nT>

      <div class="min-h-0 flex-1 p-3">
        <p v-if="loading" class="py-8 text-center text-[11px] text-ink-dim">{{ $t("ordinance.loading") }}</p>
        <p v-else-if="loadError" class="py-8 text-center text-[11px] text-fail">
          {{ loadError }}
        </p>
        <CodeEditor
          v-else
          v-model="text"
          class="h-full"
          language="markdown"
          :placeholder="$t('ordinance.placeholder')"
        />
      </div>

      <div class="flex shrink-0 items-center gap-2 border-t border-line px-3 py-2.5">
        <span v-if="text !== saved" class="text-[11px] text-warn">{{ $t("ordinance.unsaved") }}</span>
        <button
          class="ml-auto rounded bg-accent px-3 py-1 text-[11px] font-medium text-surface-0 disabled:opacity-40"
          :disabled="loading || !!loadError || saving || text === saved"
          @click="save"
        >
          {{ saving ? $t("ordinance.saving") : $t("ordinance.save") }}
        </button>
      </div>
    </div>
  </div>
</template>
