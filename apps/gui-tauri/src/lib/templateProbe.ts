import type { ProbeFailure, ProbeFinish, TemplateProbe } from "../types";

/**
 * モデルテンプレートの接続テスト（Spec 68）の画面側の規則。訳語ではなく**辞書の鍵**を返す
 * （純関数は i18n を知らない — `reasonDisplay` と同じ形）。
 */

/** 結果の 3 つの列挙。辞書の鍵を実行時に組むので、走査テストがここと ja / en を突き合わせる（D11）。 */
export const PROBE_OUTCOMES = ["replied", "truncated", "no_text"] as const;
export const PROBE_FAILURES: readonly ProbeFailure[] = [
  "config",
  "auth",
  "not_found",
  "rejected",
  "rate_limit",
  "billing",
  "server_error",
  "timeout",
  "network",
  "unexpected_shape",
];
export const PROBE_FINISHES: readonly ProbeFinish[] = ["stop", "length", "tool_use", "other"];

/**
 * ボタンを押せない理由の辞書の鍵。押せるなら `null`（D8）。
 *
 * - 新規の下書き・未保存の変更がある → 保存してから（**確かめるのは保存済みの値だけ** — D1）
 * - 鍵の入力欄に文字が残っている → 登録してから（入力欄は下書きの欄ではないので、未保存の判定では
 *   検知できない。押せると古い鍵か鍵なしの結果が「今打った鍵の結果」に読まれる — rev2 点 1）
 * - 確かめている最中
 */
export function probeBlockedKey(input: {
  dirty: boolean;
  secretTyped: boolean;
  busy: boolean;
}): string | null {
  if (input.busy) return "modelTemplate.probe.busy";
  if (input.dirty) return "modelTemplate.probe.saveFirst";
  if (input.secretTyped) return "modelTemplate.probe.registerKeyFirst";
  return null;
}

/** 結果の区画の色（`--color-run` / `--color-warn` / `--color-fail`）。 */
export type ProbeTone = "run" | "warn" | "fail";

/** 結果の区画に出すもの。 */
export interface ProbeView {
  tone: ProbeTone;
  /** 1 行目の辞書の鍵と引数。 */
  key: string;
  params: Record<string, string | number>;
  /** 払ったトークン（無ければ `null` — 失敗と、本文の無い到達しない腕）。 */
  usage: { prompt: number; completion: number; reasoning: number } | null;
  /** 失敗のときだけ。待ち時間の明示（「接続先は N 秒後を指定しています」）。 */
  retryAfterSecs: number | null;
}

/**
 * 結果から区画の中身を決める（D2・D6・D8）。**成功の文面は「使えます」ではなく「この設定で接続先が応答しました」**
 * （確かめないもの — ツール・固有スキル・添付・長い入力 — があるので、保証の範囲を広げて読ませない）。
 * 出力上限で切れた / 本文が無かった は成功に数えるが、注記なので `warn` で出す。
 */
export function probeView(probe: TemplateProbe): ProbeView {
  const seconds = Math.max(0.1, Math.round(probe.elapsedMs / 100) / 10);
  if (probe.result === "failed") {
    return {
      tone: "fail",
      key: `modelTemplate.probe.failure.${probe.failure}`,
      params: { status: probe.status ?? "-", seconds },
      usage: null,
      retryAfterSecs: probe.retryAfterSecs,
    };
  }
  switch (probe.outcome) {
    case "replied":
      return {
        tone: "run",
        key: "modelTemplate.probe.reached.replied",
        params: { text: probe.text, seconds },
        usage: probe.usage,
        retryAfterSecs: null,
      };
    case "truncated":
      return {
        tone: "warn",
        key: "modelTemplate.probe.reached.truncated",
        params: { limit: probe.limit, seconds },
        usage: probe.usage,
        retryAfterSecs: null,
      };
    case "no_text":
      return {
        tone: "warn",
        key: "modelTemplate.probe.reached.no_text",
        params: { finish: probe.finish ?? "-", seconds },
        usage: probe.usage,
        retryAfterSecs: null,
      };
  }
}

/** `no_text` の終了理由の辞書の鍵（`finish` が無い到達しない腕は `null`）。 */
export function probeFinishKey(probe: TemplateProbe): string | null {
  if (probe.result !== "reached" || probe.outcome !== "no_text" || probe.finish === null) return null;
  return `modelTemplate.probe.finish.${probe.finish}`;
}
