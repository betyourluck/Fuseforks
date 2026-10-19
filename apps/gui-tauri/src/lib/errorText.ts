/**
 * `ErrorPayload` の表示文字列（Spec 13 P4 — 多言語化 3 層の (2)、案 A）。
 *
 * コアは日本語のまま返し（**コアは言語を知らない**）、UI がここで
 * `errors.{code}` の辞書を引いて訳す。`code` は data_contract の
 * `ErrorPayload.codes` に凍結された安定キーで、これが訳の鍵になる（D5）。
 *
 * - 日本語表示: コアの `message` をそのまま出す。可変部（テンプレート名・
 *   件数など）込みの一次情報で、訳の二重管理を作らない
 * - 他言語表示: 訳語のあとに**原文を併記**する。`message` の可変部は
 *   辞書が持てないので、訳語だけにするとその情報が失われる。原文が残れば
 *   不具合報告の grep も成立する
 * - 未知の `code`（新しいコアと古い辞書の組み合わせ）: 原文へ落とす
 * - 接続先が 60 秒を超える待ちを求めて止まったとき（`retryAfterSecs`）: 表示の言語で
 *   「接続先は N 秒後の再試行を求めています」を `message` の前に添える（2026-10-19）。
 *   それまではコアの LLM の層が日本語固定の文を `message` の中へ書き足しており、
 *   英語の表示に日本語の案内が混ざった。鍵 `errors.retryAfterTooLong` はコードではない
 *   （コードは大文字の UPPER_SNAKE なので `errors.${code}` の引きとは重ならない）
 * - 接続先が画像を受け付けなかったとき（`imagesRejected`）: 同じ形で「この接続先は画像を
 *   受け付けません…画像なしで送り直してください」を添える（2026-10-20。JPEG フォールバックの
 *   結末。それまではコアが日本語固定の文で本文を差し替えていた）。鍵は `errors.imagesRejected`
 */
import { i18n } from "../i18n";
import type { ErrorPayload } from "../types";

/** コアが運んだ事実から、表示の言語の案内を組む。古い記録（欄の無い lastError）は欄が `undefined`。 */
function guidance(payload: ErrorPayload): string[] {
  const lines: string[] = [];
  if (payload.retryAfterSecs != null) {
    lines.push(i18n.global.t("errors.retryAfterTooLong", { secs: payload.retryAfterSecs }));
  }
  if (payload.imagesRejected === true) lines.push(i18n.global.t("errors.imagesRejected"));
  return lines;
}

export function formatError(payload: ErrorPayload): string {
  const { code, message } = payload;
  const body = [...guidance(payload), message].join(" ");
  if (i18n.global.locale.value === "ja") return `[${code}] ${body}`;

  const key = `errors.${code}`;
  if (!i18n.global.te(key)) return `[${code}] ${body}`;
  return `[${code}] ${i18n.global.t(key)} — ${body}`;
}
