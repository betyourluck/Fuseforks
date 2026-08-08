/**
 * 作業フォルダの一括切り替えの規則（Spec 29 D2 / D3）。
 *
 * **`mutate()` を通さない**のがこの層の前提。`mutate` は `finally` で必ず
 * `refreshAll()` を呼ぶので、8 体を回すと**全状態の再同期が 8 回**走り、
 * 失敗のたびにトーストが 8 枚出る。一括では個体ごとに捕まえて**結果を 1 つに
 * まとめ**、読み直しは呼び出し側が**最後に 1 回**行う（承認時の注記 A —
 * 適用後に一覧の現在値表示も揃う）。
 *
 * ここに実在検査は無い（D2 — 囲いはツール実行時の `resolve_in_work_dir` が持つ）。
 * [`canApply`] が見るのは**パスの有無と対象の数**だけで、中身は見ない
 * （D3 — 実在検査ではなく誤操作防止の活性制御）。
 */

import type { AgentId } from "../types";

/** 適用の対象 1 体。名前を持つのは、失敗を**名指しで**返すため。 */
export interface BatchTarget {
  id: AgentId;
  name: string;
}

/** 適用の結末 1 体ぶん。 */
export interface BatchOutcome {
  id: AgentId;
  name: string;
  /** 失敗の理由。成功なら `null`。 */
  reason: string | null;
}

/** 適用の結果。**成功数だけでなく失敗を名指しで持つ**（D2）。 */
export interface BatchSummary {
  outcomes: BatchOutcome[];
  succeeded: number;
  failed: BatchOutcome[];
}

/**
 * 適用ボタンを押せるか。**パスの中身は見ない。**
 *
 * 空のまま押せなくするのは、一括の空が**入力し忘れの形**のほうがずっと多い
 * ため（単体ダイアログの空は「未設定へ戻す」という名指しの意図だが、
 * まとめてクリアの需要は観測されていない — D3）。
 */
export function canApply(path: string, targetCount: number): boolean {
  return path.trim().length > 0 && targetCount > 0;
}

/**
 * 対象を**逐次**処理する。1 体が失敗しても残りは続行する（D2）。
 *
 * 並列にしないのは、`update_agent` が 1 呼び出しごとに `world.json` の
 * 書き込みを含むため — 並列にすると書き込みが交錯する形を自分から作る。
 *
 * **タイムアウトは持たない**（D2 の反証記録）。`update_agent` は
 * ネットワークを渡らないローカル IPC で、この村の変更系はどれも
 * タイムアウトを持たない。ここだけ足すと単体保存と規律が割れる。
 *
 * @param update 1 体を更新する。**失敗は例外で投げる**（`ipc` の作法）
 * @param describe 例外を人が読める 1 行へ。i18n を知る側の責務
 * @param onProgress 完了した件数。進捗表示（n / N）に使う
 */
export async function applyWorkDir(
  targets: readonly BatchTarget[],
  update: (target: BatchTarget) => Promise<void>,
  describe: (error: unknown) => string,
  onProgress?: (done: number, total: number) => void,
): Promise<BatchSummary> {
  const outcomes: BatchOutcome[] = [];

  for (const target of targets) {
    try {
      await update(target);
      outcomes.push({ id: target.id, name: target.name, reason: null });
    } catch (error) {
      // **握り潰さない。** 理由を落とすと、失敗した個体をどう直せばよいかが
      // 画面から読めなくなる（#72 と同じ形）。
      outcomes.push({ id: target.id, name: target.name, reason: describe(error) });
    }
    onProgress?.(outcomes.length, targets.length);
  }

  const failed = outcomes.filter((outcome) => outcome.reason !== null);
  return { outcomes, succeeded: outcomes.length - failed.length, failed };
}
