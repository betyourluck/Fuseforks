import { describe, expect, it } from "vitest";

import { filterAgents, matchesAgent, searchTokens, type SearchableAgent } from "./agentSearch";
import type { Role } from "../types";

function role(id: string, name: string): Role {
  return {
    id,
    name,
    description: "",
    color: null,
    defaults: {
      construct: "",
      modelTemplateId: null,
      enabledTools: null,
      maxToolIterations: null,
    },
  };
}

const ROLES = [role("researcher", "調査役"), role("lead", "進行役")];

const ZARI: SearchableAgent = { id: "agent", name: "ザリ・ロブステル", model: "claude-opus-5-5", roleId: "lead" };
const GEMMY: SearchableAgent = { id: "agent_3", name: "ジェミー", model: "gemini-3.8-flash", roleId: "researcher" };
const LUNA: SearchableAgent = { id: "agent_8", name: "ルナ", model: "gpt-6.1-sol", roleId: null };
const ALL = [ZARI, GEMMY, LUNA];

describe("searchTokens", () => {
  it("空白で割り、空の語を落とす（空白だけなら語 0 個）", () => {
    expect(searchTokens("  gpt   調査 ")).toEqual(["gpt", "調査"]);
    expect(searchTokens("   ")).toEqual([]);
  });

  it("大文字小文字と全角半角を畳む", () => {
    expect(searchTokens("ＧＰＴ Opus")).toEqual(["gpt", "opus"]);
  });
});

describe("matchesAgent", () => {
  it("表示名・id・モデル名・役職名のどれかに部分一致すれば当たり", () => {
    expect(matchesAgent(ZARI, ROLES, ["ロブ"])).toBe(true);
    expect(matchesAgent(GEMMY, ROLES, ["agent_3"])).toBe(true);
    expect(matchesAgent(LUNA, ROLES, ["sol"])).toBe(true);
    expect(matchesAgent(GEMMY, ROLES, ["調査"])).toBe(true);
  });

  it("語は全部が当たる個体だけ（AND）", () => {
    expect(matchesAgent(GEMMY, ROLES, ["gemini", "調査"])).toBe(true);
    expect(matchesAgent(ZARI, ROLES, ["gemini", "調査"])).toBe(false);
  });

  it("引けない役職の名前では当てない（画面にバッジが出ていない語）", () => {
    const orphan: SearchableAgent = { ...LUNA, roleId: "消えた役職" };
    expect(matchesAgent(orphan, ROLES, ["消えた"])).toBe(false);
  });
});

describe("filterAgents", () => {
  it("問いが空なら全員、当たった個体は渡された順のまま", () => {
    expect(filterAgents(ALL, ROLES, "")).toEqual(ALL);
    expect(filterAgents(ALL, ROLES, "AGENT_")).toEqual([GEMMY, LUNA]);
  });

  it("全角の問いも半角の名前に当たる", () => {
    expect(filterAgents(ALL, ROLES, "ｇｅｍｉｎｉ")).toEqual([GEMMY]);
  });
});
