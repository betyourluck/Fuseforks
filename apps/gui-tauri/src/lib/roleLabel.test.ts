import { describe, expect, it } from "vitest";

import { roleLabel } from "./roleLabel";
import type { Role } from "../types";

function role(id: string, name: string): Role {
  return {
    id,
    name,
    description: "",
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
