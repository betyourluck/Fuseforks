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

import { i18n } from "../i18n";
import type {
  AgentId,
  AgentMcpStatus,
  AgentMessage,
  AgentSnapshot,
  AgentSpec,
  ApprovalOutcome,
  AttachmentPayload,
  BlackboardNote,
  CommandPolicyView,
  ConfigFileKind,
  ErrorPayload,
  ForkPoint,
  Language,
  McpHostStatus,
  ModelTemplate,
  ModelTemplateId,
  Role,
  RoleId,
  McpConfig,
  McpServerStatus,
  PlanWaveRecord,
  Recurrence,
  ScheduleOptions,
  ScheduleView,
  SessionSummary,
  FetchedPrices,
  PricingSourceView,
  StatsReport,
  StatsScope,
  TopologyEdge,
  TopologyPosition,
  WorkDirListing,
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
    message:
      typeof error === "string" ? error : i18n.global.t("orchestrator.ipcFailed"),
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

/** サーヴァントの絆の保存済みノード座標を取得する。 */
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

/** ワークスペースの実パスを取得する。 */
export const workspacePath = () => call<string>("workspace_path");

// ---- 外の LLM から依頼を受ける扉（Spec 25） ----------------------------------

/**
 * 扉の状態を取得する。
 *
 * **合鍵をそのまま返す。** API キー（`modelCredentialExists` が存在しか返さない）
 * とは扱いが逆で、こちらは画面に出してクライアントの設定へ貼るための値。
 */
export const mcpHostStatus = () => call<McpHostStatus>("mcp_host_status");

/** 扉の ON / OFF とポートを保存し、その場で反映する。ON にすると合鍵ができる。 */
export const setMcpHost = (enabled: boolean, port: number) =>
  call<McpHostStatus>("set_mcp_host", { enabled, port });

/** 合鍵を作り直す（開いていれば新しい鍵で開き直す）。 */
export const regenerateMcpHostToken = () =>
  call<McpHostStatus>("regenerate_mcp_host_token");

/** 外部クライアントの呼び名。未設定なら `null`（名乗りをそのまま使う）。 */
export const getExternalName = () => call<string | null>("get_external_name");

/** 外部クライアントの呼び名を設定する。`null` で未設定へ戻す。 */
export const setExternalName = (name: string | null) =>
  call<void>("set_external_name", { name });

/** 外部クライアントのアイコン（WebP バイト列）。未設定なら `null`。 */
export const getExternalIcon = () => call<number[] | null>("get_external_icon");

/** 外部クライアントのアイコンを設定する。 */
export const setExternalIcon = (data: number[]) =>
  call<void>("set_external_icon", { data });

/** 外部クライアントのアイコンを削除する。 */
export const clearExternalIcon = () => call<void>("clear_external_icon");

/** 外部からの依頼を受ける窓口。未設定なら `null`。 */
export const getReception = () => call<AgentId | null>("get_reception");

/** 窓口を差し替える。`null` で未設定へ戻す。 */
export const setReception = (agentId: AgentId | null) =>
  call<void>("set_reception", { agentId });

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

/** サーヴァントの絆で移動したノードの座標を保存する。 */
export const setTopologyPosition = (agentId: AgentId, position: TopologyPosition) =>
  call<void>("set_topology_position", { agentId, position });

/** モデルテンプレートを登録または更新する。 */
export const upsertModelTemplate = (template: ModelTemplate) =>
  call<void>("upsert_model_template", { template });

/** モデルテンプレートを削除する。参照中のエージェントが居れば拒否される。 */
export const deleteModelTemplate = (templateId: ModelTemplateId) =>
  call<void>("delete_model_template", { templateId });

// ---- 役職（Spec 14） --------------------------------------------------------

/** 登録済みの役職一覧。 */
export const listRoles = () => call<Role[]>("list_roles");

/**
 * 役職を登録または更新する。
 *
 * **既存のサーヴァントには何も起きない** — 既定値は新規作成のときにコピー
 * されるので（role_contract 凍結 4）、ここで中身を直しても既に居る個体の設定は
 * 変わらない。変わるのは名前を参照しているバッジと顔ぶれだけ。
 */
export const upsertRole = (role: Role) => call<void>("upsert_role", { role });

/**
 * 役職を削除する。**参照中でも拒まない**（モデルテンプレートとの決定的な差）。
 *
 * 役職はコピー済みなので、消してもサーヴァントの動作は変わらない —
 * バッジと顔ぶれの役職表示が消えるだけ。
 */
export const deleteRole = (roleId: RoleId) => call<void>("delete_role", { roleId });

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
 * 村の黒板（work_dir の `blackboard/`）の付箋を読む。
 *
 * **内容を書く IPC は存在しない。** 書くのはエージェント（file ツール）と人で、
 * 条例の「1 人 1 ファイル」運用を GUI が迂回しない。**削除だけは在る**（下記）—
 * 誰かの名前で内容を書く操作ではないので、その凍結を破らない。
 */
export const listBlackboard = () => call<BlackboardNote[]>("list_blackboard");

/**
 * 付箋を 1 枚**ごみ箱へ移す**。
 *
 * `dir` は一覧が返した `BlackboardNote.dir` をそのまま渡す（コア側が
 * 「いま使われている work_dir か」を検査する）。**完全削除はしない**ので、
 * 個別削除には確認を付けていない。
 */
export const deleteBlackboardNote = (dir: string, name: string) =>
  call<void>("delete_blackboard_note", { dir, name });

/** 付箋を全部ごみ箱へ移す。戻り値は移した枚数。 */
export const clearBlackboard = () => call<number>("clear_blackboard");

// ---- 村の設定（Spec 13） -------------------------------------------------------

/** トークン予算の天井（実効トークン建て）。`null` = 天井なし。 */
export const getTokenBudget = () => call<number | null>("get_token_budget");

/**
 * トークン予算の天井を差し替える。**次の依頼から効く**（再起動不要 —
 * settings_contract の即時反映）。`0` は `INVALID_TOKEN_BUDGET` で拒否されるが、
 * 入力段でも弾くこと（「保存したのに黙って別の値になる」を画面に作らない）。
 */
export const setTokenBudget = (ceiling: number | null) =>
  call<void>("set_token_budget", { ceiling });

/** UI の表示言語。bootstrap が初回に OS から確定済みなので、必ず値が返る。 */
export const getLanguage = () => call<Language>("get_language");

/**
 * UI の表示言語を差し替える。保存されるのは `world.json` の 1 フィールドだけで、
 * システムプロンプトは変わらない（settings_contract の多言語化 3 層）。
 */
export const setLanguage = (language: Language) =>
  call<void>("set_language", { language });

// ---- コマンドの承認（Spec 20） ------------------------------------------------

/** 全サーヴァントの判断待ち要求。読めなかった個体は `broken: true` で返る。 */
export const listCommandRequests = () =>
  call<CommandPolicyView[]>("list_command_requests");

/**
 * 判断待ちの 1 件を承認して `allow` へ入れる。
 *
 * **粒度は `open` だけで指定する。** パターン文字列を送る口は無い —
 * あると「粒度は機械が決めない」が「粒度を GUI が何でも決められる」へ反転する。
 */
export const approveCommand = (
  agentId: AgentId,
  command: string,
  args: string[],
  open: boolean,
) => call<ApprovalOutcome>("approve_command", { agentId, command, args, open });

/** 判断待ちの 1 件を却下して `deny` へ入れる。 */
export const rejectCommand = (
  agentId: AgentId,
  command: string,
  args: string[],
  open: boolean,
) => call<ApprovalOutcome>("reject_command", { agentId, command, args, open });
// ---- 利用者（Spec 19） --------------------------------------------------------

/**
 * 利用者の呼び名。`null` = 未設定。
 *
 * **未設定を既定値へ倒さずそのまま返す** — 画面が「まだ設定していない」ことを
 * 示せるようにするため。封筒の既定（「ユーザー」）はコアが持つ。
 */
export const getUserName = () => call<string | null>("get_user_name");

/**
 * 利用者の呼び名を差し替える。`null` で既定へ戻す。
 *
 * 書式（空 / `】` / 制御文字 / 32 字超）はコアが拒否する（`INVALID_USER_NAME`）。
 * **フロントで先回りして検査しない** — 同じ規律が 2 箇所に生える。
 */
export const setUserName = (name: string | null) =>
  call<void>("set_user_name", { name });

/** 利用者のアイコン（WebP バイト列）。未設定なら `null`。 */
export const getUserIcon = () => call<number[] | null>("get_user_icon");

/** 利用者のアイコンを設定する。`data` は UI 側で WebP へ変換済みであること。 */
export const setUserIcon = (data: number[]) =>
  call<void>("set_user_icon", { data });

/** 利用者のアイコンを削除する。 */
export const clearUserIcon = () => call<void>("clear_user_icon");

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

/**
 * 統計（Spec 39）。`scope` は `{ kind: "session", sessionId }` か `{ kind: "all" }`。
 *
 * 数字はコアの集計（`aggregate` の 1 実装）から出る唯一の経路。`turnRecorded` は
 * id だけを運ぶので、受けたらこれを叩き直す（統計画面を開いている間だけ）。
 */
export const sessionStats = (scope: StatsScope) =>
  call<StatsReport>("session_stats", { scope });

// ---- 単価表（Spec 41） --------------------------------------------------------

/** 取得元の設定を読む。**取りに行かない。** */
export const pricingSourceStatus = () => call<PricingSourceView>("pricing_source_status");

/** 取得元の URL を保存する。**保存しただけでは取りに行かない**（凍結）。 */
export const savePricingSource = (url: string) =>
  call<PricingSourceView>("save_pricing_source", { url });

/**
 * 単価表を取りに行く。**利用者がボタンを押したときだけ呼ぶ唯一の入口。**
 *
 * **起動経路・画面遷移・タイマーから呼んではならない**
 * （`data_contract` の `pricing_fetch_freeze`）。
 */
export const fetchModelPrices = () => call<FetchedPrices>("fetch_model_prices");

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
  attachments?: AttachmentPayload[],
) => call<void>("send_user_message", { agentId, content, coRecipients, attachments });

/**
 * 添付画像の実体（WebP バイト列）を読む（Spec 23）。
 *
 * `null` は「保持期間を過ぎて削除された」（D9）— エラーではなく通常の答えで、
 * 表示側はプレースホルダの枠を出す。
 */
export const readAttachment = (id: string) =>
  call<number[] | null>("read_attachment", { id });

/**
 * 入力欄のパス補完に渡すファイル一覧（Spec 24）。
 *
 * **`@` を打った瞬間に 1 回だけ呼び、補完が開いている間は呼び直さない**（D6）。
 * 作業フォルダが未設定なら空の一覧が返るが、**その判定は呼ぶ前にできる** —
 * `AgentSnapshot.workDir` がフロントにあるので、未設定なら呼ばずに理由を出す。
 */
export const listWorkDirFiles = (agentId: AgentId) =>
  call<WorkDirListing>("list_work_dir_files", { agentId });

// ---- 予定（Spec 07） ----------------------------------------------------------

/** 登録済みの予定（登録順）。次回発火時刻はコア側で算出済み。 */
export const listSchedules = () => call<ScheduleView[]>("list_schedules");

/**
 * 予定を登録する。宛先は停止中でもよいが、未登録なら拒否される。
 *
 * **`options.probe` を伴う登録は、この呼び出しが承認も書く**（Spec 28 D10）—
 * 書いた人 = 承認した人なので、追加の確認は出さない。
 * **承認を書く経路はこの IPC だけ**で、`schedules.json` を直接書いても承認は
 * 付かない（配られた村の前判定が黙って走らないのはこれが根拠）。
 */
export const createSchedule = (
  to: AgentId,
  message: string,
  recurrence: Recurrence,
  options?: ScheduleOptions,
) => call<ScheduleView>("create_schedule", { to, message, recurrence, options });

/**
 * 既存の予定の前判定を、この端末で実行してよいと承認する（Spec 28 D10）。
 *
 * 配られた村・手で書いた `schedules.json` の前判定はここを通るまで走らない。
 * **押す前にコマンド行の原文を画面へ出すこと** — 中身を見ずに押せる形にすると、
 * 承認が「読まずにクリックする儀式」に落ちる。
 */
export const approveScheduleProbe = (id: string) =>
  call<void>("approve_schedule_probe", { id });

/** 予定を削除する。復元はできない。 */
export const deleteSchedule = (id: string) => call<void>("delete_schedule", { id });

/** 予定の一時停止・再開。停止中は発火も消化もされない。 */
export const setScheduleEnabled = (id: string, enabled: boolean) =>
  call<void>("set_schedule_enabled", { id, enabled });
