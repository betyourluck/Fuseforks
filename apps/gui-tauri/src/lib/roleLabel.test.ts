import { describe, expect, it } from "vitest";

import { roleBadge, roleLabel } from "./roleLabel";
import type { Role } from "../types";

function role(id: string, name: string, color: Role["color"] = null): Role {
  return {
    id,
    name,
    description: "",
    color,
    defaults: {
      construct: "",
      modelTemplateId: null,
      ragSources: [],
      enabledTools: null,
      maxToolIterations: null,
    },
  };
}

const ROLES = [role("researcher", "調査役"), role("reviewer", "査読役")];

describe("roleLabel", () => {
  it("id から表示名を引く", () => {
    expect(roleLabel("researcher", ROLES)).toBe("調査役");
  });

  it("役職なし（null）は null", () => {
    expect(roleLabel(null, ROLES)).toBeNull();
  });

  it("**引けない id は null**。`[不明]` のような代替表示を作らない", () => {
    // 存在しない役を出しても判断材料にならず、顔ぶれでは毎ターンぶんの
    // トークンを払うだけになる（role_contract 凍結 5）。
    expect(roleLabel("消えた役職", ROLES)).toBeNull();
  });

  it("役職が 1 つも無い村でも落ちない", () => {
    expect(roleLabel("researcher", [])).toBeNull();
  });

  it("改名は表示に追従する（名前は参照であってコピーではない）", () => {
    const renamed = [role("researcher", "コード調査役")];
    expect(roleLabel("researcher", renamed)).toBe("コード調査役");
  });
});

describe("roleBadge（色つき）", () => {
  it("色なしの役職では color が付かない（既定の枠線・字色をそのまま使う）", () => {
    const badge = roleBadge("researcher", ROLES);
    expect(badge).toEqual({ name: "調査役", color: undefined });
  });

  it("色ありの役職は CSS 変数を返す。**生の色値を組み立てない**", () => {
    // 配色は style.css の @theme にしか無い、という規律をここで留める。
    const colored = [role("reviewer", "査読役", "teal")];
    expect(roleBadge("reviewer", colored)).toEqual({
      name: "査読役",
      color: "var(--color-role-teal)",
    });
  });

  it("引けない id は null（色の有無に関係なく表示ごと省く）", () => {
    expect(roleBadge("消えた役職", ROLES)).toBeNull();
    expect(roleBadge(null, ROLES)).toBeNull();
  });

  it("roleLabel は roleBadge の名前と一致する（2 実装に割れない）", () => {
    const colored = [role("reviewer", "査読役", "pink")];
    expect(roleLabel("reviewer", colored)).toBe(roleBadge("reviewer", colored)!.name);
  });
});
