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
import { open as openDialog } from "@tauri-apps/plugin-dialog";

import MarkdownEditor from "./MarkdownEditor.vue";
import { snapshotToSpec } from "../lib/agentSpec";
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

/**
 * スナップショットから下書きを起こす。
 *
 * 全欄の複写は `snapshotToSpec` の 1 実装（Spec 29 D4）。この画面に UI の無い
 * 欄（`batchStart` / `roleId`）も**写さないと保存で既定へ戻る**が、その規律は
 * 複写の実装が 1 箇所になったことで、ここではなくあちらが持つ。
 */
function seed(): void {
  const source = agent.value;
  draft.value = source ? snapshotToSpec(source) : null;
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
 * ネイティブのフォルダ選択ダイアログで作業フォルダを選ぶ。
 *
 * **手打ちを残したまま、ボタンを足す形にする。** 打てなくすると、
 * 設定を配ったり別の端末で開いたりしたときにパスを直す手段が消える。
 *
 * 選ばれたパスは**そのまま入力欄へ入れる**（存在の検査はしない）— 実在の強制は
 * Rust 側の `canonicalize` + 前方一致が持っており、ここで先回りすると
 * 同じ規律が 2 箇所に生える。取り消し（`null`）のときは何もしない。
 */
async function pickWorkDir(): Promise<void> {
  const picked = await openDialog({
    directory: true,
    multiple: false,
    defaultPath: draft.value?.workDir ?? undefined,
  });
  if (typeof picked === "string") workDirInput.value = picked;
}

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
  // `rag` はこの一覧に**入れない**（Spec 18 D13）。提示は「参照 RAG」の宣言
  // だけが決める — 宣言 = オプトインで、チェックは同じ意図の 2 つ目の
  // スイッチだった（明示配列の個体に宣言しても出ない、を実機で踏んだ）。
  // Spec 15。**既定集合の外**なので、`enabledTools: null` でもチェックが付かない。
  // 作業フォルダは提示条件の一部だが `needsWorkDir` では表せない
  // （登録に `cwd` があれば作業フォルダ無しでも実行できる）ため false。
  { name: "run", labelKey: "agentSettings.tools.run", needsWorkDir: false },
] as const;

/**
 * `enabledTools: null`（既定に従う）で提示される集合。
 *
 * **Rust 側の `DEFAULT_ENABLED_TOOLS` と対応させる**（手動同期の契約。
 * `BUNDLED_TOOL_NAMES` と対応する `BUNDLED_TOOLS` と同じ扱い）。
 * `run` だけがここに居ないので、**更新しただけで実行能力が増えない**。
 */
const DEFAULT_ENABLED_TOOLS: readonly string[] = [
  "remember",
  "grep",
  "fd",
  "diff",
  "sd",
  "yq",
  "file",
];

/**
 * ツールのチェック状態。`enabledTools: null` は「既定に従う」= 全 ON 表示。
 * これは null の**効果の表示**であって、明示配列 7 本を保存するのではない。
 * 利用者がどれかを触った瞬間に明示配列へ切り替わる（Spec 02）。
 */
function isToolChecked(name: string): boolean {
  const list = draft.value?.enabledTools;
  // **`null` は「全部 ON」ではない。** 既定集合に居るものだけが ON
  // （Spec 15 で `run` を既定の外へ出した）。
  return list === null || list === undefined
    ? DEFAULT_ENABLED_TOOLS.includes(name)
    : list.includes(name);
}

/** ツールの ON/OFF。null（既定）から触ると明示配列へ切り替わる。 */
function toggleTool(name: string, checked: boolean): void {
  if (!draft.value) return;
  const current = draft.value.enabledTools ?? [...DEFAULT_ENABLED_TOOLS];
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
    current.hearsRoomLog !== source.hearsRoomLog ||
    current.allowHandoff !== source.allowHandoff ||
    current.planReview !== source.planReview ||
    // 役職だけを変えたときも保存できること。**入れ忘れると、選び直しても
    // 保存ボタンが有効にならず「変えられない」と読まれる。**
    current.roleId !== source.roleId
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

/**
 * 参照 RAG のフォルダを追加する（Spec 18）。作業フォルダの「参照…」と同じ
 * `dialog` プラグインで、選ばれたパスは**そのまま宣言へ入れる**（実在の検査は
 * しない — Rust 側が呼び出しごとに掛け、無効なら印を付ける。ここで先回りすると
 * 同じ規律が 2 箇所に生える）。同じフォルダの二重宣言だけは畳む。
 */
async function addRagSource(): Promise<void> {
  if (!draft.value) return;
  const picked = await openDialog({ directory: true, multiple: false });
  if (typeof picked !== "string") return;
  if (!draft.value.ragSources.includes(picked)) {
    draft.value.ragSources = [...draft.value.ragSources, picked];
  }
}

/** 宣言からフォルダを外す。**消せるのは人だけ**（rag_tool_contract）。 */
function removeRagSource(source: string): void {
  if (!draft.value) return;
  draft.value.ragSources = draft.value.ragSources.filter((s) => s !== source);
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
    class="fixed inset-0 z-40 flex items-center justify-center bg-scrim"
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

          <!--
            役職（Spec 14）。**ここで変わるのはラベルだけ** — 設定の流し込みは
            新規作成のときにしか起きない（role_contract 凍結 4。上書きは
            取り消せないので、既存の Construct.md を雛形で潰さない）。
            その旨を注記に書く — 書かないと「役職を変えたのに中身が変わらない」
            と読まれる。

            **出す条件は「村に役職がある」か「この個体が役職を持っている」。**
            前半は「選べない選択肢を並べない」ため。後半を入れないと、役職を
            全部削除した村で**個体に残った孤児の `role_id` を外す入口が消える**
            （動作には影響しないが、「選べるのに変えられない」と同じ形の穴。
            failures.md #53）。
          -->
          <template v-if="state.roles.length || draft.roleId">
            <label class="mb-1 block text-[11px] text-ink-dim">
              {{ $t("agentSettings.role") }}
            </label>
            <select
              v-model="draft.roleId"
              class="mb-1 w-full rounded border border-line bg-surface-0 px-2 py-1 outline-none focus:border-accent"
            >
              <option :value="null">{{ $t("agentSettings.noRole") }}</option>
              <option v-for="role in state.roles" :key="role.id" :value="role.id">
                {{ role.name }}
              </option>
            </select>
            <p class="mb-3 text-[10px] text-ink-dim">{{ $t("agentSettings.roleHelp") }}</p>
          </template>

          <label class="mb-1 block text-[11px] text-ink-dim">
            {{ $t("agentSettings.workDir") }}
          </label>
          <div class="mb-1 flex gap-1">
            <input
              v-model="workDirInput"
              spellcheck="false"
              :placeholder="$t('agentSettings.workDirPlaceholder')"
              class="min-w-0 flex-1 rounded border border-line bg-surface-0 px-2 py-1 font-mono text-[11px] outline-none focus:border-accent"
            />
            <!--
              手打ちは残す（配った村を別の端末で開いたときに直す手段が要る）。
              ボタンはネイティブのダイアログを開いて、選ばれたパスを欄へ入れるだけ。
            -->
            <button
              type="button"
              class="shrink-0 rounded border border-line px-2 py-1 text-[11px] hover:border-accent"
              :title="$t('agentSettings.workDirBrowse')"
              @click="pickWorkDir"
            >
              {{ $t("agentSettings.workDirBrowse") }}
            </button>
          </div>
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
          <p v-if="isToolChecked('run')" class="mb-1 text-[10px] text-warn">
            {{ $t("agentSettings.runOnWarn") }}
          </p>
          <label class="flex items-center gap-2 text-[12px]">
            <input type="checkbox" v-model="draft.allowHandoff" />
            <span>{{ $t("agentSettings.allowHandoff") }}</span>
          </label>
          <p v-if="!draft.allowHandoff" class="mt-0.5 text-[10px] text-ink-dim">
            {{ $t("agentSettings.allowHandoffHint") }}
          </p>
          <!-- plan の編集窓（Spec 43）。既定 OFF — 既存の村の plan の挙動を変えない。 -->
          <label class="mt-1 flex items-center gap-2 text-[12px]">
            <input type="checkbox" v-model="draft.planReview" />
            <span>{{ $t("agentSettings.planReview") }}</span>
          </label>
          <p v-if="draft.planReview" class="mt-0.5 text-[10px] text-ink-dim">
            {{ $t("agentSettings.planReviewHint") }}
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
            <div
              v-for="source in draft.ragSources"
              :key="source"
              class="flex items-center gap-2 text-[12px]"
            >
              <span class="min-w-0 flex-1 truncate" :title="source">{{ source }}</span>
              <button
                type="button"
                class="shrink-0 rounded border border-line px-1.5 py-0.5 text-[11px] hover:bg-panel"
                @click="removeRagSource(source)"
              >
                {{ $t("agentSettings.ragRemove") }}
              </button>
            </div>
            <p v-if="!draft.ragSources.length" class="text-[11px] text-ink-dim">
              {{ $t("agentSettings.ragEmpty") }}
            </p>
            <button
              type="button"
              class="rounded border border-line px-2 py-0.5 text-[11px] hover:bg-panel"
              @click="addRagSource"
            >
              {{ $t("agentSettings.ragBrowse") }}
            </button>
            <p class="text-[10px] text-ink-dim">{{ $t("agentSettings.ragHint") }}</p>
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
