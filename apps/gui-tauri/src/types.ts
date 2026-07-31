/**
 * Rust 側 `agent-core` のドメイン型に 1 対 1 で対応する TypeScript 定義。
 *
 * **この 2 つは手で同期させる契約になっている。** Rust 側の `crates/agent-core/src/model.rs`
 * および `event.rs` のフィールドを増減させたら、必ずここも直すこと。
 * serde は `rename_all = "camelCase"` を指定しているので、命名はキャメルケースで一致する。
 */

/** エージェント識別子。Rust 側は透過的な newtype なので、ワイヤ上はただの文字列。 */
export type AgentId = string;

/** 接続マップ上のノード座標。稼働状態と違い、再起動後にも復元する表示設定。 */
export interface TopologyPosition {
  x: number;
  y: number;
}

/** モデルテンプレート識別子。 */
export type ModelTemplateId = string;

/** エージェントのライフサイクル状態。 */
export type AgentStatus = "idle" | "starting" | "running" | "stopping" | "failed";

/** 設定ファイルの種別。実ファイル名の解決は Rust 側が行う。 */
export type ConfigFileKind = "skill" | "memory" | "construct" | "mcp";

/**
 * LLM のワイヤプロトコル。未指定なら baseUrl から自動判定される。
 *
 * `gemini` は自動判定されない（明示選択のみ）。Gemini の base URL は
 * OpenAI 互換としても動いており、自動判定を変えると既存の設定が黙って
 * 別のワイヤへ移ってしまうため。
 */
export type Provider = "open_ai_compat" | "anthropic" | "gemini";

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
  /**
   * 同梱ツール（grep / diff）が読める作業フォルダの絶対パス。
   * `null` なら未設定で、ツールは「設定されていない」と答えるだけになる。
   * エージェントはプロンプトインジェクションを受けうるため、読める範囲は
   * ユーザーが明示したフォルダに限る（範囲の強制は Rust 側）。
   */
  workDir: string | null;
  /**
   * 1 回の発話処理で許すツール実行の回数。`null` なら既定値（6）。
   * コーディング用エージェントは調査のツール往復が多く、既定では
   * 途中で打ち切られやすいので個別に引き上げられる。
   */
  maxToolIterations: number | null;
  /**
   * 提示する同梱ツール名。`null` = 既定に従う（全提示。新しい同梱ツールも
   * 自動で増える）。明示配列 = 必要な道具だけ（自動で増えない）。
   * 新規作成時の保存値は null。作業フォルダ未設定によるファイル系の
   * 自動除外は、このリストより優先される（Rust 側で強制）。
   */
  enabledTools: string[] | null;
  /**
   * 広場ログ（他エージェント同士の会話）を受け取るか。既定 true。
   * 受信側だけの設定 — false でも自分の発話は他者の広場ログに載る
   * （プライバシー機能ではなくコスト機能）。
   */
  hearsRoomLog: boolean;
  /**
   * 一括起動（左ペインの ▶）の対象にするか。既定 true。
   *
   * **自動起動ではない** — アプリを開いた時点では誰も走らず、▶ を押したときに
   * 「どれを起こすか」の選択だけを持つ。**稼働状態とも別**（それは `status`）。
   */
  batchStart: boolean;
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
  /** 同梱ツール（grep / diff）の作業フォルダ。未設定なら `null`。 */
  workDir: string | null;
  /** ツール実行回数の個別上限。`null` なら既定値。 */
  maxToolIterations: number | null;
  /** 提示する同梱ツール名。`null` なら既定（全提示）。 */
  enabledTools: string[] | null;
  /** 広場ログを受け取るか。 */
  hearsRoomLog: boolean;
  /** 一括起動（▶）の対象か。稼働状態とは別（それは `status`）。 */
  batchStart: boolean;
  /**
   * 累積トークンのうち入力（プロンプト）側。
   * **キャッシュ率の分母はこちら。出力はキャッシュできないので、
   * 合計を分母にすると天井が 100% にならず、取り残し量が読めない。**
   */
  promptTokens: number;
  /** 入力トークンのうち、プロンプトキャッシュから読まれた分。 */
  cachedTokens: number;
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
  /**
   * Google 検索による接地。**`provider === "gemini"` のときだけ効く。**
   * OpenAI 互換の口は `google_search` を 400 で拒否するため、互換経路のまま
   * 真にしても接地は起きない。関数呼び出しとは併用でき、委譲は止まらない。
   */
  googleSearch: boolean;
  requestTimeoutSecs: number;
  maxRetries: number;
}

/** 参照した web ページ 1 件。 */
export interface GroundingSource {
  uri: string;
  /** ページ表題。取れなければ空文字。 */
  title: string;
}

/**
 * プロバイダが代行して実行した接地の記録（Spec 05）。
 *
 * **`sources` が空であることが「出典は存在しない」の判定**であり、
 * モデルが本文で語る出典を信じない根拠になる。表示層はこの区別を潰さない。
 */
export interface Grounding {
  queries: string[];
  sources: GroundingSource[];
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
  /**
   * 接地の来歴。接地が起きなかった発話では省かれる。
   * **表示専用** — モデルへは戻らない（Spec 05 Notes 9）。
   */
  grounding?: Grounding;
}

/** MCP サーバー 1 台の起動方法（Claude Desktop の設定と同じ形）。 */
export interface McpServerConfig {
  command: string;
  args: string[];
  /** 追加の環境変数。**秘密は書かないこと** — mcp.json は平文で保存される。 */
  env: Record<string, string>;
  /** 設定を消さずに一時停止するための欄。 */
  enabled: boolean;
}

/** `mcp.json` 全体。キー名は Claude Desktop 互換。 */
export interface McpConfig {
  mcpServers: Record<string, McpServerConfig>;
}

/** MCP サーバー 1 台の接続状態。 */
export interface McpServerStatus {
  name: string;
  connected: boolean;
  /** 提供されたツール名（サーバー名で修飾済み）。 */
  tools: string[];
  error: string | null;
}

/**
 * エージェント別 MCP の状態（Spec 02）。
 * 接続はエージェントの稼働に紐付くため、停止中は `running: false` で
 * サーバー一覧は空になる（状態は永続化されない）。
 */
export interface AgentMcpStatus {
  running: boolean;
  /** mcp.json の読み込み失敗（外部編集で壊れた場合）。 */
  loadError: string | null;
  servers: McpServerStatus[];
}

/** トポロジーの有向辺。 */
export interface TopologyEdge {
  source: AgentId;
  target: AgentId;
}

// ---- 予定（Spec 07） ----------------------------------------------------------

/** 曜日。Rust 側 `schedule::Weekday` と同じ表記。 */
export type Weekday = "mon" | "tue" | "wed" | "thu" | "fri" | "sat" | "sun";

/**
 * 再現規則。`kind` による判別共用体。
 *
 * cron 式は採らない（読めない人には一切読めず、UI も自由入力欄にしかならない）。
 * 要望の 2 例（毎週 X 曜 hh:mm / 定期的に）が言い切れる最小の構造。
 */
export type Recurrence =
  | { kind: "interval"; everyMinutes: number }
  | { kind: "daily"; hour: number; minute: number }
  | { kind: "weekly"; weekday: Weekday; hour: number; minute: number };

/**
 * 予定一覧の 1 行。`nextDueMs` と `recurrenceLabel` はコア側で算出される —
 * フロントはカレンダー計算を持たない（真実が 2 箇所できる）。
 */
export interface ScheduleView {
  id: string;
  to: AgentId;
  message: string;
  recurrence: Recurrence;
  createdAtMs: number;
  /** 直近に「消化した」予定時刻。発火時刻ではない。 */
  lastConsumedDueMs: number | null;
  /** 偽なら発火も消化もしない（設定を消さずに一時停止する欄）。 */
  enabled: boolean;
  /** 次回の発火予定時刻（epoch ミリ秒）。求まらなければ null。 */
  nextDueMs: number | null;
  /** 再現規則の日本語表記（「毎週 木曜 17:00」）。配送本文の由来と同じ関数。 */
  recurrenceLabel: string;
}

/** 曜日の表示名。Rust 側 `Weekday::label_ja` と同じ語彙。 */
export const WEEKDAY_LABELS: Record<Weekday, string> = {
  mon: "月曜",
  tue: "火曜",
  wed: "水曜",
  thu: "木曜",
  fri: "金曜",
  sat: "土曜",
  sun: "日曜",
};

/** RAG 索引の断片。 */
export interface RagChunk {
  id: string;
  source: string;
  text: string;
}

/**
 * plan の 1 タスクの解決分類（Spec 08 — 波ペイン）。
 *
 * 文言 parse では取らない — コアが型で刻んだ値がそのまま届く。
 * セル色の対応は data_contract.yaml の PlanTaskState が正。
 */
export type PlanTaskState =
  | "running"
  | "answered"
  | "handed_off"
  | "undeliverable"
  | "no_answer"
  | "timed_out";

/** `planWaveStarted` が運ぶタスクの告知形（開始時点で確定している 2 欄だけ）。 */
export interface PlanTaskAnnounced {
  to: AgentId;
  msgChars: number;
}

/** 波の 1 タスクの記録。同一性は `(planId, to)`（同一宛先の重複は静的な不正）。 */
export interface PlanTaskRecord {
  to: AgentId;
  state: PlanTaskState;
  /** 配送からこのタスクの解決まで。相手のキュー待ちを含む（並列なのは配送）。 */
  elapsedMs: number | null;
  msgChars: number;
}

/** plan 1 波の実行記録。所有者はコアの in-memory（リング上限 50・プロセス寿命）。 */
export interface PlanWaveRecord {
  /** プロセス内で単調増加。1 始まり・0 は予約。 */
  planId: number;
  /** 進行役。 */
  agentId: AgentId;
  /** ターン内連番（ターンを跨いで重複する。同定は planId の仕事）。 */
  wave: number;
  startedAtMs: number;
  /** 入力順（束ねと同じ。解決順ではない）。 */
  tasks: PlanTaskRecord[];
  /** 波の完了時に埋まる。 */
  bundleChars: number | null;
  /** 波全体の所要（= キュー待ち込みの最遅 1 体分）。波の完了時に埋まる。 */
  elapsedMs: number | null;
}

/** コア層から押し出される状態変化。`type` による判別共用体。 */
export type CoreEvent =
  | { type: "agentStatusChanged"; agentId: AgentId; status: AgentStatus }
  | {
      type: "agentStatsUpdated";
      agentId: AgentId;
      uptimeSecs: number;
      totalTokens: number;
      /** キャッシュ率の分母・分子。合計だけだと率が refreshAll 頼みになり、
          再起動後の会話で欄ごと消える（failures.md #33 の経路版）。 */
      promptTokens: number;
      cachedTokens: number;
    }
  | { type: "messageSent"; message: AgentMessage }
  | { type: "topologyChanged" }
  | { type: "agentFailed"; agentId: AgentId; error: ErrorPayload }
  | {
      type: "backendDegraded";
      modelTemplateId: ModelTemplateId;
      reason: string;
    }
  | { type: "agentTyping"; agentId: AgentId; active: boolean }
  | { type: "conversationCleared" }
  | { type: "toolInvoked"; agentId: AgentId; tool: string; ok: boolean }
  | { type: "toolLimitReached"; agentId: AgentId; maxIterations: number }
  /** 同じツール呼び出しの繰り返しを検出して実行せずに打ち切った
      （failures.md #41 の処方 1）。上限到達とは別の打ち切りで、直し方も違う。 */
  | { type: "toolRepeatBlocked"; agentId: AgentId; tool: string; repeats: number }
  | { type: "hopLimitReached"; agentId: AgentId; maxHops: number }
  // Spec 08（波ペイン）。順序保証は per planId のみ（Started → Resolved* → Finished）。
  | {
      type: "planWaveStarted";
      planId: number;
      agentId: AgentId;
      wave: number;
      tasks: PlanTaskAnnounced[];
      startedAtMs: number;
    }
  | {
      type: "planTaskResolved";
      planId: number;
      to: AgentId;
      state: PlanTaskState;
      elapsedMs: number;
    }
  | {
      type: "planWaveFinished";
      planId: number;
      bundleChars: number;
      elapsedMs: number;
    };

/** 設定ファイル種別と表示名の対応。Rust 側の実ファイル名と揃えてある。 */
export const CONFIG_FILE_LABELS: Record<ConfigFileKind, string> = {
  skill: "SKILL.md",
  memory: "Memory.md",
  construct: "Construct.md",
  /** エージェント別 MCP。保存時に JSON 検証があり、壊れた内容は保存拒否される。 */
  mcp: "mcp.json",
};

/** 状態と表示色の対応。 */
export const STATUS_LABELS: Record<AgentStatus, string> = {
  idle: "停止中",
  starting: "起動中",
  running: "稼働中",
  stopping: "停止処理中",
  failed: "失敗",
};
