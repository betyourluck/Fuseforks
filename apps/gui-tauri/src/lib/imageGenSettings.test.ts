import { describe, expect, it } from "vitest";

import type { ImageGenSettingsView } from "../types";
import {
  buildConfig,
  inputsFromSlot,
  parseTimeoutInput,
  probeBlockKey,
  probeMessageKey,
  shownProvider,
  slotDirty,
} from "./imageGenSettings";

/** 4 社の行を持つ画面の状態（Rust の `view` が返す形）。 */
function view(over: Partial<ImageGenSettingsView> = {}): ImageGenSettingsView {
  const row = (id: "openai" | "gemini" | "meta" | "xai", model = "") => ({
    id,
    slot: { model, baseUrl: "", timeoutSecs: null },
    hasKey: false,
    defaultModel: `${id}-default`,
    defaultBaseUrl: `https://${id}.example/v1`,
    defaultTimeoutSecs: 120,
    honorsResolution: id !== "meta",
  });
  return {
    provider: null,
    aspect: "square",
    resolution: "standard",
    providers: [row("openai", "gpt-x"), row("gemini"), row("meta"), row("xai")],
    active: false,
    inactiveReason: "no_provider",
    blocked: null,
    ...over,
  };
}

describe("タイムアウトの入力", () => {
  it("空・数でない・範囲外は既定（null）", () => {
    for (const raw of ["", "  ", "abc", "0", "1.5", "-3", "3601", "1e3"]) {
      expect(parseTimeoutInput(raw), raw).toBeNull();
    }
  });

  it("1〜3600 の整数は値（前後の空白は落とす）", () => {
    expect(parseTimeoutInput("5")).toBe(5);
    expect(parseTimeoutInput(" 30 ")).toBe(30);
    expect(parseTimeoutInput("3600")).toBe(3600);
  });
});

describe("保存する設定を組む", () => {
  it("4 社の枠を全部送り、表示している接続先の枠だけ入力で置き換える", () => {
    const v = view({ provider: "xai" });
    const config = buildConfig(v, { model: " grok-y ", baseUrl: "", timeout: "45" });
    expect(Object.keys(config.perProvider).sort()).toEqual(["gemini", "meta", "openai", "xai"]);
    expect(config.perProvider.xai).toEqual({ model: "grok-y", baseUrl: "", timeoutSecs: 45 });
    // 表示していない枠は保存値のまま（送らないと Rust 側で消える）。
    expect(config.perProvider.openai?.model).toBe("gpt-x");
    expect(config.provider).toBe("xai");
  });

  it("接続先を「使わない」へ切り替えられる（null と undefined を取り違えない）", () => {
    const v = view({ provider: "gemini" });
    expect(buildConfig(v, inputsFromSlot(undefined), { provider: null }).provider).toBeNull();
    expect(buildConfig(v, inputsFromSlot(undefined)).provider).toBe("gemini");
  });

  it("切り替えの保存は、切り替える前に表示していた枠へ入力を書く", () => {
    const v = view({ provider: "openai" });
    const config = buildConfig(v, { model: "gpt-z", baseUrl: "", timeout: "" }, { provider: "meta" });
    expect(config.provider).toBe("meta");
    expect(config.perProvider.openai?.model).toBe("gpt-z");
    expect(config.perProvider.meta?.model).toBe("");
  });

  it("縦横比と解像度は指定したものだけ変わる", () => {
    const v = view({ aspect: "portrait", resolution: "high" });
    const config = buildConfig(v, inputsFromSlot(undefined), { resolution: "highest" });
    expect(config.aspect).toBe("portrait");
    expect(config.resolution).toBe("highest");
  });

  it("接続先が未選択なら、どの枠も入力で置き換えない", () => {
    const config = buildConfig(view(), { model: "typed", baseUrl: "", timeout: "" });
    expect(Object.values(config.perProvider).map((s) => s?.model)).not.toContain("typed");
  });
});

describe("未保存と「接続を確かめる」の門", () => {
  it("空白だけの違いは未保存にしない", () => {
    const v = view({ provider: "openai" });
    expect(slotDirty(v, { model: "  gpt-x ", baseUrl: "", timeout: "" })).toBe(false);
    expect(slotDirty(v, { model: "gpt-y", baseUrl: "", timeout: "" })).toBe(true);
    expect(slotDirty(v, { model: "gpt-x", baseUrl: "", timeout: "120" })).toBe(true);
  });

  it("押せない理由は 接続先 → 鍵 → 未保存 の順", () => {
    const clean = inputsFromSlot(undefined);
    expect(probeBlockKey(view(), clean)).toBe("settings.imageGen.probeNeedsProvider");
    const selected = view({ provider: "gemini" });
    expect(probeBlockKey(selected, clean)).toBe("settings.imageGen.probeNeedsKey");
    const withKey = view({
      provider: "gemini",
      providers: view().providers.map((p) => (p.id === "gemini" ? { ...p, hasKey: true } : p)),
    });
    expect(shownProvider(withKey)?.hasKey).toBe(true);
    expect(probeBlockKey(withKey, { ...clean, model: "gemini-z" })).toBe(
      "settings.imageGen.probeNeedsSave",
    );
    expect(probeBlockKey(withKey, clean)).toBeNull();
  });

  it("知らない結末の語は unknown の鍵へ（鍵名を画面に出さない）", () => {
    expect(probeMessageKey("ok")).toBe("settings.imageGen.probe.ok");
    expect(probeMessageKey("rate_limited")).toBe("settings.imageGen.probe.rate_limited");
    expect(probeMessageKey("interrupted")).toBe("settings.imageGen.probe.unknown");
  });
});
