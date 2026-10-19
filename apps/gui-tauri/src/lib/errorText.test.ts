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
  return { code, message, detail: null, agentId: null, retryable: false, retryAfterSecs: null, imagesRejected: false };
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

  // 2026-10-19: 秒数の文はコアの LLM の層が日本語固定で本文へ書き足していた（英語の表示に日本語が
  // 混ざった）。コアは秒数を `retryAfterSecs` で運び、文にするのはここ。
  it("接続先が 60 秒を超える待ちを求めて止まったときは、表示の言語で秒数を添える", () => {
    const stopped: ErrorPayload = {
      ...payload("LLM_API", 'API エラー (status=429): {"error":{"message":"slow down"}}'),
      retryAfterSecs: 3600,
    };

    i18n.global.locale.value = "en";
    const en = formatError(stopped);
    expect(en).toContain("The provider asks to retry after 3600 seconds.");
    expect(en).not.toContain("秒後の再試行");
    expect(en).toContain("slow down");

    i18n.global.locale.value = "ja";
    const ja = formatError(stopped);
    expect(ja).toContain("プロバイダは 3600 秒後の再試行を求めています。");
    expect(ja).toContain("slow down");
  });

  // 2026-10-20: JPEG フォールバックの「この接続先は画像を受け付けません」も、コアの LLM の層が日本語固定で
  // 本文を差し替えていた。コアは事実を `imagesRejected` で運び、文にするのはここ。
  it("接続先が画像を受け付けなかったときは、表示の言語で原因と次の手を添える", () => {
    const rejected = {
      ...payload("LLM_API", "API エラー (status=400): unsupported image format"),
      imagesRejected: true,
    };

    i18n.global.locale.value = "en";
    const en = formatError(rejected);
    expect(en).toContain("This endpoint does not accept images");
    expect(en).not.toContain("画像を受け付けません");
    expect(en).toContain("unsupported image format");

    i18n.global.locale.value = "ja";
    const ja = formatError(rejected);
    expect(ja).toContain("この接続先は画像を受け付けません");
    expect(ja).toContain("unsupported image format");
  });

  it("秒数が無ければ何も添えない（従来の表示のまま）", () => {
    const text = formatError(payload("LLM_API", "API エラー (status=500): boom"));
    expect(text).toBe("[LLM_API] API エラー (status=500): boom");
  });

  it("未知の code は原文へ落とす", () => {
    i18n.global.locale.value = "en";
    const text = formatError(payload("SOME_FUTURE_CODE", "未来のエラー"));
    expect(text).toBe("[SOME_FUTURE_CODE] 未来のエラー");
  });
});
