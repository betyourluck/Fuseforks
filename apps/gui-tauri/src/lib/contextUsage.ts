/**
 * コンテキスト使用率の輪（Spec 49）— 純関数。
 *
 * 分子は**直近の LLM 呼び出し 1 回**の入力トークン（`AgentSnapshot.lastPromptTokens`。
 * キャッシュ込み・ツール取得込み）、分母はモデルテンプレートの `contextLength`。
 * **ターンの合計（`turn:` 行の `prompt=`）は使わない** — 全周の和なので 6 周のターンで
 * 窓の何倍にもなる（D1 の罠）。
 *
 * 色の閾値は利用者の指定（「75% 以上は黄、90% 以上は赤、通常は青」）を境界値込みで
 * 固定した定数で、設定にしない（D4）。
 */

/** これ以上で `warn`。 */
export const CONTEXT_WARN_RATIO = 0.75;
/** これ以上で `fail`。 */
export const CONTEXT_FAIL_RATIO = 0.9;

export type ContextTone = "text-accent" | "text-warn" | "text-fail";

/**
 * 比。**出さない条件を `null` で返す** — 分子が `null`（まだ 1 度も呼び出していない）/
 * 分母が無いか 0（テンプレートが引けない）。0 で割らない。
 *
 * 1.0 を超える値はそのまま返す（切り詰めない）— 超えているのに動いている =
 * `contextLength` の設定が実際の窓より小さい、という診断を数字から読むため（D2）。
 */
export function contextRatio(
  lastPromptTokens: number | null | undefined,
  contextLength: number | null | undefined,
): number | null {
  if (lastPromptTokens === null || lastPromptTokens === undefined) return null;
  if (!contextLength || contextLength <= 0) return null;
  return lastPromptTokens / contextLength;
}

/** 3 段の色。境界値は「以上」（0.75 ちょうどで `warn`、0.9 ちょうどで `fail`）。 */
export function contextTone(ratio: number): ContextTone {
  if (ratio >= CONTEXT_FAIL_RATIO) return "text-fail";
  if (ratio >= CONTEXT_WARN_RATIO) return "text-warn";
  return "text-accent";
}

/** 弧の長さの比。**1.0 で止める** — 超えた比をそのまま渡すと弧が 1 周を超えて描かれる。 */
export function contextArc(ratio: number): number {
  return Math.min(Math.max(ratio, 0), 1);
}

/** 表示の %。**丸めない** — 100 を超える数字はそのまま出す（D2 の診断）。 */
export function contextPercent(ratio: number): number {
  return Math.round(ratio * 100);
}
