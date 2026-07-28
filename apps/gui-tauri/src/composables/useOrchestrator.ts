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
  /**
   * エージェントアイコンの object URL。`null` は「確認済みで未設定」、
   * キー不在は「未確認」。バイト列を毎描画で運ばないための投影キャッシュ。
   */
  icons: Record<AgentId, string | null>;
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
  icons: {},
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
    // 再同期自体の失敗で、元の操作の通知を上書きしない。ただし黙殺もしない —
    // 取り直しが落ちると投影が古いまま残るので、痕跡が無いと調査不能になる。
    await refreshAll().catch((error) =>
      console.warn("[useOrchestrator] 再同期に失敗しました（表示が古い可能性）:", error),
    );
  }
}

/**
 * 取り直しの世代番号。
 *
 * refreshAll は「mutate の完了時」と「コアイベント受信時」の 2 系統から呼ばれ、
 * **並行に走る**。IPC の完了順は開始順と一致しないので、古い状態を持った応答が
 * 後から着地すると新しい状態を黙って上書きする。実機では「削除したのに一覧に残る」
 * 「保存したのに表示が戻る」として現れ、再起動まで直らない（failures.md #19）。
 * 各呼び出しに世代を振り、**最後に開始した取り直しだけ**が状態を書けるようにする。
 */
let refreshEpoch = 0;

/** 一覧をコア側から取り直す。 */
async function refreshAll(): Promise<void> {
  const epoch = ++refreshEpoch;
  const [agents, edges, templates, ragSources] = await Promise.all([
    ipc.listAgents(),
    ipc.listTopology(),
    ipc.listModelTemplates(),
    ipc.listRagSources(),
  ]);

  // 自分より新しい取り直しが始まっていたら、この応答は既に過去のもの。捨てる。
  if (epoch !== refreshEpoch) return;

  state.agents = agents;
  state.edges = edges;
  state.templates = templates;
  state.ragSources = ragSources;
  await refreshIcons();
}

/** バイト列をアイコン表示用の object URL にする。 */
function iconUrlOf(bytes: number[]): string {
  return URL.createObjectURL(new Blob([new Uint8Array(bytes)], { type: "image/webp" }));
}

/**
 * アイコンの投影キャッシュを一覧と同期する。
 *
 * 取得するのは**未確認のエージェントだけ**。mutate のたびに全アイコンを
 * 引き直すと、数十 KB のバイト列が毎操作 IPC を往復する。
 * 差し替え・削除は操作したアクション側がキャッシュを直接更新する。
 */
async function refreshIcons(): Promise<void> {
  // 消えたエージェントの URL は破棄する。object URL はプロセスが持つ実リソースで、
  // 放置するとエージェントの作り直しのたびに Blob が溜まり続ける。
  const known = new Set(state.agents.map((a) => a.id));
  for (const id of Object.keys(state.icons)) {
    if (!known.has(id)) {
      const url = state.icons[id];
      if (url) URL.revokeObjectURL(url);
      delete state.icons[id];
    }
  }

  await Promise.all(
    state.agents
      .filter((a) => !(a.id in state.icons))
      .map(async (a) => {
        try {
          const bytes = await ipc.getAgentIcon(a.id);
          state.icons[a.id] = bytes && bytes.length ? iconUrlOf(bytes) : null;
        } catch {
          // アイコンは装飾であり、取得失敗で操作全体の通知を汚さない。
          state.icons[a.id] = null;
        }
      }),
  );
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
      // mutate 由来の取り直しと並行しうるが、世代ガードが後勝ち上書きを防ぐ。
      refreshAll().catch((error) =>
        console.warn("[useOrchestrator] イベント由来の再同期に失敗しました:", error),
      );
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

    /**
     * アイコンを保存する。`bytes` は UI 側で WebP へ変換済みであること。
     * 成功したらキャッシュを手元のバイト列で直接更新する（再フェッチしない）。
     */
    async setAgentIcon(agentId: AgentId, bytes: Uint8Array): Promise<boolean> {
      const done = await mutate("アイコンの保存", () =>
        ipc.setAgentIcon(agentId, Array.from(bytes)),
      );
      if (done !== null) {
        const old = state.icons[agentId];
        if (old) URL.revokeObjectURL(old);
        state.icons[agentId] = iconUrlOf(Array.from(bytes));
      }
      return done !== null;
    },

    /** アイコンを削除する。 */
    async clearAgentIcon(agentId: AgentId): Promise<boolean> {
      const done = await mutate("アイコンの削除", () =>
        ipc.clearAgentIcon(agentId),
      );
      if (done !== null) {
        const old = state.icons[agentId];
        if (old) URL.revokeObjectURL(old);
        state.icons[agentId] = null;
      }
      return done !== null;
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

    /**
     * 複数エージェントへ同じ内容を同報する。
     *
     * コアの配送 API は単一宛先のまま。同報の実体は「N 回呼ぶ」以上のものではなく、
     * 複数宛先 API をコアへ足すと部分失敗の集約という新しい契約が生まれる。
     * 各宛先の mailbox は独立なので、1 宛先の失敗は他の配送を妨げない
     * （Promise.all は最初の失敗を報告するが、他の送信自体は走り切る）。
     */
    async sendMany(agentIds: AgentId[], content: string): Promise<void> {
      await mutate("送信", () =>
        Promise.all(
          agentIds.map((id) => ipc.sendUserMessage(id, content)),
        ).then(() => undefined),
      );
    },

    dismissToast,
  };
}
