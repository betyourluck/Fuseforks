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
import { formatError } from "../lib/errorText";
import { i18n, setLocale } from "../i18n";
import type { ToolRun } from "../lib/chatRows";
import type {
  AgentId,
  AgentMessage,
  ForkPoint,
  AgentSnapshot,
  AgentSpec,
  ConfigFileKind,
  CoreEvent,
  ErrorPayload,
  McpConfig,
  ModelTemplate,
  Role,
  PlanWaveRecord,
  SessionSummary,
  TopologyPosition,
  TopologyEdge,
} from "../types";

/** Rust 側が emit するイベント名。`state.rs` の `CORE_EVENT` と一致させること。 */
const CORE_EVENT = "core://event";

/** 画面に保持する発話の上限。超えた分は古いほうから捨てる。 */
const MESSAGE_LIMIT = 500;

/** 保持する波の上限。コアのリング（PLAN_WAVE_CAPACITY）と同じ規則で切る。 */
const PLAN_WAVE_LIMIT = 50;

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
  topologyPositions: Record<AgentId, TopologyPosition>;
  messages: AgentMessage[];
  templates: ModelTemplate[];
  /**
   * 登録済みの役職（Spec 14）。バッジの表示名はここから引く。
   *
   * **役職の目録はプロンプトに入らない**（role_contract 凍結 6）。読み手は
   * 「どの雛形を選ぶか」を決める人だけなので、投影も画面のためだけに持つ。
   */
  roles: Role[];
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
  /**
   * 応答を作っている最中のエージェント。会話ペインの「入力中…」表示に使う。
   * true のキーだけを持ち、終了イベントでキーごと消す。
   */
  typing: Record<AgentId, true>;
  /**
   * 打ち切り要求中のエージェント（Spec 10）。停止ボタンを押してから、
   * コアが周回境界で検知するまでの間の表示に使う — 検知は飛行中の
   * LLM 呼び出しの完走を待つので、数秒〜分単位の遅れがありうる。
   * この間なにも変わらないと「押せていない」ように見える。
   * `turnInterrupted` か `agentTyping` の終了（完走で逃げ切った場合）で消す。
   */
  interruptPending: Record<AgentId, true>;
  /**
   * 実行されたツールの履歴。会話の時系列に混ぜて表示する。
   *
   * **エージェントが何をしたかは会話ログに現れない** — ツールの結果は
   * プロンプトの中で消えるので、発話だけを見ていると「黙って副作用だけ
   * 起きた」状態と区別がつかない。コアはそのために `toolInvoked` を撒いており、
   * ここがその受け手。
   *
   * 会話ログと同じ上限で切る（`MESSAGE_LIMIT`）。
   */
  toolRuns: ToolRun[];
  /**
   * エージェントごとの直近のツール実行。カードに出す。
   *
   * 履歴（`toolRuns`）とは別に持つ。一覧は「今この個体が何をしているか」を
   * 知りたい場所で、時系列を遡る場所ではない。
   */
  lastTool: Record<AgentId, ToolRun>;
  /**
   * plan 波の記録（Spec 08 — 波ペイン）。古い順。
   *
   * 突き合わせの鍵は planId、タスクの鍵は (planId, to)。新規チャットでは
   * **消さない** — reset_rule が消すのは会話ログと履歴の 2 つだけで、
   * 波の記録は統計と同じ「起きた事実の観測」の側（コアも消していない）。
   */
  planWaves: PlanWaveRecord[];
  /**
   * 保存されている会話の一覧（Spec 12。`updatedAt` の新しい順）。
   *
   * **常時は追いかけない** — 一覧ダイアログを開いたときと、会話が切り替わった
   * ときだけ取り直す。発話のたびに `updatedAt` が動くので、毎イベントで
   * 引くと会話中ずっと IPC が往復する（表示していない一覧のために）。
   */
  sessions: SessionSummary[];
  /** いま開いている会話の ID。保存先が開けていない村では空文字。 */
  currentSessionId: string;
}

const state = reactive<OrchestratorState>({
  agents: [],
  edges: [],
  topologyPositions: {},
  messages: [],
  templates: [],
  roles: [],
  ragSources: [],
  workspace: "",
  selectedAgentId: null,
  toasts: [],
  ready: false,
  initError: null,
  icons: {},
  typing: {},
  interruptPending: {},
  toolRuns: [],
  lastTool: {},
  planWaves: [],
  sessions: [],
  currentSessionId: "",
});

/** ツール実行の連番。イベントに ID が無いので受け手側で振る。 */
let toolRunSeq = 0;

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
 * IPC の失敗を表す番兵。
 *
 * **`null` を失敗の印にしてはいけない。** Rust の `CoreResult<()>` は JSON で
 * `null` にシリアライズされるため、戻り値が void のコマンド
 * （`write_ordinance` / `write_agent_config` / `set_agent_icon` …）は
 * **成功しても `null` を返す**。かつて `null` を失敗と読んでいたせいで、
 * 保存が成功しているのに「未保存」表示が残り、アイコンを差し替えても
 * 再起動まで反映されなかった（failures.md #22）。
 *
 * 番兵を専用の値にすれば、コマンドの戻り値が何であっても衝突しない。
 */
const FAILED = Symbol("ipc-failed");

/** 失敗しなかったか。呼び出し側はこれで成功を判定する。 */
function succeeded<T>(result: T | typeof FAILED): result is T {
  return result !== FAILED;
}

/**
 * 参照系の IPC を包み、失敗を通知へ変換する。失敗なら [`FAILED`]。
 *
 * 状態を変えない呼び出し専用。変更系は [`mutate`] を使うこと。
 */
async function guard<T>(
  labelKey: string,
  task: () => Promise<T>,
): Promise<T | typeof FAILED> {
  try {
    return await task();
  } catch (error) {
    const payload = error as ErrorPayload;
    pushToast(
      "error",
      i18n.global.t("orchestrator.opFailed", { op: i18n.global.t(labelKey) }),
      formatError(payload),
    );
    return FAILED;
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
async function mutate<T>(
  labelKey: string,
  task: () => Promise<T>,
): Promise<T | typeof FAILED> {
  try {
    return await task();
  } catch (error) {
    const payload = error as ErrorPayload;
    pushToast(
      "error",
      i18n.global.t("orchestrator.opFailed", { op: i18n.global.t(labelKey) }),
      formatError(payload),
    );
    return FAILED;
  } finally {
    // 再同期自体の失敗で、元の操作の通知を上書きしない。ただし黙殺もしない —
    // 取り直しが落ちると投影が古いまま残るので、痕跡が無いと調査不能になる。
    await refreshAll().catch((error) =>
      console.warn("[useOrchestrator] 再同期に失敗しました（表示が古い可能性）:", error),
    );
  }
}

/** 進行中の取り直し。単一飛行 — 同時に走る取り直しは常に 1 本だけ。 */
let inflightRefresh: Promise<void> | null = null;
/** 進行中の 1 本の完了後に走る追走。何本重なっても追加の 1 周に相乗りする。 */
let trailingRefresh: Promise<void> | null = null;

/**
 * 一覧をコア側から取り直す。
 *
 * refreshAll は「mutate の完了時」と「コアイベント受信時」の 2 系統から呼ばれ、
 * 並行しうる。IPC の完了順は開始順と一致しないので、素朴に並走させると
 * 古い応答が後から着地して新しい状態を上書きする（failures.md #19）。
 *
 * **単一飛行 + 追走 1 周**で直列化する:
 * - 進行中が無ければ、その場で取り直す
 * - 進行中が有れば、その完了後にもう 1 周走る追走へ相乗りする。
 *   進行中の 1 本は自分の変更より**古い**データを持ちうるので、
 *   それを待つだけでは足りない — 完了後の再取得まで待って初めて
 *   「await から戻った時点で自分の変更が見えている」を保証できる。
 *   保存直後に state から下書きを作り直す画面（設定ダイアログ等）はこの保証に
 *   依存する。世代ガード（古い応答を捨てる方式）はこの保証が無く、
 *   「保存したのに未保存表示に戻る」を生んだ。
 */
function refreshAll(): Promise<void> {
  if (inflightRefresh) {
    trailingRefresh ??= inflightRefresh
      .catch(() => undefined)
      .then(() => {
        trailingRefresh = null;
        return refreshAll();
      });
    return trailingRefresh;
  }
  inflightRefresh = fetchAndAssign().finally(() => {
    inflightRefresh = null;
  });
  return inflightRefresh;
}

/** 取り直しの実体。呼び出しは [`refreshAll`] 経由に限る（直列化の内側）。 */
async function fetchAndAssign(): Promise<void> {
  const [agents, edges, topologyPositions, templates, roles, ragSources] = await Promise.all([
    ipc.listAgents(),
    ipc.listTopology(),
    ipc.listTopologyPositions(),
    ipc.listModelTemplates(),
    ipc.listRoles(),
    ipc.listRagSources(),
  ]);
  state.agents = agents;
  // 会話の送信先と左右ペインの強調表示を必ず同じ選択状態にする。
  if (!agents.some((agent) => agent.id === state.selectedAgentId)) {
    state.selectedAgentId = agents[0]?.id ?? null;
  }
  state.edges = edges;
  state.topologyPositions = topologyPositions;
  state.templates = templates;
  state.roles = roles;
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
/**
 * 波の記録を planId で upsert する（Spec 08 の projection_rule）。
 *
 * **順序（リスナー登録 → list → upsert）が欠落側の本体で、upsert は重複側の対策。**
 * list の応答と event が同じ波を運んできたとき、どちらが新しいかは保証されない —
 * だが記録の遷移は片方向（running → 解決、null → 値）なので、
 * 「進んでいる方を採る」だけで正しく合流できる。
 */
function upsertPlanWave(incoming: PlanWaveRecord): void {
  const index = state.planWaves.findIndex((w) => w.planId === incoming.planId);
  if (index < 0) {
    state.planWaves.push(incoming);
    // コアのリングと同じ上限・同じ規則（状態を問わず古い方から）。
    if (state.planWaves.length > PLAN_WAVE_LIMIT) {
      state.planWaves.splice(0, state.planWaves.length - PLAN_WAVE_LIMIT);
    }
    return;
  }

  const existing = state.planWaves[index];
  state.planWaves[index] = {
    ...incoming,
    tasks: incoming.tasks.map((task) => {
      const prev = existing.tasks.find((t) => t.to === task.to);
      // 解決済みを running で巻き戻さない。
      return prev && task.state === "running" && prev.state !== "running" ? prev : task;
    }),
    bundleChars: incoming.bundleChars ?? existing.bundleChars,
    elapsedMs: incoming.elapsedMs ?? existing.elapsedMs,
  };
}

function applyEvent(event: CoreEvent): void {
  switch (event.type) {
    case "agentStatusChanged":
      patchAgent(event.agentId, { status: event.status });
      // 停止・失敗したエージェントの「入力中…」を残さない保険。
      // 通常は agentTyping の終了イベントが対で消すが、二重の網にしておく。
      if (event.status !== "running" && event.status !== "starting") {
        delete state.typing[event.agentId];
      }
      break;

    case "agentStatsUpdated":
      patchAgent(event.agentId, {
        uptimeSecs: event.uptimeSecs,
        totalTokens: event.totalTokens,
        promptTokens: event.promptTokens,
        cachedTokens: event.cachedTokens,
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

    case "agentTyping":
      if (event.active) {
        state.typing[event.agentId] = true;
      } else {
        delete state.typing[event.agentId];
        // ターンは何らかの形で終わった。打ち切り要求が検知される前に完走で
        // 逃げ切った場合、ここで消さないと「停止要求中…」が永遠に残る。
        delete state.interruptPending[event.agentId];
      }
      break;

    case "turnInterrupted":
      // 出口の行（System の 1 行）は会話ログ側に別途届く（messageSent）。
      // ここでの仕事は「要求中」表示の解除だけ — トーストは出さない。
      // 利用者が自分で押した操作の完了をトーストで重ねると、波の連鎖では
      // 1 回の操作に対して人数分の通知が積まれる。
      delete state.interruptPending[event.agentId];
      break;

    case "toolInvoked": {
      const run: ToolRun = {
        id: `tool-${++toolRunSeq}`,
        agentId: event.agentId,
        tool: event.tool,
        ok: event.ok,
        tsMs: Date.now(),
      };
      state.toolRuns.push(run);
      if (state.toolRuns.length > MESSAGE_LIMIT) {
        state.toolRuns.splice(0, state.toolRuns.length - MESSAGE_LIMIT);
      }
      state.lastTool[event.agentId] = run;
      break;
    }

    case "toolLimitReached": {
      // 打ち切りは会話ログにも本文として現れるが、そちらはモデルの言葉で、
      // これはこちらが打ち切った事実。設定で直せる種類の詰まりなので、
      // どの上限に当たったかを数字で出す。
      const name =
        state.agents.find((a) => a.id === event.agentId)?.name ?? event.agentId;
      pushToast(
        "warn",
        i18n.global.t("orchestrator.toolLimit", { name }),
        i18n.global.t("orchestrator.toolLimitDetail", { max: event.maxIterations }),
      );
      break;
    }

    case "toolRepeatBlocked": {
      // 上限到達と混ぜない。当たったのは上限ではないので、上限を上げても
      // 直らない — 直し方が違う打ち切りは、別の文言で出す。
      const name =
        state.agents.find((a) => a.id === event.agentId)?.name ?? event.agentId;
      pushToast(
        "warn",
        i18n.global.t("orchestrator.repeatBlocked", { name }),
        i18n.global.t("orchestrator.repeatBlockedDetail", {
          tool: event.tool,
          repeats: event.repeats,
        }),
      );
      break;
    }

    case "conversationCleared":
      // messages は Shared.log の投影。コア側が先にクリアし、この通知で
      // 投影を追従させる（順序は reset_rule で固定）。
      state.messages = [];
      // ツール実行は会話に紐づく事実なので、会話と一緒に消す。
      state.toolRuns = [];
      state.lastTool = {};
      // **ここでは通知を出さない**（Spec 12）。この 1 本は新規チャットでも
      // 開き直しでも分岐でも同じように飛ぶので、「新規チャットを開始しました」と
      // 言うと開き直しのときに嘘になる。何が起きたかは操作した側が知っている。
      break;

    case "sessionSwitched":
      // 会話が切り替わった（Spec 12）。conversationCleared が空にした直後に
      // 届くので、**ここでコア側の復元結果を引き直す** — 開き直しと分岐では
      // コアが会話ログを戻しており、空のままにすると画面だけが白紙に見える。
      state.currentSessionId = event.sessionId;
      void guard("orchestrator.op.loadSession", async () => {
        state.messages = await ipc.listMessages(MESSAGE_LIMIT);
        state.sessions = await ipc.listSessions();
      });
      break;

    case "agentFailed": {
      patchAgent(event.agentId, { lastError: event.error });
      const name =
        state.agents.find((a) => a.id === event.agentId)?.name ?? event.agentId;
      pushToast(
        "error",
        i18n.global.t("orchestrator.agentFailed", { name }),
        formatError(event.error),
      );
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
          i18n.global.t("orchestrator.degraded", { name }),
          i18n.global.t("orchestrator.degradedDetail", { reason: event.reason }),
        );
      }
      break;
    }

    case "hopLimitReached": {
      const name =
        state.agents.find((a) => a.id === event.agentId)?.name ?? event.agentId;
      pushToast(
        "warn",
        i18n.global.t("orchestrator.hopLimit"),
        i18n.global.t("orchestrator.hopLimitDetail", { name, max: event.maxHops }),
      );
      break;
    }

    case "planWaveStarted":
      upsertPlanWave({
        planId: event.planId,
        agentId: event.agentId,
        wave: event.wave,
        startedAtMs: event.startedAtMs,
        tasks: event.tasks.map((task) => ({
          to: task.to,
          state: "running",
          elapsedMs: null,
          msgChars: task.msgChars,
        })),
        bundleChars: null,
        elapsedMs: null,
      });
      break;

    case "planTaskResolved": {
      // Started より前に Resolved は来ない（per planId の順序保証）。
      // 波が無いのはリングから押し出された後だけで、その更新は捨ててよい。
      const wave = state.planWaves.find((w) => w.planId === event.planId);
      const task = wave?.tasks.find((t) => t.to === event.to);
      if (task) {
        task.state = event.state;
        task.elapsedMs = event.elapsedMs;
      }
      break;
    }

    case "planWaveFinished": {
      const wave = state.planWaves.find((w) => w.planId === event.planId);
      if (wave) {
        wave.bundleChars = event.bundleChars;
        wave.elapsedMs = event.elapsedMs;
        // 完了した波に「実行中」を残さない（コアの finish_wave と同じ後始末）。
        for (const task of wave.tasks) {
          if (task.state === "running") task.state = "no_answer";
        }
      }
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
    // バックエンドの初期化（MCP 接続を含む）が終わるまで待つ。
    // ウィンドウは即時に出る設計なので、この間は「初期起動中…」の覆いが見える。
    // 完了前に他のコマンドを呼ぶと状態未登録で失敗するため、ここが関門になる。
    for (;;) {
      const boot = await ipc.bootStatus();
      if (boot.error) {
        throw {
          code: "BOOT_FAILED",
          message: i18n.global.t("orchestrator.bootFailed"),
          detail: boot.error,
          agentId: null,
          retryable: false,
        } satisfies ErrorPayload;
      }
      if (boot.ready) break;
      await new Promise((resolve) => setTimeout(resolve, 250));
    }

    // 購読を先に張る。読み込み中に発生したイベントを取りこぼさない。
    unlisten?.();
    unlisten = await listen<CoreEvent>(CORE_EVENT, (e) => applyEvent(e.payload));

    await refreshAll();
    state.messages = await ipc.listMessages(MESSAGE_LIMIT);
    // 波の再投影（Spec 08）。リスナーは上で登録済みなので、この取得中に
    // 飛んだ event は取りこぼさない。同じ波が両経路から来たら upsert が合流する。
    for (const wave of await ipc.listPlanWaves()) {
      upsertPlanWave(wave);
    }
    state.workspace = await ipc.workspacePath();
    // 開いている会話（Spec 12）。一覧はダイアログを開くまで引かない。
    state.currentSessionId = await ipc.currentSession();
    // 表示言語（Spec 13）。world.json で確定済みの値を映す — 覆いが外れる前に
    // 当てることで、確定言語と違う文言が一瞬見える継ぎ目を作らない。
    setLocale(await ipc.getLanguage());
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

    /**
     * 画面右上の通知を出す。IPC を通らない UI 層の操作
     * （プラグイン呼び出しなど）が失敗したときの表面化に使う。
     * ハンドラ内の例外を素通しにすると Vue の ErrorBoundary まで昇り、
     * 無関係な区画が「表示に失敗しました」ごと落ちる。
     */
    notify(level: Toast["level"], title: string, detail?: string): void {
      pushToast(level, title, detail);
    },
    async refreshAll(): Promise<void> {
      await guard("orchestrator.op.refresh", refreshAll);
    },

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
      await mutate(running ? "orchestrator.op.start" : "orchestrator.op.stop", () =>
        ipc.setAgentRunning(agentId, running),
      );
    },

    /**
     * 飛行中のターンを打ち切る（Spec 10 — 会話ペインの停止ボタン）。
     *
     * 検知は周回境界（飛行中の LLM 呼び出しは完走する）なので、押した瞬間には
     * 止まらない。その間の「押せている」表示は `interruptPending` が担い、
     * `turnInterrupted` か typing の終了で解除される。
     */
    async interruptTurn(agentId: AgentId): Promise<void> {
      state.interruptPending[agentId] = true;
      await mutate("orchestrator.op.interruptTurn", () => ipc.interruptTurn(agentId));
    },

    /**
     * 村の飛行中ターンを全部打ち切る（Spec 10 — 全体停止）。
     *
     * 要求中の表示は今まさに飛行中の面々にだけ立てる。飛行していない
     * エージェントは切られないので、立てると解除する契機が来ない。
     */
    async interruptAll(): Promise<void> {
      for (const agentId of Object.keys(state.typing)) {
        state.interruptPending[agentId] = true;
      }
      await mutate("orchestrator.op.interruptAll", () => ipc.interruptAll());
    },

    /**
     * 一括起動 / 一括停止（左ペインヘッダの ▶ / ■）。
     *
     * 対象の決定は `lib/batchStart.ts` の純関数が持ち、ここは配送だけを担う。
     *
     * **1 体ずつ順に叩く。** 並列にすると N 体ぶんの `refreshAll` が同時に
     * 走って世代ガードの取り直し合戦になり、しかも失敗が同時に N 個の
     * トーストとして出る。起動要求そのものは即座に返る（実際の立ち上げは
     * コア側で非同期に進む）ので、直列でも待ち時間は増えない。
     */
    async runBatch(agentIds: readonly AgentId[], running: boolean): Promise<void> {
      for (const agentId of agentIds) {
        patchAgent(agentId, { status: running ? "starting" : "stopping" });
      }
      for (const agentId of agentIds) {
        await mutate(
          running ? "orchestrator.op.batchStart" : "orchestrator.op.batchStop",
          () => ipc.setAgentRunning(agentId, running),
        );
      }
    },

    /**
     * 一括起動の対象に含めるかを切り替える（永続。`world.json` へ入る）。
     *
     * **稼働状態は変えない。** この 2 つを混ぜないことが本機構の要点で、
     * 「今は止まっているが、次の一括起動では起こしたい」を表現できるように
     * するための分離である。
     *
     * 保存は `updateAgent`（spec 全体の差し替え）を通す。専用の IPC を
     * 足さないのは、1 フィールドごとにコマンドを増やすと同期させる箇所が
     * 増えるだけだから。
     */
    async setBatchStart(agentId: AgentId, batchStart: boolean): Promise<void> {
      const current = state.agents.find((a) => a.id === agentId);
      if (!current) return;
      // 楽観的に反映する（チェックの手応えを IPC 往復に待たせない）。
      patchAgent(agentId, { batchStart });
      await mutate("orchestrator.op.saveBatchStart", () =>
        ipc.updateAgent({
          id: current.id,
          name: current.name,
          modelTemplateId: current.modelTemplateId,
          ragSources: [...current.ragSources],
          connectedAgents: [...current.connectedAgents],
          order: current.order,
          workDir: current.workDir,
          maxToolIterations: current.maxToolIterations,
          enabledTools: current.enabledTools ? [...current.enabledTools] : null,
          hearsRoomLog: current.hearsRoomLog,
          batchStart,
          // 投影から spec を組み直す経路。**写さないと保存のたびに役職が外れる**。
          roleId: current.roleId,
        }),
      );
    },

    async createAgent(spec: AgentSpec): Promise<AgentSnapshot | null> {
      const created = await mutate("orchestrator.op.createAgent", () =>
        ipc.createAgent(spec),
      );
      return succeeded(created) ? created : null;
    },

    async updateAgent(spec: AgentSpec): Promise<void> {
      await mutate("orchestrator.op.saveAgent", () => ipc.updateAgent(spec));
    },

    async deleteAgent(agentId: AgentId): Promise<void> {
      const done = await mutate("orchestrator.op.deleteAgent", () =>
        ipc.deleteAgent(agentId),
      );
      if (succeeded(done) && state.selectedAgentId === agentId) {
        state.selectedAgentId = null;
      }
    },

    async setConnections(agentId: AgentId, targets: AgentId[]): Promise<void> {
      await mutate("orchestrator.op.setConnections", () =>
        ipc.setConnections(agentId, targets),
      );
    },

    async setTopologyPosition(
      agentId: AgentId,
      position: TopologyPosition,
    ): Promise<void> {
      await mutate("orchestrator.op.savePosition", () =>
        ipc.setTopologyPosition(agentId, position),
      );
    },

    async reorder(order: AgentId[]): Promise<void> {
      // 並び替えは即座に見た目へ反映しないと操作感が壊れるので、先に order を振る。
      order.forEach((id, index) => patchAgent(id, { order: index }));
      state.agents.sort((a, b) => a.order - b.order);
      await mutate("orchestrator.op.reorder", () => ipc.reorderAgents(order));
    },

    /**
     * 成否を返す。呼び出し側（ダイアログ）は成功時だけ「保存しました」を
     * 出す — 失敗時はエラートーストが出るので、そこへ成功表示を重ねると
     * 画面が矛盾する。
     */
    async upsertTemplate(template: ModelTemplate): Promise<boolean> {
      const done = await mutate("orchestrator.op.saveTemplate", () =>
        ipc.upsertModelTemplate(template),
      );
      return done !== FAILED;
    },

    /**
     * API キーを OS の資格情報ストアへ預ける。
     *
     * 秘密は引数として通り抜けるだけで、ストアには入れない。
     * ここへ控えを持つと、値を保持しない設計が UI 層で崩れる。
     */
    async setCredential(templateId: string, secret: string): Promise<boolean> {
      const done = await mutate("orchestrator.op.setCredential", () =>
        ipc.setModelCredential(templateId, secret),
      );
      if (succeeded(done)) {
        pushToast("info", i18n.global.t("orchestrator.credentialSaved"));
        // 退避の通知は「もう解決したかもしれない」ので、次の失敗で出し直す。
        reportedDegradations.clear();
      }
      return succeeded(done);
    },

    /** API キーを資格情報ストアから削除する。 */
    async clearCredential(templateId: string): Promise<boolean> {
      const done = await mutate("orchestrator.op.clearCredential", () =>
        ipc.clearModelCredential(templateId),
      );
      if (succeeded(done)) reportedDegradations.clear();
      return succeeded(done);
    },

    async deleteTemplate(templateId: string): Promise<void> {
      await mutate("orchestrator.op.deleteTemplate", () =>
        ipc.deleteModelTemplate(templateId),
      );
    },

    /** MCP の宣言を保存し、その場で接続し直す。 */
    async saveMcpConfig(config: McpConfig): Promise<boolean> {
      const done = await mutate("orchestrator.op.saveMcp", () =>
        ipc.writeMcpConfig(config),
      );
      if (succeeded(done)) {
        pushToast(
          "info",
          i18n.global.t("orchestrator.mcpReconnected"),
          i18n.global.t("orchestrator.mcpReconnectedDetail"),
        );
      }
      return succeeded(done);
    },

    /** 設定を変えずに MCP へ繋ぎ直す。 */
    async reloadMcp(): Promise<boolean> {
      const done = await mutate("orchestrator.op.reloadMcp", () => ipc.reloadMcp());
      return succeeded(done);
    },

    /** 村の条例を保存する。次の発話から全エージェントに反映される。 */
    async saveOrdinance(content: string): Promise<boolean> {
      const done = await mutate("orchestrator.op.saveOrdinance", () =>
        ipc.writeOrdinance(content),
      );
      if (succeeded(done)) {
        pushToast(
          "info",
          i18n.global.t("orchestrator.ordinanceSaved"),
          i18n.global.t("orchestrator.ordinanceSavedDetail"),
        );
      }
      return succeeded(done);
    },

    /**
     * アイコンを保存する。`bytes` は UI 側で WebP へ変換済みであること。
     * 成功したらキャッシュを手元のバイト列で直接更新する（再フェッチしない）。
     */
    async setAgentIcon(agentId: AgentId, bytes: Uint8Array): Promise<boolean> {
      const done = await mutate("orchestrator.op.saveIcon", () =>
        ipc.setAgentIcon(agentId, Array.from(bytes)),
      );
      if (succeeded(done)) {
        const old = state.icons[agentId];
        if (old) URL.revokeObjectURL(old);
        state.icons[agentId] = iconUrlOf(Array.from(bytes));
      }
      return succeeded(done);
    },

    /** アイコンを削除する。 */
    async clearAgentIcon(agentId: AgentId): Promise<boolean> {
      const done = await mutate("orchestrator.op.clearIcon", () =>
        ipc.clearAgentIcon(agentId),
      );
      if (succeeded(done)) {
        const old = state.icons[agentId];
        if (old) URL.revokeObjectURL(old);
        state.icons[agentId] = null;
      }
      return succeeded(done);
    },

    async readConfig(agentId: AgentId, kind: ConfigFileKind): Promise<string | null> {
      const text = await guard("orchestrator.op.readConfig", () =>
        ipc.readAgentConfig(agentId, kind),
      );
      return succeeded(text) ? text : null;
    },

    async writeConfig(
      agentId: AgentId,
      kind: ConfigFileKind,
      content: string,
    ): Promise<boolean> {
      const done = await mutate("orchestrator.op.saveConfig", () =>
        ipc.writeAgentConfig(agentId, kind, content),
      );
      if (succeeded(done)) pushToast("info", i18n.global.t("orchestrator.saved"));
      return succeeded(done);
    },

    async send(agentId: AgentId, content: string): Promise<void> {
      // 発話は MessageSent イベントで届くので、ここでの再同期は
      // 送信が拒否された場合に一覧を正しく戻すために効く。
      await mutate("orchestrator.op.send", () => ipc.sendUserMessage(agentId, content));
    },

    /**
     * 新しい会話を開く（新規チャット）。表示は conversationCleared が消し、
     * sessionSwitched が新しい会話を載せる。
     *
     * **前の会話は捨てられずディスクに残る**（Spec 12）。
     */
    async newChat(): Promise<void> {
      const done = await guard("orchestrator.op.newChat", () =>
        ipc.resetConversation(),
      );
      if (succeeded(done)) {
        pushToast(
          "info",
          i18n.global.t("orchestrator.newChatOpened"),
          i18n.global.t("orchestrator.newChatOpenedDetail"),
        );
      }
    },

    // ---- 会話（セッション。Spec 12） ---------------------------------------

    /** 会話一覧を取り直す。一覧ダイアログを開いたときに呼ぶ。 */
    async refreshSessions(): Promise<void> {
      const listed = await guard("orchestrator.op.listSessions", () =>
        ipc.listSessions(),
      );
      if (succeeded(listed)) state.sessions = listed;
    },

    /**
     * 保存されている会話を開き直す。
     *
     * 飛行中のターンがあるとコアが `SESSION_SWITCH_BLOCKED` で拒む。
     * その通知は guard がそのまま出す — 直し方（停止してから切り替える）は
     * エラー文言側に書いてある。
     */
    async resumeSession(sessionId: string): Promise<boolean> {
      const done = await guard("orchestrator.op.resumeSession", () =>
        ipc.resumeSession(sessionId),
      );
      return succeeded(done);
    },

    /** 分岐できる地点（その会話のユーザー発話）。 */
    async forkPoints(sessionId: string): Promise<ForkPoint[]> {
      const points = await guard("orchestrator.op.forkPoints", () =>
        ipc.listForkPoints(sessionId),
      );
      return succeeded(points) ? points : [];
    },

    /** `atSeq` まで含めて複製し、複製した側を開く。元は不変のまま残る。 */
    async forkSession(sessionId: string, atSeq: number): Promise<boolean> {
      const done = await guard("orchestrator.op.forkSession", () =>
        ipc.forkSession(sessionId, atSeq),
      );
      if (succeeded(done)) {
        pushToast(
          "info",
          i18n.global.t("orchestrator.sessionForked"),
          i18n.global.t("orchestrator.sessionForkedDetail"),
        );
      }
      return succeeded(done);
    },

    /** 会話を消す。開いている会話を消した場合は次の会話へ切り替わる。 */
    async deleteSession(sessionId: string): Promise<boolean> {
      const done = await guard("orchestrator.op.deleteSession", () =>
        ipc.deleteSession(sessionId),
      );
      if (succeeded(done)) {
        const listed = await guard("orchestrator.op.listSessions", () =>
          ipc.listSessions(),
        );
        if (succeeded(listed)) state.sessions = listed;
        pushToast("info", i18n.global.t("orchestrator.sessionDeleted"));
      }
      return succeeded(done);
    },

    /**
     * いまの会話を要約して続ける（Spec 12 P4）。
     *
     * サーヴァント 1 体につき 1 回 LLM を呼ぶので、**押した時点でトークンを使う**。
     * 呼び出し中は数十秒かかりうるため、開始した事実も通知に出す。
     */
    async summarize(): Promise<void> {
      pushToast(
        "info",
        i18n.global.t("orchestrator.summarizing"),
        i18n.global.t("orchestrator.summarizingDetail"),
      );
      const count = await guard("orchestrator.op.summarize", () =>
        ipc.summarizeSession(),
      );
      if (!succeeded(count)) return;
      if (count === 0) {
        pushToast(
          "info",
          i18n.global.t("orchestrator.summarizeEmpty"),
          i18n.global.t("orchestrator.summarizeEmptyDetail"),
        );
        return;
      }
      pushToast(
        "info",
        i18n.global.t("orchestrator.summarized", { count }),
        i18n.global.t("orchestrator.summarizedDetail"),
      );
    },

    /** 会話を JSONL で書き出す。書き出し先のパスを通知に出す。 */
    async exportSession(sessionId: string): Promise<void> {
      const path = await guard("orchestrator.op.exportSession", () =>
        ipc.exportSession(sessionId),
      );
      if (succeeded(path)) {
        pushToast("info", i18n.global.t("orchestrator.sessionExported"), path);
      }
    },

    // ユーザーからの同報（複数宛先へ一斉送信）は UI から外した。全員のターンが
    // 並列に走り、誰も他の答えを見ないまま応答するため混乱する。まとめて動かす
    // 経路は「進行役 1 体に頼む」（orchestrator-workers）に一本化した。
    // **コア側の同報機構は残っている** — エージェント発の fan-out が使っている。

    dismissToast,
  };
}
