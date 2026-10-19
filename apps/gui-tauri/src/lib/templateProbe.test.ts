import { describe, expect, it } from "vitest";

import { isTemplateDirty } from "./templateDraft";
import { probeBlockedKey, probeFinishKey, probeView } from "./templateProbe";
import type { ModelTemplate, TemplateProbe } from "../types";

function template(overrides: Partial<ModelTemplate> = {}): ModelTemplate {
  return {
    id: "tpl",
    name: "既定",
    baseUrl: "https://api.example.com/v1",
    model: "m",
    contextLength: 128000,
    temperature: null,
    maxOutputTokens: 8192,
    credential: "keyring",
    provider: null,
    useTools: true,
    effort: null,
    googleSearch: false,
    xaiWebSearch: false,
    xaiXSearch: false,
    openaiWebSearch: false,
    openaiReasoningPro: false,
    metaWebSearch: false,
    ...overrides,
  } as ModelTemplate;
}

describe("isTemplateDirty（Spec 68 D1）", () => {
  it("新規の下書きは未保存", () => {
    expect(isTemplateDirty(template(), null)).toBe(true);
  });

  it("保存済みと同じなら未保存ではない", () => {
    expect(isTemplateDirty(template(), template())).toBe(false);
  });

  it("欄が 1 つでも違えば未保存（思考段階・温度・モデル名）", () => {
    expect(isTemplateDirty(template({ effort: "high" }), template())).toBe(true);
    expect(isTemplateDirty(template({ temperature: 0.2 }), template())).toBe(true);
    expect(isTemplateDirty(template({ model: "m2" }), template())).toBe(true);
  });

  it("欄が無いことと null は同じに扱う（後から足した欄で未保存と読まない）", () => {
    const saved = template() as unknown as Record<string, unknown>;
    const draft = { ...saved, inputPerMtok: null } as unknown as ModelTemplate;
    expect(isTemplateDirty(draft, saved as unknown as ModelTemplate)).toBe(false);
  });
});

describe("probeBlockedKey（Spec 68 D8）", () => {
  it("保存済みで鍵の入力欄が空なら押せる", () => {
    expect(probeBlockedKey({ dirty: false, secretTyped: false, busy: false })).toBeNull();
  });

  it("未保存なら保存してから", () => {
    expect(probeBlockedKey({ dirty: true, secretTyped: false, busy: false })).toBe("modelTemplate.probe.saveFirst");
  });

  it("鍵の入力欄に文字があれば登録してから（rev2 点 1）", () => {
    expect(probeBlockedKey({ dirty: false, secretTyped: true, busy: false })).toBe(
      "modelTemplate.probe.registerKeyFirst",
    );
  });

  it("確かめている最中は押せない", () => {
    expect(probeBlockedKey({ dirty: false, secretTyped: false, busy: true })).toBe("modelTemplate.probe.busy");
  });
});

const usage = { prompt: 14, completion: 2, reasoning: 0 };

describe("probeView（Spec 68 D2・D6）", () => {
  it("応答した → run で本文と秒数", () => {
    const probe: TemplateProbe = { result: "reached", elapsedMs: 1400, outcome: "replied", text: "OK", usage };
    expect(probeView(probe)).toEqual({
      tone: "run",
      key: "modelTemplate.probe.reached.replied",
      params: { text: "OK", seconds: 1.4 },
      usage,
      retryAfterSecs: null,
    });
  });

  it("出力上限で切れた → 成功だが warn の注記で上限を出す", () => {
    const probe: TemplateProbe = { result: "reached", elapsedMs: 900, outcome: "truncated", limit: 16, usage };
    const view = probeView(probe);
    expect(view.tone).toBe("warn");
    expect(view.key).toBe("modelTemplate.probe.reached.truncated");
    expect(view.params.limit).toBe(16);
  });

  it("本文が無かった → warn で終了理由の鍵を引ける", () => {
    const probe: TemplateProbe = { result: "reached", elapsedMs: 900, outcome: "no_text", finish: "other", usage };
    expect(probeView(probe).tone).toBe("warn");
    expect(probeFinishKey(probe)).toBe("modelTemplate.probe.finish.other");
  });

  it("失敗 → fail で分類の鍵・status・待ち時間を運ぶ（使用量は無い）", () => {
    const probe: TemplateProbe = {
      result: "failed",
      failure: "rate_limit",
      status: 429,
      retryAfterSecs: 120,
      error: {
        code: "LLM_API",
        message: "API エラー (status=429): slow down",
        detail: null,
        agentId: null,
        retryable: true,
        retryAfterSecs: 120,
        imagesRejected: false,
      },
      elapsedMs: 300,
    };
    const view = probeView(probe);
    expect(view.tone).toBe("fail");
    expect(view.key).toBe("modelTemplate.probe.failure.rate_limit");
    expect(view.params.status).toBe(429);
    expect(view.retryAfterSecs).toBe(120);
    expect(view.usage).toBeNull();
    expect(probeFinishKey(probe)).toBeNull();
  });
});
