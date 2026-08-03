<script setup lang="ts">
/**
 * エージェント設定のモーダル。左ペインのカードの鉛筆から開く。
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
import { useI18n } from "vue-i18n";
import { openPath } from "@tauri-apps/plugin-opener";

import MarkdownEditor from "./MarkdownEditor.vue";
import { avatarHue, avatarInitial } from "../lib/avatar";
import { compactNumber } from "../lib/format";
import { fileToWebpIcon } from "../lib/iconImage";
import * as ipc from "../lib/ipc";
import { askConfirm } from "../composables/useConfirm";
import { useOrchestrator } from "../composables/useOrchestrator";
import {
  STATUS_LABEL_KEYS,
  type AgentId,
  type AgentMcpStatus,
  type AgentSpec,
} from "../types";

const props = defineProps<{ agentId: AgentId }>();
const emit = defineEmits<{ (e: "close"): void }>();

const { t } = useI18n();
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
        hearsRoomLog: source.hearsRoomLog,
        // この画面に UI は無いが、**写さないと保存で既定へ戻る**。
        // serde の既定が true なので、対象から外していた個体が
        // 設定を開いて保存しただけで黙って対象へ復帰する。
        batchStart: source.batchStart,
        // 同上。この画面に役職を変える UI は無いが（P3）、写さないと
        // 設定を保存しただけでバッジが消える。
        roleId: source.roleId,
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
 * 同梱ツールの一覧（表示順）。Rust 側の BUNDLED_TOOL_NAMES /
 * WORK_DIR_TOOL_NAMES と対応させる（手動同期の契約）。
 */
const BUNDLED_TOOLS = [
  { name: "remember", labelKey: "agentSettings.tools.remember", needsWorkDir: false },
  { name: "grep", labelKey: "agentSettings.tools.grep", needsWorkDir: true },
  { name: "fd", labelKey: "agentSettings.tools.fd", needsWorkDir: true },
  { name: "diff", labelKey: "agentSettings.tools.diff", needsWorkDir: true },
  { name: "sd", labelKey: "agentSettings.tools.sd", needsWorkDir: true },
  { name: "yq", labelKey: "agentSettings.tools.yq", needsWorkDir: true },
  {
    name: "file",
    labelKey: "agentSettings.tools.file",
    needsWorkDir: true,
  },
] as const;

/**
 * ツールのチェック状態。`enabledTools: null` は「既定に従う」= 全 ON 表示。
 * これは null の**効果の表示**であって、明示配列 7 本を保存するのではない。
 * 利用者がどれかを触った瞬間に明示配列へ切り替わる（Spec 02）。
 */
function isToolChecked(name: string): boolean {
  const list = draft.value?.enabledTools;
  return list === null || list === undefined ? true : list.includes(name);
}

/** ツールの ON/OFF。null（既定）から触ると明示配列へ切り替わる。 */
function toggleTool(name: string, checked: boolean): void {
  if (!draft.value) return;
  const current =
    draft.value.enabledTools ?? BUNDLED_TOOLS.map((tool) => tool.name as string);
  draft.value.enabledTools = checked
    ? [...current.filter((tool) => tool !== name), name]
    : current.filter((tool) => tool !== name);
}

/** 作業フォルダが無いため提示されない（チェック不能にして理由を見せる）。 */
function isToolGated(tool: (typeof BUNDLED_TOOLS)[number]): boolean {
  return tool.needsWorkDir && !draft.value?.workDir;
}

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
    JSON.stringify(current.enabledTools) !== JSON.stringify(source.enabledTools) ||
    current.hearsRoomLog !== source.hearsRoomLog
  );
});

async function save(): Promise<void> {
  if (!draft.value) return;
  await orchestrator.updateAgent(draft.value);
  // 保存結果を画面へ残す。閉じてしまうと「反映されたのか」が確かめられない。
  seed();
}

async function requestClose(): Promise<void> {
  if (
    dirty.value &&
    !(await askConfirm({
      title: t("agentSettings.discardTitle"),
      message: t("agentSettings.discardMessage"),
      confirmLabel: t("agentSettings.discardConfirm"),
      cancelLabel: t("agentSettings.discardCancel"),
      danger: true,
    }))
  ) {
    return;
  }
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
  const ok = await askConfirm({
    title: t("agentSettings.deleteTitle", { name: target.name }),
    message: t("agentSettings.deleteMessage"),
    confirmLabel: t("agentSettings.deleteAction"),
    danger: true,
  });
  if (!ok) return;
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
      t("agentSettings.openFolderFailed"),
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
    iconError.value =
      error instanceof Error ? error.message : t("agentSettings.iconConvertFailed");
  } finally {
    iconBusy.value = false;
  }
}

async function removeIcon(): Promise<void> {
  const ok = await askConfirm({
    title: t("agentSettings.iconRemoveTitle"),
    message: t("agentSettings.iconRemoveMessage"),
    confirmLabel: t("agentSettings.deleteAction"),
    danger: true,
  });
  if (!ok) return;
  await orchestrator.clearAgentIcon(props.agentId);
}

// ---- エージェント別 MCP（Spec 02） -----------------------------------------

/**
 * 個別 MCP の接続状態。接続はエージェントの稼働に紐付くため、
 * 停止中は「未接続」としか分からない（状態は永続化されない設計）。
 */
const mcpStatus = ref<AgentMcpStatus | null>(null);

async function refreshMcpStatus(): Promise<void> {
  try {
    mcpStatus.value = await ipc.agentMcpStatus(props.agentId);
  } catch {
    // 状態表示は診断用の付加情報。取得失敗でダイアログ全体を壊さない。
    mcpStatus.value = null;
  }
}

watch(() => props.agentId, refreshMcpStatus, { immediate: true });
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
            {{ agent?.name ?? $t("agentSettings.fallbackName") }}
          </h2>
          <button
            class="flex size-6 items-center justify-center rounded border border-line hover:border-accent hover:text-accent"
            :title="$t('agentSettings.openFolderTitle')"
            @click="openFolder"
          >
            <svg
              class="size-3.5"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
              aria-hidden="true"
            >
              <path d="M3 7a2 2 0 0 1 2-2h4l2 3h8a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z" />
            </svg>
          </button>
        </header>

        <div v-if="agent && draft" class="min-h-0 flex-1 overflow-y-auto p-3">
          <!-- アイコン。丸抜きのプレビューが会話・マップ・一覧と同じ見た目になる。 -->
          <div class="mb-3 flex items-center gap-3">
            <img
              v-if="iconUrl"
              :src="iconUrl"
              class="size-14 shrink-0 rounded-full object-cover ring-1 ring-line"
              :alt="$t('agentSettings.iconAlt')"
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
                  {{
                    iconBusy
                      ? $t("agentSettings.iconConverting")
                      : iconUrl
                        ? $t("agentSettings.iconChange")
                        : $t("agentSettings.iconChoose")
                  }}
                </button>
                <button
                  v-if="iconUrl"
                  class="rounded border border-fail/60 px-2 py-1 text-fail hover:bg-fail/10"
                  @click="removeIcon"
                >
                  {{ $t("agentSettings.delete") }}
                </button>
              </div>
              <p class="mt-1 text-ink-dim">{{ $t("agentSettings.iconHint") }}</p>
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
            <dt class="text-ink-dim">{{ $t("agentSettings.status") }}</dt>
            <dd>{{ $t(STATUS_LABEL_KEYS[agent.status]) }}</dd>
            <dt class="text-ink-dim">{{ $t("agentSettings.tokens") }}</dt>
            <dd class="tabular-nums">
              {{ compactNumber(agent.totalTokens) }}
            </dd>
          </dl>

          <hr class="my-3 border-line" />

          <label class="mb-1 block text-[11px] text-ink-dim">{{ $t("agentSettings.name") }}</label>
          <input
            v-model="draft.name"
            class="mb-3 w-full rounded border border-line bg-surface-0 px-2 py-1 outline-none focus:border-accent"
          />

          <label class="mb-1 block text-[11px] text-ink-dim">{{ $t("agentSettings.model") }}</label>
          <select
            v-model="draft.modelTemplateId"
            class="mb-3 w-full rounded border border-line bg-surface-0 px-2 py-1 outline-none focus:border-accent"
          >
            <option v-for="tpl in state.templates" :key="tpl.id" :value="tpl.id">
              {{ $t("agentSettings.modelOption", { name: tpl.name, model: tpl.model }) }}
            </option>
          </select>

          <label class="mb-1 block text-[11px] text-ink-dim">
            {{ $t("agentSettings.workDir") }}
          </label>
          <input
            v-model="workDirInput"
            spellcheck="false"
            :placeholder="$t('agentSettings.workDirPlaceholder')"
            class="mb-1 w-full rounded border border-line bg-surface-0 px-2 py-1 font-mono text-[11px] outline-none focus:border-accent"
          />
          <!--
            範囲がそのまま「インジェクション時に漏洩しうる範囲」になるので、
            欄の説明として明示する。強制は Rust 側（canonicalize + 前方一致）。
          -->
          <p class="mb-3 text-[10px] text-ink-dim">
            {{ $t("agentSettings.workDirHint") }}
          </p>

          <label class="mb-1 block text-[11px] text-ink-dim">
            {{ $t("agentSettings.maxTools") }}
          </label>
          <input
            v-model="maxToolIterationsInput"
            type="number"
            min="1"
            max="99"
            :placeholder="$t('agentSettings.maxToolsPlaceholder')"
            class="mb-1 w-full rounded border border-line bg-surface-0 px-2 py-1 outline-none focus:border-accent"
          />
          <p class="mb-3 text-[10px] text-ink-dim">
            {{ $t("agentSettings.maxToolsHint") }}
          </p>

          <div class="mb-1 flex items-center gap-2">
            <label class="block text-[11px] text-ink-dim">{{ $t("agentSettings.bundledTools") }}</label>
            <span class="text-[10px] text-ink-dim opacity-70">
              {{
                draft.enabledTools === null
                  ? $t("agentSettings.toolsDefault")
                  : $t("agentSettings.toolsCustom")
              }}
            </span>
            <button
              v-if="draft.enabledTools !== null"
              class="ml-auto rounded border border-line px-1.5 py-0.5 text-[10px] text-ink-dim hover:border-accent hover:text-accent"
              @click="draft.enabledTools = null"
            >
              {{ $t("agentSettings.toolsReset") }}
            </button>
          </div>
          <!--
            提示しないツールのスキーマは毎ターンの固定費。雑談役からファイル系を
            外すと基礎トークンが下がる（トークン節約はこの製品の差別化軸）。
          -->
          <div class="mb-1 space-y-1">
            <label
              v-for="tool in BUNDLED_TOOLS"
              :key="tool.name"
              class="flex items-center gap-2 text-[12px]"
              :class="isToolGated(tool) ? 'opacity-50' : ''"
            >
              <input
                type="checkbox"
                :checked="isToolChecked(tool.name)"
                :disabled="isToolGated(tool)"
                @change="toggleTool(tool.name, ($event.target as HTMLInputElement).checked)"
              />
              <span>{{ $t(tool.labelKey) }}</span>
            </label>
          </div>
          <p v-if="!draft.workDir" class="mb-1 text-[10px] text-warn">
            {{ $t("agentSettings.noWorkDirWarn") }}
          </p>
          <p v-if="!isToolChecked('remember')" class="mb-1 text-[10px] text-warn">
            {{ $t("agentSettings.rememberOffWarn") }}
          </p>
          <div class="mb-3" />

          <label class="mb-1 block text-[11px] text-ink-dim">{{ $t("agentSettings.context") }}</label>
          <label class="flex items-center gap-2 text-[12px]">
            <input type="checkbox" v-model="draft.hearsRoomLog" />
            <span>{{ $t("agentSettings.hearsRoomLog") }}</span>
          </label>
          <p v-if="!draft.hearsRoomLog" class="mt-0.5 text-[10px] text-ink-dim">
            {{ $t("agentSettings.hearsRoomLogHint") }}
          </p>
          <div class="mb-3" />

          <div class="mb-1 flex items-center gap-2">
            <label class="block text-[11px] text-ink-dim">{{ $t("agentSettings.mcp") }}</label>
            <button
              class="ml-auto rounded border border-line px-1.5 py-0.5 text-[10px] text-ink-dim hover:border-accent hover:text-accent"
              :title="$t('agentSettings.mcpRefreshTitle')"
              @click="refreshMcpStatus"
            >
              {{ $t("agentSettings.refresh") }}
            </button>
          </div>
          <!--
            接続はエージェントの稼働に紐付く（状態は永続化しない設計）。
            編集は右の設定ファイルタブの mcp.json で。
          -->
          <div class="mb-3 space-y-1 text-[11px]">
            <p v-if="!mcpStatus || !mcpStatus.running" class="text-ink-dim">
              {{ $t("agentSettings.mcpIdle") }}
            </p>
            <template v-else>
              <p v-if="mcpStatus.loadError" class="text-fail">
                {{ $t("agentSettings.mcpLoadError", { error: mcpStatus.loadError }) }}
              </p>
              <p v-else-if="!mcpStatus.servers.length" class="text-ink-dim">
                {{ $t("agentSettings.mcpEmpty") }}
              </p>
              <div
                v-for="server in mcpStatus?.servers ?? []"
                :key="server.name"
                class="rounded border border-line px-2 py-1"
              >
                <p class="flex items-center gap-1.5">
                  <span
                    class="inline-block size-1.5 rounded-full"
                    :class="server.connected ? 'bg-run' : 'bg-fail'"
                  />
                  <span class="font-medium">{{ server.name }}</span>
                  <span v-if="server.connected" class="text-ink-dim">
                    {{ $t("agentSettings.mcpToolCount", { count: server.tools.length }) }}
                  </span>
                </p>
                <p v-if="server.error" class="mt-0.5 text-[10px] text-fail">
                  {{ server.error }}
                </p>
              </div>
            </template>
          </div>

          <label class="mb-1 block text-[11px] text-ink-dim">{{ $t("agentSettings.rag") }}</label>
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
              {{ $t("agentSettings.ragEmpty") }}
            </p>
          </div>

          <label class="mb-1 block text-[11px] text-ink-dim">{{ $t("agentSettings.connections") }}</label>
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
              {{ $t("agentSettings.othersEmpty") }}
            </p>
          </div>
        </div>

        <div class="flex shrink-0 items-center gap-2 border-t border-line px-3 py-2.5">
          <button
            class="rounded border border-fail/60 px-2 py-1 text-[11px] text-fail hover:bg-fail/10"
            @click="remove"
          >
            {{ $t("agentSettings.delete") }}
          </button>
          <span v-if="dirty" class="text-[11px] text-warn">{{ $t("agentSettings.unsaved") }}</span>
          <button
            class="ml-auto rounded bg-accent px-3 py-1 text-[11px] font-medium text-surface-0 disabled:opacity-40"
            :disabled="!dirty"
            @click="save"
          >
            {{ $t("agentSettings.save") }}
          </button>
        </div>
      </div>

      <!-- 右: 設定ファイルのエディタ -->
      <div class="flex min-w-0 flex-1 flex-col">
        <header
          class="flex shrink-0 items-center border-b border-line px-3 py-2.5 text-xs"
        >
          <h3 class="flex-1 font-semibold">{{ $t("agentSettings.configFiles") }}</h3>
          <button class="px-1 text-ink-dim hover:text-ink" @click="requestClose">
            ✕
          </button>
        </header>
        <MarkdownEditor :agent-id="agentId" :editable="true" />
      </div>
    </div>
  </div>
</template>
