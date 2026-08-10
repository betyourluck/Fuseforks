import { describe, expect, it } from "vitest";

import {
  DEFAULT_BASE_URL,
  baseUrlMismatch,
  passiveSkills,
  presetBaseUrlFor,
  providerSkills,
} from "./providerSkills";
import type { ModelTemplate } from "../types";

type Draft = Pick<
  ModelTemplate,
  "provider" | "googleSearch" | "xaiWebSearch" | "xaiXSearch"
>;

function draft(over: Partial<Draft> = {}): Draft {
  return {
    provider: null,
    googleSearch: false,
    xaiWebSearch: false,
    xaiXSearch: false,
    ...over,
  };
}

describe("baseUrlMismatch", () => {
  it("本当に他社の既定値が残っていれば指摘する", () => {
    expect(baseUrlMismatch("anthropic", DEFAULT_BASE_URL.open_ai_compat)).toBe(
      DEFAULT_BASE_URL.anthropic,
    );
    expect(baseUrlMismatch("gemini", DEFAULT_BASE_URL.anthropic)).toBe(
      DEFAULT_BASE_URL.gemini,
    );
  });

  it("自前のプロキシやローカルサーバは正当なので指摘しない", () => {
    expect(baseUrlMismatch("open_ai_compat", "http://localhost:8080/v1")).toBeNull();
    expect(baseUrlMismatch("anthropic", "https://proxy.example.test/v1")).toBeNull();
  });

  it("自動判定（provider 未指定）では何も言わない", () => {
    expect(baseUrlMismatch(null, DEFAULT_BASE_URL.anthropic)).toBeNull();
  });

  // ここが誤検知だった。互換の口も持つホストを互換で使うのは正当な運用で、
  // **この村の Grok 個体は現にそう動いている**。x.ai を既定表へ足した時点で
  // 既存の設定に嘘の警告が出るのを、この 2 本が止めている。
  it("互換の口も持つホストを OpenAI 互換で使うのは食い違いではない", () => {
    expect(
      baseUrlMismatch("open_ai_compat", DEFAULT_BASE_URL.xai_responses),
    ).toBeNull();
    expect(baseUrlMismatch("open_ai_compat", DEFAULT_BASE_URL.gemini)).toBeNull();
  });

  it("ただしネイティブを選んだのに別ホストなら指摘する（免除は互換側だけ）", () => {
    expect(baseUrlMismatch("xai_responses", DEFAULT_BASE_URL.gemini)).toBe(
      DEFAULT_BASE_URL.xai_responses,
    );
  });
});

describe("presetBaseUrlFor", () => {
  it("既定値のままなら新しいプロトコルの既定へ差し替える", () => {
    expect(
      presetBaseUrlFor("xai_responses", DEFAULT_BASE_URL.open_ai_compat),
    ).toBe(DEFAULT_BASE_URL.xai_responses);
  });

  it("手で入れた URL は触らない", () => {
    expect(presetBaseUrlFor("gemini", "https://proxy.example.test/v1")).toBeNull();
  });

  // Grok を互換から Responses へ切り替える経路。ホストが同じなので
  // URL は動かないほうが正しい（差し替えの通知も要らない）。
  it("同じ URL への差し替えは起こさない", () => {
    expect(
      presetBaseUrlFor("xai_responses", DEFAULT_BASE_URL.xai_responses),
    ).toBeNull();
  });

  // ここが実機で落ちた。互換へ戻したときに api.x.ai を api.openai.com へ
  // 書き換えると、**xAI の鍵を持って OpenAI へ送る**設定が黙って出来上がり、
  // 401 Incorrect API key provided: xai-… になる（2026-08-10 実機）。
  it("互換の口も持つホストは、互換へ戻しても書き換えない", () => {
    expect(
      presetBaseUrlFor("open_ai_compat", DEFAULT_BASE_URL.xai_responses),
    ).toBeNull();
    // Gemini でも同じ（こちらは P2 以前からの既存バグだった）。
    expect(
      presetBaseUrlFor("open_ai_compat", DEFAULT_BASE_URL.gemini),
    ).toBeNull();
  });

  // ただし免除は互換側だけ。他社のネイティブへ移すときは従来どおり揃える。
  it("ネイティブへ切り替えるときは既定へ揃える", () => {
    expect(presetBaseUrlFor("anthropic", DEFAULT_BASE_URL.xai_responses)).toBe(
      DEFAULT_BASE_URL.anthropic,
    );
    expect(
      presetBaseUrlFor("xai_responses", DEFAULT_BASE_URL.open_ai_compat),
    ).toBe(DEFAULT_BASE_URL.xai_responses);
  });
});

describe("providerSkills", () => {
  it("xAI を明示選択したときだけ Grok の 2 つを出す", () => {
    const s = providerSkills(draft({ provider: "xai_responses" }));
    expect(s.xaiWeb.offered).toBe(true);
    expect(s.xaiX.offered).toBe(true);
    expect(s.google.offered).toBe(false);
    expect(s.anyOffered).toBe(true);
  });

  it("Gemini では Google だけを出す（固有スキルは混ざらない）", () => {
    const s = providerSkills(draft({ provider: "gemini" }));
    expect(s.google.offered).toBe(true);
    expect(s.xaiWeb.offered).toBe(false);
    expect(s.xaiX.offered).toBe(false);
  });

  it("互換のままでは 1 つも出ない", () => {
    expect(providerSkills(draft({ provider: "open_ai_compat" })).anyOffered).toBe(
      false,
    );
    expect(providerSkills(draft()).anyOffered).toBe(false);
  });

  // 効かない設定が真のまま残っている状態。隠すだけだと見えなくなるので、
  // その時だけ理由を出す（コアは AND 述語で無効化するが、画面が黙ると
  // 「チェックしたのに検索しない」の原因に辿り着けない）。
  it("ワイヤを戻すと、真のまま残ったスキルだけが stranded になる", () => {
    const s = providerSkills(
      draft({ provider: "open_ai_compat", xaiWebSearch: true, googleSearch: true }),
    );
    expect(s.xaiWeb.stranded).toBe(true);
    expect(s.google.stranded).toBe(true);
    // 元から偽のものは stranded ではない（警告を増やさない）。
    expect(s.xaiX.stranded).toBe(false);
  });

  it("自分のワイヤに乗っていれば stranded にはならない", () => {
    const s = providerSkills(draft({ provider: "xai_responses", xaiWebSearch: true }));
    expect(s.xaiWeb.stranded).toBe(false);
    expect(s.xaiWeb.offered).toBe(true);
  });

  // 2 つのトグルは独立（Spec 31 D3 — 1 つに畳むと web だけ欲しい村が
  // X の攻撃面まで開ける）。
  it("web と X は独立に効く", () => {
    const s = providerSkills(
      draft({ provider: "xai_responses", xaiWebSearch: true, xaiXSearch: false }),
    );
    expect(s.xaiWeb.offered && s.xaiX.offered).toBe(true);
    expect(s.xaiWeb.stranded).toBe(false);
    expect(s.xaiX.stranded).toBe(false);
  });
});

describe("passiveSkills", () => {
  // 載せるのは「このアプリがそのプロバイダ固有の機構を実際に使っている」ものだけ。
  // Anthropic だけが cache_control を組み立てている（place_message_breakpoint）。
  it("Anthropic はプロンプトキャッシュを持つ", () => {
    expect(passiveSkills("anthropic")).toEqual(["passivePromptCache"]);
  });

  // 互換 / Gemini / xAI は usage の cached_tokens を**読んでいるだけ**で、
  // こちらは何も送っていない。サーバー側の最適化をアプリの働きとして並べない。
  it("読んでいるだけのプロバイダには載せない", () => {
    for (const p of ["open_ai_compat", "gemini", "xai_responses", null] as const) {
      expect(passiveSkills(p)).toEqual([]);
    }
  });

  // 見出しは「操作できるか」ではなく「固有の能力があるか」で描く。
  it("パッシブだけの provider でも見出しは出る", () => {
    const s = providerSkills(draft({ provider: "anthropic" }));
    expect(s.anyOffered).toBe(true);
    expect(s.google.offered || s.xaiWeb.offered || s.xaiX.offered).toBe(false);
    expect(s.passive).toEqual(["passivePromptCache"]);
  });
});
