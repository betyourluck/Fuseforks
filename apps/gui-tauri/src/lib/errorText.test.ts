/**
 * エラー表示（{@link formatError}）の規律（Spec 13 P4・案 A）。
 *
 * - 日本語ではコアの message が一次情報（辞書を経由しない）
 * - 他言語では訳語 + **原文併記**（message の可変部を失わない）
 * - 未知の code は原文へ落とす（新しいコアと古い辞書の組み合わせで
 *   空文字や生キーを画面に出さない）
 */
import { afterEach, describe, expect, it } from "vitest";

import { i18n } from "../i18n";
import { formatError } from "./errorText";
import type { ErrorPayload } from "../types";

function payload(code: string, message: string): ErrorPayload {
  return { code, message, detail: null, agentId: null, retryable: false };
}

afterEach(() => {
  i18n.global.locale.value = "ja";
});

describe("formatError", () => {
  it("日本語ではコアの message をそのまま出す", () => {
    const text = formatError(payload("AGENT_NOT_FOUND", "エージェント `x` は登録されていません"));
    expect(text).toBe("[AGENT_NOT_FOUND] エージェント `x` は登録されていません");
  });

  it("他言語では訳語のあとに原文を併記する（可変部を失わない）", () => {
    i18n.global.locale.value = "en";
    const text = formatError(payload("CREDENTIAL_MISSING", "モデルテンプレート `tpl` の API キーが未登録です"));
    expect(text).toContain("[CREDENTIAL_MISSING]");
    expect(text).toContain("The API key is not registered");
    // 原文（可変部）が残ること — 訳語だけにするとテンプレート名が失われる。
    expect(text).toContain("`tpl`");
  });

  it("未知の code は原文へ落とす", () => {
    i18n.global.locale.value = "en";
    const text = formatError(payload("SOME_FUTURE_CODE", "未来のエラー"));
    expect(text).toBe("[SOME_FUTURE_CODE] 未来のエラー");
  });
});
