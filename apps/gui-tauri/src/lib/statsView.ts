/**
 * 統計画面（Spec 39）の表示規則 — 純関数だけ。
 *
 * 数字はコアの `aggregate` から来る（`StatsReport`）。ここが持つのは「どう並べるか・
 * どう描くか」であって「いくらか」ではない — 実効の重みも比も再計算しない
 * （`stats_contract`: フロントで再計算しない）。
 */

import type { SeriesPoint, StatsReport, TurnStop } from "../types";

/**
 * 画面の 3 状態。**`recordedSince === null` は「記録が無い」であって「0」ではない**
 * （D6）— この版より前の会話は `Turn` を持たない。0 の表を出すと
 * 「払っていない」と読まれる（#68 の裏返し）。
 */
export type StatsNotice = "loading" | "empty" | "ready";

export function statsNotice(report: StatsReport | null): StatsNotice {
  if (!report) return "loading";
  if (report.scopeMeta.recordedSince === null) return "empty";
  return "ready";
}

/** 終わり方の種別 → 辞書の鍵（`stats.stop.*`）。閉じた列挙なので `Record` で網羅する。 */
export const STOP_LABEL_KEYS: Record<TurnStop["kind"], string> = {
  completed: "stats.stop.completed",
  repeat: "stats.stop.repeat",
  tool_limit: "stats.stop.toolLimit",
  failed: "stats.stop.failed",
  interrupted: "stats.stop.interrupted",
  budget_exhausted: "stats.stop.budgetExhausted",
  reserve_short: "stats.stop.reserveShort",
};

/**
 * 終わり方の色調。**完走の 3 値は通常色、残りは警告色** — `TurnStop::is_failure` と
 * 同じ境界（払ったが答えが無かった）。色は CSS 変数（`style.css`）から引く。
 */
export function stopTone(kind: TurnStop["kind"]): "ok" | "fail" {
  return kind === "completed" || kind === "repeat" || kind === "tool_limit" ? "ok" : "fail";
}

/** 0〜1 の比を `12.3%` に。有限でなければ `—`。 */
/**
 * キャッシュ率のホバーへ出す入力の内訳（Spec 40 D4）。**列は増やさない** —
 * 8 列の表に 9 列目を足すと横スクロールが常態化する。
 *
 * 返すのは辞書の鍵ではなく**数の組**。訳語はテンプレート側が当てる
 * （`reasonDisplay` / `probeDisplay` と同じ形 — 純関数は i18n を知らない）。
 *
 * **`fresh` は引き算で出す**（`prompt - cached - cacheWrite`）。負にならないよう
 * 0 で止める — 古い記録は `cacheWrite` が 0 なので `fresh` が過大に出るが、
 * **それは「記録していなかった」の正しい表示**（0 を書き込み無しと読ませない）。
 */
export function inputBreakdown(slice: {
  prompt: number;
  cached: number;
  cacheWrite: number;
  cacheWrite1h: number;
}): { cached: number; cacheWrite: number; cacheWrite1h: number; fresh: number } {
  const fresh = Math.max(0, slice.prompt - slice.cached - slice.cacheWrite);
  return {
    cached: slice.cached,
    cacheWrite: slice.cacheWrite,
    cacheWrite1h: slice.cacheWrite1h,
    fresh,
  };
}

export function formatPercent(rate: number): string {
  if (!Number.isFinite(rate)) return "—";
  return `${(rate * 100).toFixed(1)}%`;
}

/** ミリ秒を人が読む形に（1 秒未満は ms、それ以上は小数 1 桁の秒）。 */
export function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return "—";
  if (ms < 1000) return `${Math.round(ms)} ms`;
  return `${(ms / 1000).toFixed(1)} s`;
}

/** SVG の棒 1 本。座標は描画領域内のピクセル。 */
export interface SeriesBar {
  x: number;
  y: number;
  width: number;
  height: number;
  tone: "ok" | "fail";
  point: SeriesPoint;
}

/**
 * ターンごとの実効トークンを棒に写す（時系列 1 本 — D8）。
 *
 * - 高さは**線形**（最大値を `height` に合わせる）。対数にすると 1 本の突出が
 *   読めなくなる — 突出こそ見たいもの（1 ターンで天井の 1/4 を使った添付など）
 * - 全部 0 なら高さ 0 の棒（描画領域を空にしない — 「ターンはあった」が読める）
 * - 棒の幅は件数で割る。`gap` は棒の間。1 px 未満にはしない
 */
export function seriesBars(
  points: SeriesPoint[],
  width: number,
  height: number,
  gap = 1,
): SeriesBar[] {
  if (points.length === 0 || width <= 0 || height <= 0) return [];
  const max = Math.max(0, ...points.map((p) => p.effective));
  const slot = width / points.length;
  const barWidth = Math.max(1, slot - gap);
  return points.map((point, i) => {
    const h = max === 0 ? 0 : (point.effective / max) * height;
    return {
      x: i * slot,
      y: height - h,
      width: barWidth,
      height: h,
      tone: stopTone(point.stop.kind),
      point,
    };
  });
}
