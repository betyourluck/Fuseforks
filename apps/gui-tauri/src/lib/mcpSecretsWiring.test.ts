/**
 * 「秘密の値」の置き場（2026-10-08 利用者）を走査で留める。
 *
 * 欄は共通 MCP ダイアログの中にあったが、一覧は共通と全サーヴァントの参照を並べるので
 * 「共通の分しか拾わないのでは」と読まれた。そこで欄を [`McpSecretsDialog`] へ出し、
 * 入口（鍵のアイコン）を 2 か所に置いた — 共通 MCP の mcp.json の行と、サーヴァントの
 * 編集ダイアログの mcp.json タブ（「AI で作成」と同じ位置）。**どれも型検査に掛からない**
 * （入口を消しても、欄を共通 MCP へ戻しても、ビルドは通る）。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

function component(name: string): string {
  return readFileSync(fileURLToPath(new URL(`../components/${name}.vue`, import.meta.url)), "utf8");
}

describe("秘密の値の置き場", () => {
  it("入口は共通 MCP とサーヴァントの mcp.json タブの 2 か所", () => {
    expect(component("McpDialog")).toContain("<McpSecretsButton");
    expect(component("MarkdownEditor")).toContain("<McpSecretsButton");
  });

  it("共通 MCP ダイアログに欄が戻っていない（一覧は別のダイアログだけが持つ）", () => {
    const dialog = component("McpDialog");
    expect(dialog).not.toContain("data-mcp-secret");
    expect(dialog).not.toContain("listMcpSecrets");
    expect(component("McpSecretsDialog")).toContain("data-mcp-secret");
  });

  it("サーヴァントの側は mcp.json のタブのときだけ出す", () => {
    const editor = component("MarkdownEditor");
    expect(editor).toMatch(/const canOpenSecrets = computed\(\(\) => kind\.value === "mcp"\);/);
    expect(editor).toMatch(/<McpSecretsButton v-if="canOpenSecrets"/);
  });

  it("ダイアログは開いた側の上に重なる覆いを持つ（z-40 のダイアログから開く）", () => {
    expect(component("McpSecretsDialog")).toContain(
      "fixed inset-0 z-50 flex items-center justify-center bg-scrim",
    );
  });

  it("鍵は SVG で描く（絵文字は配色に追従しない）", () => {
    const button = component("McpSecretsButton");
    expect(button).toContain("<svg");
    expect(button).not.toMatch(/🔑/u);
  });
});
