/**
 * UI 文言の多言語化（Spec 13 P3）。
 *
 * **訳すのは UI 文言だけ**（settings_contract の多言語化 3 層）。
 * システムプロンプト・ツール説明はモデルへの入力なので訳さない（層 (3)）。
 * コアの文言は日本語のまま届き、UI が `ErrorPayload.code` で引いて訳す（層 (2)、P4）。
 *
 * 確定した言語は `world.json` の `language` が持つ（村の共有物 — System 行が
 * 会話ログに保存されるため、`localStorage` には置けない）。この module は
 * IPC が答えるまでの仮の値として `ja` で始まり、{@link setLocale} が確定を映す。
 */
import { createI18n } from "vue-i18n";

import en from "../locales/en.json";
import ja from "../locales/ja.json";
import type { Language } from "../types";

export const i18n = createI18n({
  legacy: false,
  globalInjection: true,
  /** IPC が答えるまでの仮の値。従来の見た目（日本語）と一致させる。 */
  locale: "ja",
  /**
   * 訳が欠けたキーは日本語で出す。空白よりも「訳が漏れている」が見える —
   * 辞書の鍵集合は ja / en で一致をテストで固定しているので、平時は使われない。
   */
  fallbackLocale: "ja",
  messages: { ja, en },
});

/**
 * 表示言語を切り替える。`<html lang>` も追従させる
 * （スクリーンリーダーの読み上げ言語とフォント選択がここを見る）。
 */
export function setLocale(locale: Language): void {
  i18n.global.locale.value = locale;
  // vitest（node 環境）には document が居ない。存在検査は本番では常に真。
  if (typeof document !== "undefined") {
    document.documentElement.lang = locale;
  }
}

/** いまの表示言語。 */
export function currentLocale(): Language {
  return i18n.global.locale.value as Language;
}
