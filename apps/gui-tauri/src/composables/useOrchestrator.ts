/**
 * オーケストレーターの状態を保持する単一ストア。
 *
 * Pinia を入れずにモジュールスコープの `reactive` で済ませているのは、
 * このアプリの状態が「コア層の写し」1 つしかないため。
 * **真実は常に Rust 側にあり、ここはその投影**という前提を崩さないよう、
 * 楽観的更新は最小限（トグルの即時反映のみ）に留めてある。
 */

import { reactive, readonly } from "vue";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import * as ipc from "../lib/ipc";
import { toErrorPayload } from "../lib/ipc";
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
  /** 初期化に失敗した理由。`null` なら未発生。 */
  initError: ErrorPayload | null;
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
  initError: null,
});

let toastSeq = 0;
let unlisten: UnlistenFn | null = null;
/** 既に通知済みの退避（テンプレート ID + 理由）。同じ通知を積み上げない。 */
const reportedDegradations = new Set<string>();
/**
 * 初期化済みフラグ。
 *
 * モジュールスコープに置き、**コンポーネントのアンマウントでは解除しない**。
 * `useOrchestrator()` は 8 個のコンポーネントから呼ばれるため、
 * ここで `onUnmounted` を登録すると、モーダルを 1 つ閉じただけで
 * イベント購読が切れてアプリ全体が更新を受け取らなくなる。
 * 購読はページの寿命と一致させるのが正しい。
 */
let initialized = false;

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

/**
 * 参照系の IPC を包み、失敗を通知へ変換する。成功なら結果、失敗なら `null`。
 *
 * 状態を変えない呼び出し専用。変更系は [`mutate`] を使うこと。
 */
async function guard<T>(label: string, task: () => Promise<T>): Promise<T | null> {
  try {
    return await task();
  } catch (error) {
    const payload = error as ErrorPayload;
    pushToast("error", `${label}に失敗しました`, `[${payload.code}] ${payload.message}`);
    return null;
  }
}

/**
 * 変更系の IPC を包む。**成否によらず必ずコア側から状態を取り直す。**
 *
 * 以前は呼び出しごとに「ここは再同期が要るか」を判断していた。その方式では
 * 判断を落とした経路だけが古い投影を表示し続ける。実際に接続の更新と並び替えが
 * 落ちており、削除に失敗したテンプレート行も一覧に残って再クリックを誘発した。
 *
 * 真実はコア側にあり、ここはその投影でしかない。**投影を更新するかどうかを
 * 判断の対象にしない**のが正しい。このアプリの payload は小さく、
 * 毎回取り直す代償は無視できる。
 */
async function mutate<T>(label: string, task: () => Promise<T>): Promise<T | null> {
  try {
    return await task();
  } catch (error) {
    const payload = error as ErrorPayload;
    pushToast("error", `${label}に失敗しました`, `[${payload.code}] ${payload.message}`);
    return null;
  } finally {
    // 再同期自体の失敗で、元の操作の通知を上書きしない。
    await refreshAll().catch(() => undefined);
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

    case "backendDegraded": {
      // 退避は発話のたびに起きるので、同じ理由の通知は 1 回だけ出す。
      const key = `${event.modelTemplateId}::${event.reason}`;
      if (!reportedDegradations.has(key)) {
        reportedDegradations.add(key);
        const name =
          state.templates.find((t) => t.id === event.modelTemplateId)?.name ??
          event.modelTemplateId;
        pushToast(
          "warn",
          `${name} は本物のモデルに接続できていません`,
          `${event.reason}\n⚙ の画面で API キーを登録してください。登録すればそのまま復帰します。`,
        );
      }
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
 * 失敗しても例外を投げず、{@link OrchestratorState.initError} に理由を残す。
 * ここで握り潰したまま `ready` を false に据え置くと、UI は「読み込み中」の
 * 覆いが外れないだけになり、初期化失敗と処理継続中が見分けられなくなる。
 */
async function initialize(): Promise<void> {
  if (initialized) return;
  initialized = true;
  state.initError = null;

  try {
    // 購読を先に張る。読み込み中に発生したイベントを取りこぼさない。
    unlisten?.();
    unlisten = await listen<CoreEvent>(CORE_EVENT, (e) => applyEvent(e.payload));

    await refreshAll();
    state.messages = await ipc.listMessages(MESSAGE_LIMIT);
    state.workspace = await ipc.workspacePath();
    state.ready = true;
  } catch (error) {
    // 再試行できるよう、失敗時はフラグを戻す。
    initialized = false;
    state.initError = toErrorPayload(error);
  }
}

export function useOrchestrator() {
  const init = initialize;

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
      // 確定値は mutate の再同期が上書きする。
      patchAgent(agentId, { status: running ? "starting" : "stopping" });
      await mutate(running ? "起動" : "停止", () =>
        ipc.setAgentRunning(agentId, running),
      );
    },

    async createAgent(spec: AgentSpec): Promise<AgentSnapshot | null> {
      return mutate("エージェントの作成", () => ipc.createAgent(spec));
    },

    async updateAgent(spec: AgentSpec): Promise<void> {
      await mutate("設定の保存", () => ipc.updateAgent(spec));
    },

    async deleteAgent(agentId: AgentId): Promise<void> {
      const done = await mutate("エージェントの削除", () => ipc.deleteAgent(agentId));
      if (done !== null && state.selectedAgentId === agentId) {
        state.selectedAgentId = null;
      }
    },

    async setConnections(agentId: AgentId, targets: AgentId[]): Promise<void> {
      await mutate("接続の更新", () => ipc.setConnections(agentId, targets));
    },

    async reorder(order: AgentId[]): Promise<void> {
      // 並び替えは即座に見た目へ反映しないと操作感が壊れるので、先に order を振る。
      order.forEach((id, index) => patchAgent(id, { order: index }));
      state.agents.sort((a, b) => a.order - b.order);
      await mutate("並び替え", () => ipc.reorderAgents(order));
    },

    async upsertTemplate(template: ModelTemplate): Promise<void> {
      await mutate("モデルテンプレートの保存", () =>
        ipc.upsertModelTemplate(template),
      );
    },

    /**
     * API キーを OS の資格情報ストアへ預ける。
     *
     * 秘密は引数として通り抜けるだけで、ストアには入れない。
     * ここへ控えを持つと、値を保持しない設計が UI 層で崩れる。
     */
    async setCredential(templateId: string, secret: string): Promise<boolean> {
      const done = await mutate("API キーの登録", () =>
        ipc.setModelCredential(templateId, secret),
      );
      if (done !== null) {
        pushToast("info", "API キーを登録しました");
        // 退避の通知は「もう解決したかもしれない」ので、次の失敗で出し直す。
        reportedDegradations.clear();
      }
      return done !== null;
    },

    /** API キーを資格情報ストアから削除する。 */
    async clearCredential(templateId: string): Promise<boolean> {
      const done = await mutate("API キーの削除", () =>
        ipc.clearModelCredential(templateId),
      );
      if (done !== null) reportedDegradations.clear();
      return done !== null;
    },

    async deleteTemplate(templateId: string): Promise<void> {
      await mutate("モデルテンプレートの削除", () =>
        ipc.deleteModelTemplate(templateId),
      );
    },

    readConfig: (agentId: AgentId, kind: ConfigFileKind) =>
      guard("設定ファイルの読み込み", () => ipc.readAgentConfig(agentId, kind)),

    async writeConfig(
      agentId: AgentId,
      kind: ConfigFileKind,
      content: string,
    ): Promise<boolean> {
      const done = await mutate("設定ファイルの保存", () =>
        ipc.writeAgentConfig(agentId, kind, content),
      );
      if (done !== null) pushToast("info", "保存しました");
      return done !== null;
    },

    async send(agentId: AgentId, content: string): Promise<void> {
      // 発話は MessageSent イベントで届くので、ここでの再同期は
      // 送信が拒否された場合に一覧を正しく戻すために効く。
      await mutate("送信", () => ipc.sendUserMessage(agentId, content));
    },

    dismissToast,
  };
}
