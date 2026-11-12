/**
 * 会話の単位で画面を絞る規則（Spec 75 D10）。
 *
 * 処理は依頼した会話（根の会話）に結び付いており、別の会話を開いている間も走り続ける。
 * コアはイベントに根の会話の ID を載せるので、画面は**開いている会話の分だけ**を会話ペインへ
 * 出し、他の会話の分は知らせ（トーストと「開く」）と「別の会話」の印にする。
 *
 * 純関数だけ。i18n も状態も知らない（訳語ではなく辞書の鍵を返す）。
 */

/**
 * コアが保存先の無い村の会話に付ける印（`SessionRef::none().label()`）。
 * 会話の単位が無いので、**常に開いている扱い**にする（Spec 75 D13）。
 */
export const NO_SESSION = "-";

/**
 * その会話がいま開いている会話か。
 *
 * `null` / `undefined`（会話を持たない記録・欄の無い旧い記録）と {@link NO_SESSION} は開いている扱い —
 * 比べる相手が無いものを「別の会話」と出すと、保存先の無い村で全部が「別の会話」になる。
 */
export function isOpenSession(sessionId: string | null | undefined, current: string): boolean {
  if (sessionId === null || sessionId === undefined || sessionId === NO_SESSION) return true;
  return sessionId === current;
}

/** イベントの会話 ID を記録の形へ（`"-"` は `null`）。`PlanWaveRecord.sessionId` と揃える。 */
export function sessionOfLabel(label: string): string | null {
  return label === NO_SESSION ? null : label;
}

/** 知らせの数え（同じ会話への知らせは 1 枚にまとめる — Spec 75 D10）。 */
export interface AnswerNotice {
  key: "orchestrator.answerElsewhere" | "orchestrator.answerElsewhereMany";
  /** その会話へ届いた答えの件数（このトーストが出てから）。 */
  count: number;
}

/**
 * 開いていない会話にサーヴァントの答えが届いたときの知らせ。
 *
 * `previous` は同じ会話へのトーストがまだ出ているときの件数（閉じられていれば `null`）。
 * 予定の波で 5 体が答えるとトーストが 5 枚並ぶので、まとめて最後の送り手と件数を出す。
 */
export function answerNotice(previous: number | null): AnswerNotice {
  const count = (previous ?? 0) + 1;
  return {
    key: count === 1 ? "orchestrator.answerElsewhere" : "orchestrator.answerElsewhereMany",
    count,
  };
}
