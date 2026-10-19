import { describe, expect, it } from "vitest";
import { unsavedSummary } from "./agentSettingsUnsaved";

describe("unsavedSummary", () => {
  it("どちらも保存済みなら null（確認を出さない）", () => {
    expect(unsavedSummary(false, null)).toBeNull();
  });

  it("左の設定だけが未保存", () => {
    const summary = unsavedSummary(true, null);
    expect(summary?.messageKey).toBe("agentSettings.discardForm");
    expect(summary?.noteKey).toBe("agentSettings.unsaved");
  });

  it("右のファイルだけが未保存でも確認を出し、ファイル名を名指しする", () => {
    // 2026-10-19 まではこの形で確認が出ず、閉じると編集が消えていた。
    const summary = unsavedSummary(false, "skill");
    expect(summary?.messageKey).toBe("agentSettings.discardFile");
    expect(summary?.noteKey).toBe("agentSettings.unsavedFile");
    expect(summary?.params.file).toBe("SKILL.md");
  });

  it("両方が未保存なら両方を名指しする", () => {
    const summary = unsavedSummary(true, "mcp");
    expect(summary?.messageKey).toBe("agentSettings.discardBoth");
    expect(summary?.noteKey).toBe("agentSettings.unsavedBoth");
    expect(summary?.params.file).toBe("mcp.json");
  });
});
