/**
 * Tauri IPC の型付きラッパ。
 *
 * `invoke` を直接呼ぶ代わりにここを通すことで、
 * - コマンド名の打ち間違いをコンパイル時に検出できる
 * - エラーが必ず {@link ErrorPayload} 形で返ることを型で保証できる
 *
 * Rust 側は引数名を snake_case で宣言しているが、Tauri v2 が JS の camelCase を
 * 自動変換するため、呼び出し側はキャメルケースで書く。
 */

import { invoke } from "@tauri-apps/api/core";
import type {
  AgentId,
  AgentMcpStatus,
  AgentMessage,
  AgentSnapshot,
  AgentSpec,
  BlackboardNote,
  ConfigFileKind,
  ErrorPayload,
  ForkPoint,
  ModelTemplate,
  ModelTemplateId,
  McpConfig,
  McpServerStatus,
  PlanWaveRecord,
  RagChunk,
  Recurrence,
  ScheduleView,
  SessionSummary,
  TopologyEdge,
  TopologyPosition,
} from "../types";

/**
 * Rust 側から返る失敗を、常に {@link ErrorPayload} の形へ正規化する。
 *
 * Tauri のブリッジ自体が落ちた場合（コマンド名の誤りなど）は `ErrorPayload` に
 * ならないため、ここで包み直す。UI 側に「形が 2 通りある」状態を持ち込まない。
 */
export function toErrorPayload(error: unknown): ErrorPayload {
  if (
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    "message" in error
  ) {
    return error as ErrorPayload;
  }
  return {
    code: "IPC_FAILED",
    message: typeof error === "string" ? error : "IPC 呼び出しに失敗しました",
    detail: error instanceof Error ? error.message : null,
    agentId: null,
    retryable: false,
  };
}

/**
 * 引数を素の JSON 値へ落とす。
 *
 * Tauri v2 の IPC は structured clone を通るため、**Vue の reactive Proxy を
 * そのまま渡すと `DataCloneError` で弾かれる**。呼び出し側で `toRaw` を
 * 掛けて回る方式は、新しい画面を足すたびに同じ穴が空くので採らない。
 * 境界であるここで 1 回だけ正規化し、上位はリアクティブな値を素直に渡せるようにする。
 *
 * `undefined` のキーは JSON 化の過程で落ちる。Rust 側は該当引数を `Option` で
 * 受けており、キーの不在は `None` として解釈されるため、これで整合する。
 */
function toPlain(args?: Record<string, unknown>): Record<string, unknown> | undefined {
  if (args === undefined) return undefined;
  return JSON.parse(JSON.stringify(args)) as Record<string, unknown>;
}

/** `invoke` の薄いラッパ。失敗は必ず {@link ErrorPayload} として throw する。 */
async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, toPlain(args));
  } catch (error) {
    throw toErrorPayload(error);
  }
}

// ---- 起動ハンドシェイク -------------------------------------------------------

/** 起動の進み具合。`commands.rs` の `BootStatus` と一致させること。 */
export interface BootStatus {
  ready: boolean;
  error: string | null;
}

/**
 * 初期化が終わったかを問い合わせる。
 *
 * バックエンドの初期化（MCP 接続を含む）はバックグラウンドで走っており、
 * 完了までは他のコマンドを呼んではいけない（状態が未登録で失敗する）。
 * これだけは初期化前でも常に答えが返る。
 */
export const bootStatus = () => call<BootStatus>("boot_status");

// ---- 参照系 -----------------------------------------------------------------

/** 登録済みエージェントを表示順で取得する。 */
export const listAgents = () => call<AgentSnapshot[]>("list_agents");

/** トポロジーの全辺を取得する。 */
export const listTopology = () => call<TopologyEdge[]>("list_topology");

/** 村の地図の保存済みノード座標を取得する。 */
export const listTopologyPositions = () =>
  call<Record<AgentId, TopologyPosition>>("list_topology_positions");

/** メッセージログを取得する。`limit` 指定時は末尾からその件数。 */
export const listMessages = (limit?: number) =>
  call<AgentMessage[]>("list_messages", { limit });

/** plan 波の記録を取得する（Spec 08 — 波ペイン。古い順・実行中の波も含む）。 */
export const listPlanWaves = () => call<PlanWaveRecord[]>("list_plan_waves");

/** エージェント別トークン消費量を取得する（Rust 側で Rayon 集計）。 */
export const tokenUsage = () => call<Record<AgentId, number>>("token_usage");

/** モデルテンプレート一覧を取得する。 */
export const listModelTemplates = () => call<ModelTemplate[]>("list_model_templates");

/** RAG ソース名一覧を取得する。 */
export const listRagSources = () => call<string[]>("list_rag_sources");

/** RAG を検索する。 */
export const searchRag = (sources: string[], query: string, topK: number) =>
  call<RagChunk[]>("search_rag", { sources, query, topK });

/** ワークスペースの実パスを取得する。 */
export const workspacePath = () => call<string>("workspace_path");

/**
 * API キーを OS の資格情報ストアへ登録する。
 *
 * 秘密がフロントを通るのはこの 1 本だけで、方向は片道。
 * 読み出す API は存在しない。
 */
export const setModelCredential = (templateId: ModelTemplateId, secret: string) =>
  call<void>("set_model_credential", { templateId, secret });

/** API キーを資格情報ストアから削除する。 */
export const clearModelCredential = (templateId: ModelTemplateId) =>
  call<void>("clear_model_credential", { templateId });

/** API キーが登録済みかだけを問い合わせる。値は返らない。 */
export const modelCredentialExists = (templateId: ModelTemplateId) =>
  call<boolean>("model_credential_exists", { templateId });

// ---- 定義の編集 -------------------------------------------------------------

/** エージェントを登録する。 */
export const createAgent = (spec: AgentSpec) =>
  call<AgentSnapshot>("create_agent", { spec });

/** エージェント定義を差し替える。 */
export const updateAgent = (spec: AgentSpec) =>
  call<AgentSnapshot>("update_agent", { spec });

/** エージェントを削除する。稼働中なら停止してから消される。 */
export const deleteAgent = (agentId: AgentId) =>
  call<void>("delete_agent", { agentId });

/** 接続先を差し替える。 */
export const setConnections = (agentId: AgentId, targets: AgentId[]) =>
  call<void>("set_connections", { agentId, targets });

/** 左ペインの並び順を確定する。 */
export const reorderAgents = (order: AgentId[]) =>
  call<void>("reorder_agents", { order });

/** 村の地図上で移動したノードの座標を保存する。 */
export const setTopologyPosition = (agentId: AgentId, position: TopologyPosition) =>
  call<void>("set_topology_position", { agentId, position });

/** モデルテンプレートを登録または更新する。 */
export const upsertModelTemplate = (template: ModelTemplate) =>
  call<void>("upsert_model_template", { template });

/** モデルテンプレートを削除する。参照中のエージェントが居れば拒否される。 */
export const deleteModelTemplate = (templateId: ModelTemplateId) =>
  call<void>("delete_model_template", { templateId });

// ---- 設定ファイル -----------------------------------------------------------

/** 設定ファイルを読む。未作成なら空文字が返る。 */
export const readAgentConfig = (agentId: AgentId, kind: ConfigFileKind) =>
  call<string>("read_agent_config", { agentId, kind });

/** 設定ファイルを書く。 */
export const writeAgentConfig = (
  agentId: AgentId,
  kind: ConfigFileKind,
  content: string,
) => call<void>("write_agent_config", { agentId, kind, content });

// ---- 村の条例 ----------------------------------------------------------------

/** 村の条例（全エージェント共通の規則）を読む。未設定なら空文字。 */
export const readOrdinance = () => call<string>("read_ordinance");

/** 村の条例を書く。次の発話からすべてのエージェントに反映される。 */
export const writeOrdinance = (content: string) =>
  call<void>("write_ordinance", { content });

// ---- 村の黒板 ----------------------------------------------------------------

/**
 * 村の黒板（work_dir の `黒板/`）の付箋を読む。
 *
 * 読み取り専用 — 書き込みの IPC は存在しない。書くのはエージェント
 * （file ツール）と人で、条例の「1 人 1 ファイル」運用を GUI が迂回しない。
 */
export const listBlackboard = () => call<BlackboardNote[]>("list_blackboard");

// ---- MCP ---------------------------------------------------------------------

/** `mcp.json` の宣言を読む。未作成なら空の集合。 */
export const readMcpConfig = () => call<McpConfig>("read_mcp_config");

/** `mcp.json` を書き、その場で接続し直す。 */
export const writeMcpConfig = (config: McpConfig) =>
  call<void>("write_mcp_config", { config });

/** 設定を変えずに接続し直す。 */
export const reloadMcp = () => call<void>("reload_mcp");

/** 各 MCP サーバーの接続状態。 */
export const listMcpServers = () => call<McpServerStatus[]>("list_mcp_servers");

/** エージェント別 MCP の状態。停止中は running: false でサーバー一覧は空。 */
export const agentMcpStatus = (agentId: AgentId) =>
  call<AgentMcpStatus>("agent_mcp_status", { agentId });

/**
 * 新しい会話を開く（新規チャット）。稼働状態・統計・Memory.md は残る。
 *
 * **Spec 12 で意味が変わった**: 今の会話は捨てられず、閉じて新しい会話が開く。
 */
export const resetConversation = () => call<void>("reset_conversation");

// ---- 会話（セッション。Spec 12） ---------------------------------------------

/** 保存されている会話の一覧（`updatedAt` の新しい順）。 */
export const listSessions = () => call<SessionSummary[]>("list_sessions");

/** いま開いている会話の ID。保存先が開けていない村では空文字。 */
export const currentSession = () => call<string>("current_session");

/**
 * 保存されている会話を開き直す。
 *
 * 飛行中のターンがあると `SESSION_SWITCH_BLOCKED` で失敗する。
 */
export const resumeSession = (sessionId: string) =>
  call<void>("resume_session", { sessionId });

/** 分岐できる地点（その会話のユーザー発話）を古い順で返す。 */
export const listForkPoints = (sessionId: string) =>
  call<ForkPoint[]>("list_fork_points", { sessionId });

/** `atSeq` **まで含めて**複製し、複製した側を開く。元は不変のまま残る。 */
export const forkSession = (sessionId: string, atSeq: number) =>
  call<string>("fork_session", { sessionId, atSeq });

/** 会話を消す。開いている会話を消した場合は次の会話へ切り替わる。 */
export const deleteSession = (sessionId: string) =>
  call<void>("delete_session", { sessionId });

/** 会話を JSONL で書き出し、**書き出し先のパス**を返す。 */
export const exportSession = (sessionId: string) =>
  call<string>("export_session", { sessionId });

/**
 * いまの会話を要約して続ける。要約できたサーヴァント数を返す。
 *
 * **人が押したときだけ走る**（LLM 呼び出し = トークンなので自動化しない）。
 */
export const summarizeSession = () => call<number>("summarize_session");

// ---- アイコン ----------------------------------------------------------------

/** エージェントのアイコン（WebP バイト列）を取得する。未設定なら `null`。 */
export const getAgentIcon = (agentId: AgentId) =>
  call<number[] | null>("get_agent_icon", { agentId });

/** エージェントのアイコンを保存する。`data` は WebP へ変換済みであること。 */
export const setAgentIcon = (agentId: AgentId, data: number[]) =>
  call<void>("set_agent_icon", { agentId, data });

/** エージェントのアイコンを削除する。 */
export const clearAgentIcon = (agentId: AgentId) =>
  call<void>("clear_agent_icon", { agentId });

// ---- ライフサイクルと配送 ---------------------------------------------------

/** トグル操作で稼働状態を切り替える。既に望む状態なら何も起きない。 */
export const setAgentRunning = (agentId: AgentId, running: boolean) =>
  call<AgentSnapshot>("set_agent_running", { agentId, running });

/**
 * 飛行中のターンを協調的に打ち切る（Spec 10）。
 *
 * 切るのはターンであってエージェントではない — 稼働は降ろさず、会話も
 * 履歴も残る。飛行中のターンが無ければ何も起きない（成功）。検知は周回境界
 * なので、押した瞬間には止まらない — 表示は `interruptPending` が担う。
 */
export const interruptTurn = (agentId: AgentId) =>
  call<void>("interrupt_turn", { agentId });

/** 村の飛行中ターンを全部打ち切る（Spec 10）。冪等 — 飛行中 0 でも成功。 */
export const interruptAll = () => call<void>("interrupt_all");

/**
 * ユーザー発話をエージェントへ投入する。
 *
 * `coRecipients` は同報の全宛先（受信者自身を含む）。同報時だけ渡すと、
 * 受信者のプロンプトに「全員が既に受け取っている」注記が入り、
 * 各エージェントが律儀に転送し合う反響を防ぐ。単独宛では省略する。
 */
export const sendUserMessage = (
  agentId: AgentId,
  content: string,
  coRecipients?: AgentId[],
) => call<void>("send_user_message", { agentId, content, coRecipients });

/** RAG 索引に断片を追加する。 */
export const indexRagChunk = (chunk: RagChunk) =>
  call<void>("index_rag_chunk", { chunk });

// ---- 予定（Spec 07） ----------------------------------------------------------

/** 登録済みの予定（登録順）。次回発火時刻はコア側で算出済み。 */
export const listSchedules = () => call<ScheduleView[]>("list_schedules");

/** 予定を登録する。宛先は停止中でもよいが、未登録なら拒否される。 */
export const createSchedule = (to: AgentId, message: string, recurrence: Recurrence) =>
  call<ScheduleView>("create_schedule", { to, message, recurrence });

/** 予定を削除する。復元はできない。 */
export const deleteSchedule = (id: string) => call<void>("delete_schedule", { id });

/** 予定の一時停止・再開。停止中は発火も消化もされない。 */
export const setScheduleEnabled = (id: string, enabled: boolean) =>
  call<void>("set_schedule_enabled", { id, enabled });
