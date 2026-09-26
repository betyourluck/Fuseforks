// @ts-expect-error @types/node を入れない方針のため（editorSaveWiring.test.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import en from "../locales/en.json";
import ja from "../locales/ja.json";
import { nextRunApproval, RUN_APPROVAL_ORDER, runApprovalLabelKey } from "./runApproval";

describe("runApproval: クリックで巡るモード", () => {
  it("承認あり → 自動許可＋承認 → 自動許可 → 承認あり の順に巡る", () => {
    expect(nextRunApproval("required")).toBe("auto_approve");
    expect(nextRunApproval("auto_approve")).toBe("no_approval");
    expect(nextRunApproval("no_approval")).toBe("required");
  });

  it("知らない値は既定へ戻す", () => {
    expect(nextRunApproval("everything" as never)).toBe("required");
  });

  it("3 つのモードの字が ja / en の辞書にある", () => {
    for (const mode of RUN_APPROVAL_ORDER) {
      const [, key] = runApprovalLabelKey(mode).split(".");
      expect((ja.statusBar as Record<string, string>)[key]).toBeTruthy();
      expect((en.statusBar as Record<string, string>)[key]).toBeTruthy();
    }
  });

  it("帯はコンボリストではなくボタンで、押すと次のモードへ進む", () => {
    const bar = readFileSync(new URL("../components/StatusBar.vue", import.meta.url), "utf8");
    expect(bar).not.toMatch(/<select/);
    expect(bar).toMatch(/nextRunApproval\(state\.runApproval\)/);
  });
});
