/**
 * 起動と終了の画面側の**配線**を機械で留める（Spec 69 P2。`launch_exit_contract` 2・4）。
 *
 * 判定は純関数（`hidesOnClose` / `startupSettings.ts`）の単体が守る。ここで留めるのは純関数の外 —
 * 閉じる経路が隠す判定を確認より先に通ること / **窓を隠すのは App.vue の 1 箇所だけ** /
 * トレイの「終了」のイベント名が Rust の `tray.rs` と同じ文字列であること（ずれると「終了」を押しても
 * 何も起きない = 静かに壊れる）/ 起動時に設定を読むこと / 設定ページの置き場と読み直し /
 * IPC の名前が Rust の登録に在ること。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync, readdirSync, statSync } from "node:fs";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

function read(rel: string): string {
  return readFileSync(fileURLToPath(new URL(rel, import.meta.url)), "utf8");
}

/** `//` と `/* *\/` のコメントを落とす（説明文に名前を書くのは自由）。 */
function code(source: string): string {
  return source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");
}

const app = read("../App.vue");
const closeConfirm = read("./closeConfirm.ts");
const orchestrator = read("../composables/useOrchestrator.ts");
const dialog = read("../components/SettingsDialog.vue");
const ipc = read("./ipc.ts");
const trayRs = read("../../src-tauri/src/tray.rs");
const libRs = read("../../src-tauri/src/lib.rs");
const capability = read("../../src-tauri/capabilities/default.json");

function between(source: string, start: string, end: string): string {
  const from = source.indexOf(start);
  expect(from, `${start} が無い`).toBeGreaterThanOrEqual(0);
  const to = source.indexOf(end, from + start.length);
  return source.slice(from, to === -1 ? undefined : to);
}

describe("閉じる経路（Spec 69 D2）", () => {
  it("✕ は隠す判定を確認より先に通り、隠すときは確認を出さずに return する", () => {
    const guard = between(code(app), "async function registerCloseGuard()", "async function onTrayQuit()");
    const hides = guard.indexOf("hidesOnClose(");
    const hide = guard.indexOf("win.hide()");
    const confirm = guard.indexOf("settings.confirmClose");
    expect(hides).toBeGreaterThanOrEqual(0);
    expect(hide).toBeGreaterThan(hides);
    expect(confirm).toBeGreaterThan(hide);
    expect(guard.slice(hide, confirm)).toContain("return;");
    // 組み立て前は隠さない — 判定へ ready と initError を渡している。
    expect(guard).toContain("ready: state.ready");
    expect(guard).toContain("failed: state.initError !== null");
  });

  it("窓を隠すのは App.vue の 1 箇所だけ（画面の src 全体で .hide( は 1 つ）", () => {
    const root = fileURLToPath(new URL("..", import.meta.url));
    const hits: string[] = [];
    const walk = (dir: string) => {
      for (const name of readdirSync(dir)) {
        const path = `${dir}/${name}`;
        if (statSync(path).isDirectory()) {
          walk(path);
        } else if (/\.(vue|ts)$/.test(name) && !/\.test\.ts$/.test(name)) {
          const count = code(readFileSync(path, "utf8")).split(".hide(").length - 1;
          for (let i = 0; i < count; i += 1) hits.push(path);
        }
      }
    };
    walk(root);
    expect(hits).toHaveLength(1);
    expect(hits[0]).toMatch(/App\.vue$/);
  });

  it("トレイの「終了」は Rust と同じイベント名を聴き、確認を出している間は 2 回目を積まない", () => {
    const eventName = /export const TRAY_QUIT_EVENT = "([^"]+)"/.exec(closeConfirm)?.[1];
    const rustName = /pub const QUIT_EVENT: &str = "([^"]+)"/.exec(trayRs)?.[1];
    expect(eventName).toBeDefined();
    expect(eventName).toBe(rustName);
    expect(code(app)).toContain("listen(TRAY_QUIT_EVENT");

    const quit = between(code(app), "async function onTrayQuit()", "let unlistenTrayQuit");
    expect(quit.indexOf("if (exitConfirmInFlight) return;")).toBeGreaterThanOrEqual(0);
    expect(quit.indexOf("exitConfirmInFlight = true")).toBeLessThan(quit.indexOf("confirmExit()"));
    expect(quit).toContain("finally");
    expect(quit).toContain("exitConfirmInFlight = false");
  });

  it("capability に窓を隠す権限がある", () => {
    expect(JSON.parse(capability).permissions).toContain("core:window:allow-hide");
  });
});

describe("起動時の読み込みと設定ページ（Spec 69 D1・D7）", () => {
  it("initialize() が起動と終了の設定を state.startup へ読む", () => {
    expect(code(orchestrator)).toContain("state.startup = await ipc.getStartupSettings()");
  });

  it("起動と終了は村のページではなく、開くたびに 2 本とも読み直す", () => {
    const village = /const VILLAGE_PAGES: Page\[\] = \[([^\]]*)\]/.exec(dialog)?.[1] ?? "";
    expect(village).not.toContain("startup");
    expect(code(dialog)).toContain('if (next === "startup") void loadStartup();');
    const load = between(code(dialog), "async function loadStartup()", "async function applyStartup(");
    expect(load).toContain("ipc.getStartupSettings()");
    expect(load).toContain("ipc.getAutostart()");
  });

  it("終了する前の確認のチェックは「起動と終了」のページにだけある", () => {
    const occurrences = dialog.split('v-model="settings.confirmClose"').length - 1;
    expect(occurrences).toBe(1);
    const startupPage = between(dialog, "<template v-else-if=\"page === 'startup'\">", "</template>");
    expect(startupPage).toContain('v-model="settings.confirmClose"');
  });

  it("画面が呼ぶ 4 本の IPC は Rust の generate_handler に登録されている", () => {
    for (const command of ["get_startup_settings", "set_startup_settings", "get_autostart", "set_autostart"]) {
      expect(ipc).toContain(`"${command}"`);
      expect(libRs).toContain(`commands::${command},`);
    }
  });
});
