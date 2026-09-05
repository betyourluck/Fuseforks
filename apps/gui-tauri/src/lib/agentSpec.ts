/**
 * 投影（`AgentSnapshot`）から `AgentSpec` を組み直す 1 実装（Spec 29 D4）。
 *
 * `update_agent` は **spec 丸ごとの差し替え**なので、呼ぶ側は投影から全欄を
 * 写して組み直す。この組み立ては `AgentSettingsDialog.seed()` と
 * `useOrchestrator.setBatchStart()` に複製されており、**写し忘れた欄は保存の
 * たびに既定へ戻る**（`batchStart` を写し忘れると、設定を開いて保存しただけで
 * 一括起動の対象へ黙って復帰する — 実装済みの警告コメントが 2 箇所にあった）。
 *
 * **新しい欄の写し忘れはコンパイラが捕まえる**（戻り値の型が `AgentSpec` なので
 * 欄が欠けるとエラー）。**捕まえないのは「写したが値が違う」** — 既定値の直書きや
 * 別の欄からの複写は型が合ってしまう。そちらはテストが、既定値と重ならない
 * 値を持つ投影で全欄を突き合わせて留める。
 */

import type { AgentSnapshot, AgentSpec } from "../types";

/**
 * 投影から spec を組み直す。`overrides` で差し替える欄だけを指定する。
 *
 * **`id` は上書きできない**（`Omit`）— `id` を変えると `update_agent` の
 * 宛先そのものが変わり、正当な使い道が 1 つも無い。
 *
 * 配列は複製する — 呼び出し元（設定ダイアログの下書き）が編集しても
 * 投影を汚さないため。
 */
export function snapshotToSpec(
  snapshot: AgentSnapshot,
  overrides?: Partial<Omit<AgentSpec, "id">>,
): AgentSpec {
  return {
    id: snapshot.id,
    name: snapshot.name,
    modelTemplateId: snapshot.modelTemplateId,
    ragSources: [...snapshot.ragSources],
    connectedAgents: [...snapshot.connectedAgents],
    order: snapshot.order,
    workDir: snapshot.workDir,
    maxToolIterations: snapshot.maxToolIterations,
    enabledTools: snapshot.enabledTools ? [...snapshot.enabledTools] : null,
    hearsRoomLog: snapshot.hearsRoomLog,
    allowHandoff: snapshot.allowHandoff,
    planReview: snapshot.planReview,
    batchStart: snapshot.batchStart,
    roleId: snapshot.roleId,
    groupId: snapshot.groupId,
    ...overrides,
  };
}
