/**
 * リモート MCP の OAuth の入口（Spec 67 P2）を走査で留める。
 *
 * どれも型検査に掛からない — 入口を片方のダイアログから消しても、IPC の登録を忘れても、
 * 部品が開いた瞬間に認可を始める形に変えても、ビルドは通る。特に最後は D6（ブラウザは人が
 * 「認可する」を押したときだけ開く）を破る形で、画面を開いただけで外部のログイン画面が出る。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

function read(relative: string): string {
  return readFileSync(fileURLToPath(new URL(relative, import.meta.url)), "utf8");
}

const component = (name: string) => read(`../components/${name}.vue`);

describe("OAuth の入口", () => {
  it("共通 MCP とサーヴァントの MCP の状態の行の 2 か所に置く", () => {
    expect(component("McpDialog")).toContain("<McpOAuthControls");
    expect(component("AgentSettingsDialog")).toContain("<McpOAuthControls");
  });

  it("IPC の 3 本が Rust 側に登録されている", () => {
    const handlers = read("../../src-tauri/src/lib.rs");
    for (const command of ["mcp_oauth_authorize", "mcp_oauth_cancel", "mcp_oauth_revoke"]) {
      expect(handlers).toContain(`commands::${command},`);
      expect(read("./ipc.ts")).toContain(`"${command}"`);
    }
  });

  it("部品は開いただけでは認可を始めない（ブラウザはボタンを押したときだけ — D6）", () => {
    const controls = component("McpOAuthControls");
    expect(controls).not.toContain("onMounted");
    expect(controls).not.toMatch(/watch\(/);
    // 認可の呼び出しはボタンの関数の中の 1 か所だけ。
    expect(controls.match(/ipc\.mcpOAuthAuthorize\(/g)?.length).toBe(1);
    expect(controls).toMatch(/@click="authorize"/);
  });

  it("OAuth でないエントリには何も描かない", () => {
    const controls = component("McpOAuthControls");
    expect(controls).toMatch(/props\.status\.auth !== "none" && !!props\.status\.url/);
    expect(controls).toMatch(/<div v-if="isOAuth"/);
  });
});
