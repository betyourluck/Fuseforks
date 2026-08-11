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
  | "provider"
  | "googleSearch"
  | "xaiWebSearch"
  | "xaiXSearch"
  | "openaiWebSearch"
  | "openaiReasoningPro"
>;

function draft(over: Partial<Draft> = {}): Draft {
  return {
    provider: null,
    googleSearch: false,
    xaiWebSearch: false,
    xaiXSearch: false,
    openaiWebSearch: false,
    openaiReasoningPro: false,
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

/**
 * **既定 URL の表が非単射になる 7 通り**（Spec 34 D7）。
 *
 * `open_ai_compat` と `open_ai_responses` が**同じ文字列**
 * （`https://api.openai.com/v1`）を指す。gemini / xai_responses は
 * 「互換の口も持つ他社ホスト」で**鍵が別**だったが、ここは**値が一致する**。
 *
 * 机上では 3 述語とも壊れないと追えるが、**#91 は机上で追って外した事故**
 * （集合へ要素を足したら、その集合を否定に使っている述語が黙って厳しくなった）。
 * だから 7 通りを機械で置く。**行 4・5・7 は rev1 の表に無かった** —
 * 特に行 5（人が入れたプロキシ URL）は「触らない」が期待値なのに、
 * どこにも書かれていなかった。
 */
describe("既定 URL が非単射になる 7 通り（Spec 34 D7）", () => {
  const OPENAI = DEFAULT_BASE_URL.open_ai_compat;

  it("1: 互換 + api.openai.com は警告なし（既存の主経路）", () => {
    expect(baseUrlMismatch("open_ai_compat", OPENAI)).toBeNull();
  });

  it("2: Responses + api.openai.com も警告なし", () => {
    expect(DEFAULT_BASE_URL.open_ai_responses).toBe(OPENAI);
    expect(baseUrlMismatch("open_ai_responses", OPENAI)).toBeNull();
  });

  it("3: 互換 → Responses で URL が動かない", () => {
    expect(presetBaseUrlFor("open_ai_responses", OPENAI)).toBeNull();
  });

  it("4: Responses → 互換でも URL が動かない", () => {
    expect(presetBaseUrlFor("open_ai_compat", OPENAI)).toBeNull();
  });

  it("5: カスタム URL（プロキシ）は触らない", () => {
    const proxy = "https://proxy.example.test/v1";
    expect(baseUrlMismatch("open_ai_responses", proxy)).toBeNull();
    expect(presetBaseUrlFor("open_ai_responses", proxy)).toBeNull();
  });

  it("6: anthropic + api.openai.com は従来どおり警告", () => {
    expect(baseUrlMismatch("anthropic", OPENAI)).toBe(DEFAULT_BASE_URL.anthropic);
  });

  // **登録しないとここが壊れる。** preset が undefined になって null が返り、
  // base URL が api.anthropic.com のまま残る（presetBaseUrlFor の doc が
  // 名指しする元のバグの鏡像）。「同じ文字列を 2 回登録する意味が無い」は誤り。
  it("7: anthropic → Responses は api.openai.com へ書き換わる", () => {
    expect(presetBaseUrlFor("open_ai_responses", DEFAULT_BASE_URL.anthropic)).toBe(
      OPENAI,
    );
  });
});

describe("OpenAI Responses の固有スキル（Spec 34 D4 / D5）", () => {
  it("Responses を選んだときだけ 2 つとも出る", () => {
    const s = providerSkills(
      draft({
        provider: "open_ai_responses",
        openaiWebSearch: true,
        openaiReasoningPro: true,
      }),
    );
    expect(s.openaiWeb.offered && s.openaiPro.offered).toBe(true);
    expect(s.openaiWeb.stranded || s.openaiPro.stranded).toBe(false);
    // xAI / Gemini の行は出ない（provider で分けており、モデル名では分けない）。
    expect(s.xaiWeb.offered || s.xaiX.offered || s.google.offered).toBe(false);
  });

  // UI からは作れないが world.json の直接編集で作れる組。**隠すだけだと
  // 真のまま見えなくなる**ので、そのときだけ理由と直し方を出す。
  it("互換のままトグルが真なら stranded", () => {
    const s = providerSkills(
      draft({
        provider: "open_ai_compat",
        openaiWebSearch: true,
        openaiReasoningPro: true,
      }),
    );
    expect(s.openaiWeb.offered || s.openaiPro.offered).toBe(false);
    expect(s.openaiWeb.stranded && s.openaiPro.stranded).toBe(true);
  });

  // **2 つは独立。** 片方だけ真にしても、もう片方は stranded にならない。
  it("web 検索と Pro モードは独立", () => {
    const s = providerSkills(
      draft({ provider: "open_ai_compat", openaiReasoningPro: true }),
    );
    expect(s.openaiPro.stranded).toBe(true);
    expect(s.openaiWeb.stranded).toBe(false);
  });

  // 見出しは Responses でも描く（固有の能力があるので）。
  it("Responses では見出しが出る", () => {
    expect(providerSkills(draft({ provider: "open_ai_responses" })).anyOffered).toBe(
      true,
    );
  });
});
