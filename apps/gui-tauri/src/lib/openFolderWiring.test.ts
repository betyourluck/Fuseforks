// @ts-expect-error @types/node を入れない方針のため（editorSaveWiring.test.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

/**
 * 「フォルダを開く」のパスの組み立てを機械で留める（2026-10-30 の監査 10）。
 *
 * 旧実装は `${workspace}\\agents\\${id}` と Windows の区切りを直書きしており、macOS / Linux では
 * 存在しないパスを開きに行った（3 OS で配っているのに、片方だけ壊れる形）。区切りは
 * `@tauri-apps/api/path` の `join` に任せる。
 */
const dialog = readFileSync(new URL("../components/AgentSettingsDialog.vue", import.meta.url), "utf8");

describe("フォルダを開くパスの配線", () => {
  it("区切り文字を直書きせず、tauri の join で組む", () => {
    expect(dialog).toContain('import { join } from "@tauri-apps/api/path";');
    expect(dialog).toContain('await openPath(await join(state.workspace, "agents", props.agentId));');
    expect(dialog).not.toContain("\\\\agents\\\\");
  });
});
