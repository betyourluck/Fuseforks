/**
 * 入退室の通知の判定（会話ペインの表示切り替え用・2026-08-08 利用者要望）。
 *
 * **これは表示だけの判定で、プロンプトには 1 バイトも効かない。**
 * モデルへ届く経路は `compose_presence_notices`（コア側）で、そちらは
 * `shared.log` の直近 `room_log_window` 件から System 発を拾う別の機構。
 * ここで隠しても送信量は変わらない（隠すと減ると読まれると誤解になる）。
 *
 * ## 文字列で見ている理由と、その代償
 *
 * ワイヤの `AgentMessage` に種別の欄が無いので、**文末で見るしかない**。
 * 種別を足すのはコアの型とワイヤを変える変更で、表示の都合には重すぎる
 * （`data_contract` の `Endpoint` / `AgentMessage` を触ることになる）。
 *
 * 代償は**コア側の文言を変えるとここが黙って効かなくなる**こと。型検査にも
 * lint にも掛からないので、`presenceNotice.test.ts` が
 * **`orchestrator.rs` を読んで文言の一致を機械で留める**
 * （`defaultEnabledTools.test.ts` / `toolLabel.test.ts` と同じ形）。
 */

import type { AgentMessage } from "../types";

/**
 * 定常の入退室の文末。コア（`orchestrator.rs` の `set_status`）が組む文字列と
 * 1 対 1 で、`presenceNotice.test.ts` が実ソースと突き合わせている。
 */
export const PRESENCE_SUFFIXES = ["が稼働を開始しました", "が停止しました"] as const;

/**
 * 失敗による停止の文末。**隠さない。**
 *
 * カードは「いまの状態」しか示さないので、**過去に落ちた事実が残るのは
 * 会話ログだけ**。起動し直せばカードは稼働中に戻り、失敗した痕跡はどこにも
 * 無くなる。他に置き場のない情報は隠さない（「閉じる前の確認」で予定の行だけは
 * 常に出す、と同じ判断）。
 *
 * なお `が停止しました` とは一致しない — こちらは「により停止しました」なので、
 * 直前の 1 字が違う。**その精度が効いていることをテストが負の対照で見ている。**
 */
export const PRESENCE_FAILURE_SUFFIX = "が失敗により停止しました";

/**
 * その発話が「隠してよい入退室の通知」か。
 *
 * 送り手が System・宛先が User の通知のうち、定常の起動・停止だけを真にする。
 * 予定の飛ばし（`…への予定「…」を飛ばしました（停止中）`）や予算切れなどの
 * System 通知はここに落ちない — **隠すのは繰り返し出るものだけ**で、
 * 1 回きりの告知は隠す価値がない。
 */
export function isPresenceNotice(message: AgentMessage): boolean {
  if (message.from.kind !== "system" || message.to.kind !== "user") return false;
  return PRESENCE_SUFFIXES.some((suffix) => message.content.endsWith(suffix));
}
