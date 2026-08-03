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
 */
import { i18n } from "../i18n";
import type { ErrorPayload } from "../types";

export function formatError(payload: ErrorPayload): string {
  const { code, message } = payload;
  if (i18n.global.locale.value === "ja") return `[${code}] ${message}`;

  const key = `errors.${code}`;
  if (!i18n.global.te(key)) return `[${code}] ${message}`;
  return `[${code}] ${i18n.global.t(key)} — ${message}`;
}
