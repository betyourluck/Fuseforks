<script setup lang="ts">
/**
 * エージェント設定のモーダル。左ペインのカードの ⚙ から開く。
 *
 * 右ペインに常駐させていた頃は、読み取り専用のプレビューと ✏️ による編集モードの
 * 二段構えにしていた。誤クリックで稼働中の設定を書き換えないための保険だったが、
 * **モーダルを開く操作そのものが既に意図の表明**なので、ここでは直接編集にする。
 *
 * 下書きは別に持ち、保存するまでコアへ送らない。1 フィールド変更するたびに
 * IPC を飛ばすと、途中の不整合な状態（接続先を差し替える最中の空配列など）が
 * コアの検査に引っかかる。
 */
import { computed, ref, watch } from "vue";
import { openPath } from "@tauri-apps/plugin-opener";

import MarkdownEditor from "./MarkdownEditor.vue";
import { avatarHue, avatarInitial } from "../lib/avatar";
import { fileToWebpIcon } from "../lib/iconImage";
import { useOrchestrator } from "../composables/useOrchestrator";
import { STATUS_LABELS, type AgentId, type AgentSpec } from "../types";

const props = defineProps<{ agentId: AgentId }>();
const emit = defineEmits<{ (e: "close"): void }>();

const orchestrator = useOrchestrator();
const { state } = orchestrator;

const agent = computed(
  () => state.agents.find((a) => a.id === props.agentId) ?? null,
);

/** 自分以外のエージェント。接続先の候補。 */
const others = computed(() =>
  state.agents.filter((a) => a.id !== props.agentId),
);

const draft = ref<AgentSpec | null>(null);

/** スナップショットから下書きを起こす。 */
function seed(): void {
  const source = agent.value;
  draft.value = source
    ? {
        id: source.id,
        name: source.name,
        modelTemplateId: source.modelTemplateId,
        ragSources: [...source.ragSources],
        connectedAgents: [...source.connectedAgents],
        order: source.order,
        workDir: source.workDir,
        maxToolIterations: source.maxToolIterations,
        enabledTools: source.enabledTools ? [...source.enabledTools] : null,
      }
    : null;
}

/**
 * 作業フォルダの入力バッファ。空文字と `null`（未設定）を同一視するため、
 * 保存時に trim して空なら `null` へ落とす。
 */
const workDirInput = computed({
  get: () => draft.value?.workDir ?? "",
  set: (value: string) => {
    if (draft.value) draft.value.workDir = value.trim() || null;
  },
});

/**
 * ツール実行上限の入力バッファ。空欄 = 既定値（null）。
 * 0 以下や数値でない入力も null（既定値）へ落とす — 不正値で保存を
 * 止めるより、既定へ戻る方が復帰しやすい。
 */
const maxToolIterationsInput = computed({
  get: () => draft.value?.maxToolIterations?.toString() ?? "",
  set: (value: string) => {
    if (!draft.value) return;
    const parsed = Number.parseInt(value, 10);
    draft.value.maxToolIterations =
      Number.isFinite(parsed) && parsed >= 1 ? Math.min(parsed, 99) : null;
  },
});

watch(() => props.agentId, seed, { immediate: true });

/** 未保存の変更があるか。閉じるときの確認に使う。 */
const dirty = computed(() => {
  const source = agent.value;
  const current = draft.value;
  if (!source || !current) return false;
  return (
    current.name !== source.name ||
    current.modelTemplateId !== source.modelTemplateId ||
    current.ragSources.join() !== source.ragSources.join() ||
    current.connectedAgents.join() !== source.connectedAgents.join() ||
    current.workDir !== source.workDir ||
    current.maxToolIterations !== source.maxToolIterations ||
    JSON.stringify(current.enabledTools) !== JSON.stringify(source.enabledTools)
  );
});

async function save(): Promise<void> {
  if (!draft.value) return;
  await orchestrator.updateAgent(draft.value);
  // 保存結果を画面へ残す。閉じてしまうと「反映されたのか」が確かめられない。
  seed();
}

function requestClose(): void {
  if (dirty.value && !confirm("未保存の変更があります。破棄して閉じますか？")) return;
  emit("close");
}

function toggleConnection(targetId: AgentId, connected: boolean): void {
  if (!draft.value) return;
  draft.value.connectedAgents = connected
    ? [...draft.value.connectedAgents, targetId]
    : draft.value.connectedAgents.filter((id) => id !== targetId);
}

function toggleRag(source: string, enabled: boolean): void {
  if (!draft.value) return;
  draft.value.ragSources = enabled
    ? [...draft.value.ragSources, source]
    : draft.value.ragSources.filter((s) => s !== source);
}

async function remove(): Promise<void> {
  const target = agent.value;
  if (!target) return;
  if (!confirm(`${target.name} を削除します。設定ファイルも消えます。よろしいですか？`))
    return;
  await orchestrator.deleteAgent(target.id);
  emit("close");
}

/**
 * 設定ファイルの置き場をエクスプローラで開く。
 *
 * 失敗は通知へ落とす。ここで reject を素通しにすると Vue の ErrorBoundary
 * まで昇り、無関係な「エージェント一覧」の区画が丸ごと落ちたうえに
 * 失敗の主語まで取り違えられる（実際に起きた。failures.md #28）。
 */
async function openFolder(): Promise<void> {
  if (!state.workspace) return;
  try {
    await openPath(`${state.workspace}\\agents\\${props.agentId}`);
  } catch (error) {
    orchestrator.notify(
      "error",
      "設定フォルダを開けませんでした",
      error instanceof Error ? error.message : String(error),
    );
  }
}

// ---- アイコン ----------------------------------------------------------------

const iconInput = ref<HTMLInputElement | null>(null);
const iconBusy = ref(false);
const iconError = ref("");

const iconUrl = computed(() => state.icons[props.agentId] ?? null);

/**
 * 選択された画像を WebP へ変換して保存する。
 *
 * 変換はここ（UI 層）の責務。コアは WebP 以外を受け付けない契約なので、
 * 生の png / jpg をそのまま送る経路は最初から存在しない。
 */
async function onIconPicked(event: Event): Promise<void> {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  // 同じファイルを選び直しても change が発火するよう、先に値を消す。
  input.value = "";
  if (!file) return;

  iconBusy.value = true;
  iconError.value = "";
  try {
    const bytes = await fileToWebpIcon(file);
    await orchestrator.setAgentIcon(props.agentId, bytes);
  } catch (error) {
    // 変換の失敗（壊れた画像など）は IPC まで到達しないので、ここで表示する。
    iconError.value = error instanceof Error ? error.message : "変換に失敗しました";
  } finally {
    iconBusy.value = false;
  }
}

async function removeIcon(): Promise<void> {
  if (!confirm("アイコン画像を削除しますか？")) return;
  await orchestrator.clearAgentIcon(props.agentId);
}
</script>

<template>
  <div
    class="fixed inset-0 z-40 flex items-center justify-center bg-black/60"
    @click.self="requestClose"
  >
    <div
      class="flex h-[640px] w-[880px] overflow-hidden rounded-lg border border-line bg-surface-1 shadow-2xl"
    >
      <!-- 左: 設定フォーム -->
      <div class="flex w-[340px] shrink-0 flex-col border-r border-line">
        <header
          class="flex shrink-0 items-center gap-2 border-b border-line px-3 py-2.5 text-xs"
        >
          <h2 class="min-w-0 flex-1 truncate font-semibold">
            {{ agent?.name ?? "エージェント" }}
          </h2>
          <button
            class="rounded border border-line px-1.5 py-0.5 hover:border-accent hover:text-accent"
            title="設定フォルダを開く"
            @click="openFolder"
          >
            📁
          </button>
        </header>

        <div v-if="agent && draft" class="min-h-0 flex-1 overflow-y-auto p-3">
          <!-- アイコン。丸抜きのプレビューが会話・マップ・一覧と同じ見た目になる。 -->
          <div class="mb-3 flex items-center gap-3">
            <img
              v-if="iconUrl"
              :src="iconUrl"
              class="size-14 shrink-0 rounded-full object-cover ring-1 ring-line"
              alt="エージェントのアイコン"
            />
            <div
              v-else
              class="flex size-14 shrink-0 items-center justify-center rounded-full text-lg font-semibold text-surface-0"
              :style="{ backgroundColor: avatarHue(agent.name) }"
            >
              {{ avatarInitial(agent.name) }}
            </div>

            <div class="min-w-0 text-[11px]">
              <div class="flex gap-2">
                <button
                  class="rounded border border-line px-2 py-1 hover:border-accent hover:text-accent disabled:opacity-40"
                  :disabled="iconBusy"
                  @click="iconInput?.click()"
                >
                  {{ iconBusy ? "変換中…" : iconUrl ? "画像を変更" : "画像を選ぶ" }}
                </button>
                <button
                  v-if="iconUrl"
                  class="rounded border border-fail/60 px-2 py-1 text-fail hover:bg-fail/10"
                  @click="removeIcon"
                >
                  削除
                </button>
              </div>
              <p class="mt-1 text-ink-dim">png / jpg を選ぶと WebP に変換して保存します。</p>
              <p v-if="iconError" class="mt-0.5 text-fail">{{ iconError }}</p>
            </div>

            <input
              ref="iconInput"
              type="file"
              accept="image/png,image/jpeg"
              class="hidden"
              @change="onIconPicked"
            />
          </div>

          <dl class="grid grid-cols-[80px_minmax(0,1fr)] gap-x-2 gap-y-1 text-[11px]">
            <dt class="text-ink-dim">ID</dt>
            <dd class="selectable truncate font-mono">{{ agent.id }}</dd>
            <dt class="text-ink-dim">状態</dt>
            <dd>{{ STATUS_LABELS[agent.status] }}</dd>
            <dt class="text-ink-dim">トークン</dt>
            <dd class="tabular-nums">
              {{ agent.totalTokens.toLocaleString("ja-JP") }}
            </dd>
          </dl>

          <hr class="my-3 border-line" />

          <label class="mb-1 block text-[11px] text-ink-dim">名前</label>
          <input
            v-model="draft.name"
            class="mb-3 w-full rounded border border-line bg-surface-0 px-2 py-1 outline-none focus:border-accent"
          />

          <label class="mb-1 block text-[11px] text-ink-dim">使用モデル</label>
          <select
            v-model="draft.modelTemplateId"
            class="mb-3 w-full rounded border border-line bg-surface-0 px-2 py-1 outline-none focus:border-accent"
          >
            <option v-for="t in state.templates" :key="t.id" :value="t.id">
              {{ t.name }}（{{ t.model }}）
            </option>
          </select>

          <label class="mb-1 block text-[11px] text-ink-dim">
            作業フォルダ（grep / diff の探索範囲）
          </label>
          <input
            v-model="workDirInput"
            spellcheck="false"
            placeholder="例: D:\Projects\my-app（未設定ならツールは動きません）"
            class="mb-1 w-full rounded border border-line bg-surface-0 px-2 py-1 font-mono text-[11px] outline-none focus:border-accent"
          />
          <!--
            範囲がそのまま「インジェクション時に漏洩しうる範囲」になるので、
            欄の説明として明示する。強制は Rust 側（canonicalize + 前方一致）。
          -->
          <p class="mb-3 text-[10px] text-ink-dim">
            このフォルダの中だけを同梱ツールが読めます。機密を含むフォルダは指定しないでください。
          </p>

          <label class="mb-1 block text-[11px] text-ink-dim">
            ツール実行の上限 / ターン
          </label>
          <input
            v-model="maxToolIterationsInput"
            type="number"
            min="1"
            max="99"
            placeholder="既定: 6"
            class="mb-1 w-full rounded border border-line bg-surface-0 px-2 py-1 outline-none focus:border-accent"
          />
          <p class="mb-3 text-[10px] text-ink-dim">
            1 回の応答で使えるツールの回数。調査の多いコーディング用エージェントは
            12〜20 に上げると打ち切られにくくなります（そのぶんトークンを使います）。
          </p>

          <label class="mb-1 block text-[11px] text-ink-dim">参照 RAG</label>
          <div class="mb-3 space-y-1">
            <label
              v-for="source in state.ragSources"
              :key="source"
              class="flex items-center gap-2 text-[12px]"
            >
              <input
                type="checkbox"
                :checked="draft.ragSources.includes(source)"
                @change="toggleRag(source, ($event.target as HTMLInputElement).checked)"
              />
              <span>{{ source }}</span>
            </label>
            <p v-if="!state.ragSources.length" class="text-[11px] text-ink-dim">
              索引済みの RAG ソースがありません。
            </p>
          </div>

          <label class="mb-1 block text-[11px] text-ink-dim">接続先エージェント</label>
          <div class="space-y-1">
            <label
              v-for="other in others"
              :key="other.id"
              class="flex items-center gap-2 text-[12px]"
            >
              <input
                type="checkbox"
                :checked="draft.connectedAgents.includes(other.id)"
                @change="
                  toggleConnection(other.id, ($event.target as HTMLInputElement).checked)
                "
              />
              <span>{{ other.name }}</span>
            </label>
            <p v-if="!others.length" class="text-[11px] text-ink-dim">
              他のエージェントがいません。
            </p>
          </div>
        </div>

        <div class="flex shrink-0 items-center gap-2 border-t border-line px-3 py-2.5">
          <button
            class="rounded border border-fail/60 px-2 py-1 text-[11px] text-fail hover:bg-fail/10"
            @click="remove"
          >
            削除
          </button>
          <span v-if="dirty" class="text-[11px] text-warn">未保存</span>
          <button
            class="ml-auto rounded bg-accent px-3 py-1 text-[11px] font-medium text-surface-0 disabled:opacity-40"
            :disabled="!dirty"
            @click="save"
          >
            保存
          </button>
        </div>
      </div>

      <!-- 右: 設定ファイルのエディタ -->
      <div class="flex min-w-0 flex-1 flex-col">
        <header
          class="flex shrink-0 items-center border-b border-line px-3 py-2.5 text-xs"
        >
          <h3 class="flex-1 font-semibold">設定ファイル</h3>
          <button class="px-1 text-ink-dim hover:text-ink" @click="requestClose">
            ✕
          </button>
        </header>
        <MarkdownEditor :agent-id="agentId" :editable="true" />
      </div>
    </div>
  </div>
</template>
