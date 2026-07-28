/**
 * オーケストレーターの状態を保持する単一ストア。
 *
 * Pinia を入れずにモジュールスコープの `reactive` で済ませているのは、
 * このアプリの状態が「コア層の写し」1 つしかないため。
 * **真実は常に Rust 側にあり、ここはその投影**という前提を崩さないよう、
 * 楽観的更新は最小限（トグルの即時反映のみ）に留めてある。
 */

import { onUnmounted, reactive, readonly, ref } from "vue";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import * as ipc from "../lib/ipc";
import type {
  AgentId,
  AgentMessage,
  AgentSnapshot,
  AgentSpec,
  ConfigFileKind,
  CoreEvent,
  ErrorPayload,
  ModelTemplate,
  TopologyEdge,
} from "../types";

/** Rust 側が emit するイベント名。`state.rs` の `CORE_EVENT` と一致させること。 */
const CORE_EVENT = "core://event";

/** 画面に保持する発話の上限。超えた分は古いほうから捨てる。 */
const MESSAGE_LIMIT = 500;

/** 画面右上に出す通知。 */
export interface Toast {
  id: number;
  level: "error" | "warn" | "info";
  title: string;
  detail?: string;
}

interface OrchestratorState {
  agents: AgentSnapshot[];
  edges: TopologyEdge[];
  messages: AgentMessage[];
  templates: ModelTemplate[];
  ragSources: string[];
  workspace: string;
  selectedAgentId: AgentId | null;
  toasts: Toast[];
  ready: boolean;
}

const state = reactive<OrchestratorState>({
  agents: [],
  edges: [],
  messages: [],
  templates: [],
  ragSources: [],
  workspace: "",
  selectedAgentId: null,
  toasts: [],
  ready: false,
});

let toastSeq = 0;
let unlisten: UnlistenFn | null = null;
const initialized = ref(false);

/** 通知を積む。同じ内容が連続しても潰さない（発生回数が診断の材料になるため）。 */
function pushToast(level: Toast["level"], title: string, detail?: string): void {
  const toast: Toast = { id: ++toastSeq, level, title, detail };
  state.toasts.push(toast);
  // 情報通知だけ自動で消す。失敗は消さない — 見落とすと原因追跡ができなくなる。
  if (level === "info") {
    setTimeout(() => dismissToast(toast.id), 4000);
  }
}

/** 通知を閉じる。 */
export function dismissToast(id: number): void {
  const index = state.toasts.findIndex((t) => t.id === id);
  if (index >= 0) state.toasts.splice(index, 1);
}

/** IPC 呼び出しを包み、失敗を通知へ変換する。成功なら結果、失敗なら `null`。 */
async function guard<T>(label: string, task: () => Promise<T>): Promise<T | null> {
  try {
    return await task();
  } catch (error) {
    const payload = error as ErrorPayload;
    pushToast("error", `${label}に失敗しました`, `[${payload.code}] ${payload.message}`);
    return null;
  }
}

/** 一覧をコア側から取り直す。 */
async function refreshAll(): Promise<void> {
  const [agents, edges, templates, ragSources] = await Promise.all([
    ipc.listAgents(),
    ipc.listTopology(),
    ipc.listModelTemplates(),
    ipc.listRagSources(),
  ]);
  state.agents = agents;
  state.edges = edges;
  state.templates = templates;
  state.ragSources = ragSources;
}

/** 単一エージェントの一部フィールドだけを差し替える。 */
function patchAgent(agentId: AgentId, patch: Partial<AgentSnapshot>): void {
  const index = state.agents.findIndex((a) => a.id === agentId);
  if (index >= 0) {
    state.agents[index] = { ...state.agents[index], ...patch };
  }
}

/** コアイベントを状態へ反映する。 */
function applyEvent(event: CoreEvent): void {
  switch (event.type) {
    case "agentStatusChanged":
      patchAgent(event.agentId, { status: event.status });
      break;

    case "agentStatsUpdated":
      patchAgent(event.agentId, {
        uptimeSecs: event.uptimeSecs,
        totalTokens: event.totalTokens,
      });
      break;

    case "messageSent":
      state.messages.push(event.message);
      if (state.messages.length > MESSAGE_LIMIT) {
        state.messages.splice(0, state.messages.length - MESSAGE_LIMIT);
      }
      break;

    case "topologyChanged":
      // 辺の変化は一覧全体に波及しうるので、差分適用せず取り直す。
      void refreshAll();
      break;

    case "agentFailed": {
      patchAgent(event.agentId, { lastError: event.error });
      const name =
        state.agents.find((a) => a.id === event.agentId)?.name ?? event.agentId;
      pushToast("error", `${name} が失敗しました`, `[${event.error.code}] ${event.error.message}`);
      break;
    }

    case "hopLimitReached": {
      const name =
        state.agents.find((a) => a.id === event.agentId)?.name ?? event.agentId;
      pushToast(
        "warn",
        "転送上限に達したため会話を打ち切りました",
        `${name} / 上限 ${event.maxHops} 回`,
      );
      break;
    }
  }
}

/**
 * ストアを初期化する。アプリのルートコンポーネントから 1 回だけ呼ぶ。
 *
 * 二重購読を避けるためモジュールスコープのフラグで守っている。
 * HMR で同じコンポーネントが再マウントされたときにイベントが二重に届くのを防ぐ。
 */
export function useOrchestrator() {
  async function init(): Promise<void> {
    if (initialized.value) return;
    initialized.value = true;

    // 購読を先に張る。読み込み中に発生したイベントを取りこぼさない。
    unlisten = await listen<CoreEvent>(CORE_EVENT, (e) => applyEvent(e.payload));

    await guard("初期化", async () => {
      await refreshAll();
      state.messages = await ipc.listMessages(MESSAGE_LIMIT);
      state.workspace = await ipc.workspacePath();
      state.ready = true;
    });
  }

  onUnmounted(() => {
    unlisten?.();
    unlisten = null;
    initialized.value = false;
  });

  return {
    state: readonly(state) as Readonly<OrchestratorState>,

    init,
    refreshAll: () => guard("再読み込み", refreshAll),

    /** 選択中のエージェントを切り替える。 */
    select(agentId: AgentId | null): void {
      state.selectedAgentId = agentId;
    },

    /** 選択中のエージェント（無ければ `null`）。 */
    selected(): AgentSnapshot | null {
      return state.agents.find((a) => a.id === state.selectedAgentId) ?? null;
    },

    async toggleRunning(agentId: AgentId, running: boolean): Promise<void> {
      // 楽観的に状態を進める。トグルの反応が LLM の応答待ちに引きずられないように。
      patchAgent(agentId, { status: running ? "starting" : "stopping" });
      const snapshot = await guard(running ? "起動" : "停止", () =>
        ipc.setAgentRunning(agentId, running),
      );
      if (snapshot) {
        patchAgent(agentId, snapshot);
      } else {
        // 失敗したら投影を捨てて真実を取り直す。
        void refreshAll();
      }
    },

    async createAgent(spec: AgentSpec): Promise<AgentSnapshot | null> {
      const created = await guard("エージェントの作成", () => ipc.createAgent(spec));
      if (created) await refreshAll();
      return created;
    },

    async updateAgent(spec: AgentSpec): Promise<void> {
      const updated = await guard("設定の保存", () => ipc.updateAgent(spec));
      if (updated) await refreshAll();
    },

    async deleteAgent(agentId: AgentId): Promise<void> {
      const done = await guard("エージェントの削除", () => ipc.deleteAgent(agentId));
      if (done !== null) {
        if (state.selectedAgentId === agentId) state.selectedAgentId = null;
        await refreshAll();
      }
    },

    async setConnections(agentId: AgentId, targets: AgentId[]): Promise<void> {
      await guard("接続の更新", () => ipc.setConnections(agentId, targets));
    },

    async reorder(order: AgentId[]): Promise<void> {
      // 並び替えは即座に見た目へ反映しないと操作感が壊れるので、先に order を振る。
      order.forEach((id, index) => patchAgent(id, { order: index }));
      state.agents.sort((a, b) => a.order - b.order);
      await guard("並び替え", () => ipc.reorderAgents(order));
    },

    async upsertTemplate(template: ModelTemplate): Promise<void> {
      const done = await guard("モデルテンプレートの保存", () =>
        ipc.upsertModelTemplate(template),
      );
      if (done !== null) await refreshAll();
    },

    async deleteTemplate(templateId: string): Promise<void> {
      const done = await guard("モデルテンプレートの削除", () =>
        ipc.deleteModelTemplate(templateId),
      );
      if (done !== null) await refreshAll();
    },

    readConfig: (agentId: AgentId, kind: ConfigFileKind) =>
      guard("設定ファイルの読み込み", () => ipc.readAgentConfig(agentId, kind)),

    async writeConfig(
      agentId: AgentId,
      kind: ConfigFileKind,
      content: string,
    ): Promise<boolean> {
      const done = await guard("設定ファイルの保存", () =>
        ipc.writeAgentConfig(agentId, kind, content),
      );
      if (done !== null) pushToast("info", "保存しました");
      return done !== null;
    },

    async send(agentId: AgentId, content: string): Promise<void> {
      await guard("送信", () => ipc.sendUserMessage(agentId, content));
    },

    dismissToast,
  };
}
