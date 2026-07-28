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
  AgentMessage,
  AgentSnapshot,
  AgentSpec,
  ConfigFileKind,
  ErrorPayload,
  ModelTemplate,
  ModelTemplateId,
  RagChunk,
  TopologyEdge,
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

/** `invoke` の薄いラッパ。失敗は必ず {@link ErrorPayload} として throw する。 */
async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    throw toErrorPayload(error);
  }
}

// ---- 参照系 -----------------------------------------------------------------

/** 登録済みエージェントを表示順で取得する。 */
export const listAgents = () => call<AgentSnapshot[]>("list_agents");

/** トポロジーの全辺を取得する。 */
export const listTopology = () => call<TopologyEdge[]>("list_topology");

/** メッセージログを取得する。`limit` 指定時は末尾からその件数。 */
export const listMessages = (limit?: number) =>
  call<AgentMessage[]>("list_messages", { limit });

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

// ---- ライフサイクルと配送 ---------------------------------------------------

/** トグル操作で稼働状態を切り替える。既に望む状態なら何も起きない。 */
export const setAgentRunning = (agentId: AgentId, running: boolean) =>
  call<AgentSnapshot>("set_agent_running", { agentId, running });

/** ユーザー発話をエージェントへ投入する。 */
export const sendUserMessage = (agentId: AgentId, content: string) =>
  call<void>("send_user_message", { agentId, content });

/** RAG 索引に断片を追加する。 */
export const indexRagChunk = (chunk: RagChunk) =>
  call<void>("index_rag_chunk", { chunk });
