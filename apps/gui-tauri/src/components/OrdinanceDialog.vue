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

import CodeEditor from "./CodeEditor.vue";
import * as ipc from "../lib/ipc";
import { useOrchestrator } from "../composables/useOrchestrator";

const emit = defineEmits<{ (e: "close"): void }>();

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
    loadError.value = `条例を読み込めませんでした: ${String(
      (error as { message?: string }).message ?? error,
    )}`;
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

function requestClose(): void {
  if (text.value !== saved.value && !confirm("未保存の変更があります。破棄して閉じますか？"))
    return;
  emit("close");
}
</script>

<template>
  <div
    class="fixed inset-0 z-40 flex items-center justify-center bg-black/60"
    @click.self="requestClose"
  >
    <div
      class="flex h-[560px] w-[680px] flex-col overflow-hidden rounded-lg border border-line bg-surface-1 shadow-2xl"
    >
      <header class="flex shrink-0 items-center gap-2 border-b border-line px-3 py-2.5 text-xs">
        <h2 class="flex-1 font-semibold">村の条例</h2>
        <button class="px-1 text-ink-dim hover:text-ink" @click="requestClose">✕</button>
      </header>

      <p class="shrink-0 border-b border-line bg-surface-0 px-3 py-2 text-[11px] text-ink-dim">
        この場の<strong class="text-ink">全サーヴァントに共通で適用される規則</strong>です。
        各サーヴァントのシステムプロンプトの最上段（個別設定より上）に入り、
        保存すると次の発話から反映されます。モデルごとの振る舞いの差を
        揃えたいときの共通ルールもここに書けます。
      </p>

      <div class="min-h-0 flex-1 p-3">
        <p v-if="loading" class="py-8 text-center text-[11px] text-ink-dim">読み込み中…</p>
        <p v-else-if="loadError" class="py-8 text-center text-[11px] text-fail">
          {{ loadError }}
        </p>
        <CodeEditor
          v-else
          v-model="text"
          class="h-full"
          language="markdown"
          placeholder="例:&#10;ここは Outcasts 村です。&#10;- 雰囲気だけの感想より、実装の中身や検証できる話を大事にする&#10;- 相手の発言を繰り返すくらいなら会話を終える&#10;- 丁寧だが率直に。忖度はしない"
        />
      </div>

      <div class="flex shrink-0 items-center gap-2 border-t border-line px-3 py-2.5">
        <span v-if="text !== saved" class="text-[11px] text-warn">未保存</span>
        <button
          class="ml-auto rounded bg-accent px-3 py-1 text-[11px] font-medium text-surface-0 disabled:opacity-40"
          :disabled="loading || !!loadError || saving || text === saved"
          @click="save"
        >
          {{ saving ? "保存中…" : "保存" }}
        </button>
      </div>
    </div>
  </div>
</template>
