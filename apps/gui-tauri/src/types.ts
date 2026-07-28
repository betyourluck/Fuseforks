/**
 * Rust 側 `agent-core` のドメイン型に 1 対 1 で対応する TypeScript 定義。
 *
 * **この 2 つは手で同期させる契約になっている。** Rust 側の `crates/agent-core/src/model.rs`
 * および `event.rs` のフィールドを増減させたら、必ずここも直すこと。
 * serde は `rename_all = "camelCase"` を指定しているので、命名はキャメルケースで一致する。
 */

/** エージェント識別子。Rust 側は透過的な newtype なので、ワイヤ上はただの文字列。 */
export type AgentId = string;

/** モデルテンプレート識別子。 */
export type ModelTemplateId = string;

/** エージェントのライフサイクル状態。 */
export type AgentStatus = "idle" | "starting" | "running" | "stopping" | "failed";

/** 設定ファイルの種別。実ファイル名の解決は Rust 側が行う。 */
export type ConfigFileKind = "skill" | "memory" | "construct";

/** LLM のワイヤプロトコル。未指定なら baseUrl から自動判定される。 */
export type Provider = "open_ai_compat" | "anthropic";

/** 推論の深さ。未指定ならリクエストに含めない。 */
export type Effort = "low" | "medium" | "high" | "xhigh" | "max";

/**
 * 認証情報の取得元。
 *
 * 秘密そのものを保持するバリアントは存在しない。実値は OS の資格情報ストアにあり、
 * フロントへは「登録済みかどうか」しか返らない。
 *
 * `unset`（未設定）と `not_required`（認証不要）は別の状態。まとめると、
 * キー未登録のテンプレートが認証ヘッダ無しで外部へ送られ、401 になる。
 */
export type CredentialSource = "unset" | "not_required" | "keyring";

/** 発話の送り手・受け手。`kind` による判別共用体。 */
export type Endpoint =
  | { kind: "user" }
  | { kind: "system" }
  | { kind: "agent"; id: AgentId };

/** GUI 境界を越えるエラー表現。`code` は安定した機械可読値。 */
export interface ErrorPayload {
  code: string;
  message: string;
  detail: string | null;
  agentId: string | null;
  retryable: boolean;
}

/** ユーザーが編集するエージェント定義。 */
export interface AgentSpec {
  id: AgentId;
  name: string;
  modelTemplateId: ModelTemplateId;
  ragSources: string[];
  connectedAgents: AgentId[];
  order: number;
}

/** UI へ渡るエージェントの現在像（定義 + 実行時統計）。 */
export interface AgentSnapshot {
  id: AgentId;
  name: string;
  /** 解決済みのモデル名。テンプレート欠落時は `<unknown>`。 */
  model: string;
  modelTemplateId: ModelTemplateId;
  status: AgentStatus;
  uptimeSecs: number;
  totalTokens: number;
  ragSources: string[];
  connectedAgents: AgentId[];
  order: number;
  lastError: ErrorPayload | null;
}

/** LLM 接続設定のテンプレート。API キーの実値は保持せず、環境変数名だけを持つ。 */
export interface ModelTemplate {
  id: ModelTemplateId;
  name: string;
  /** API の base URL。パス（`/chat/completions` 等）は Rust 側が付ける。 */
  baseUrl: string;
  model: string;
  contextLength: number;
  /** `null` なら送らない。新しめのモデルは temperature 非対応で 400 を返す。 */
  temperature: number | null;
  maxOutputTokens: number;
  /**
   * 認証情報の取得元。**キーの実値はこの型のどこにも現れない。**
   * 登録の有無は `model_credential_exists` で別途問い合わせる。
   */
  credential: CredentialSource;
  provider: Provider | null;
  useTools: boolean;
  effort: Effort | null;
  requestTimeoutSecs: number;
  maxRetries: number;
}

/** 会話ログの 1 発話。 */
export interface AgentMessage {
  id: string;
  from: Endpoint;
  to: Endpoint;
  content: string;
  tokens: number;
  tsMs: number;
  /** ユーザー入力を起点とした転送回数。無限往復を止める燃料。 */
  hop: number;
  /** 同報の全宛先（受信者自身を含む）。単独宛では省かれる。 */
  coRecipients?: AgentId[];
}

/** トポロジーの有向辺。 */
export interface TopologyEdge {
  source: AgentId;
  target: AgentId;
}

/** RAG 索引の断片。 */
export interface RagChunk {
  id: string;
  source: string;
  text: string;
}

/** コア層から押し出される状態変化。`type` による判別共用体。 */
export type CoreEvent =
  | { type: "agentStatusChanged"; agentId: AgentId; status: AgentStatus }
  | {
      type: "agentStatsUpdated";
      agentId: AgentId;
      uptimeSecs: number;
      totalTokens: number;
    }
  | { type: "messageSent"; message: AgentMessage }
  | { type: "topologyChanged" }
  | { type: "agentFailed"; agentId: AgentId; error: ErrorPayload }
  | {
      type: "backendDegraded";
      modelTemplateId: ModelTemplateId;
      reason: string;
    }
  | { type: "hopLimitReached"; agentId: AgentId; maxHops: number };

/** 設定ファイル種別と表示名の対応。Rust 側の実ファイル名と揃えてある。 */
export const CONFIG_FILE_LABELS: Record<ConfigFileKind, string> = {
  skill: "SKILL.md",
  memory: "Memory.md",
  construct: "Construct.md",
};

/** 状態と表示色の対応。 */
export const STATUS_LABELS: Record<AgentStatus, string> = {
  idle: "停止中",
  starting: "起動中",
  running: "稼働中",
  stopping: "停止処理中",
  failed: "失敗",
};
