// @ts-expect-error @types/node を入れない方針のため（editorSaveWiring.test.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

/**
 * 発話の中のファイルの位置を開く（2026-11-08）の配線。
 *
 * 形の判定は `messagePath.ts`、解決と起動はコア（`src-tauri/src/editor.rs`）が持ち、それぞれ単体で
 * 留めている。ここで確かめるのは、**開く経路が 2 つ（案内 / 外部エディタ）に閉じていること**と、
 * 押した側が正しい起点（作業フォルダ）を渡していること。
 */
const read = (path: string): string => readFileSync(new URL(path, import.meta.url), "utf8");
const chat = read("../components/ChatPanel.vue");
const blackboard = read("../components/BlackboardPane.vue");
const host = read("../components/PathMenuHost.vue");
const app = read("../App.vue");
const editorRs = read("../../src-tauri/src/editor.rs");
const commandsRs = read("../../src-tauri/src/commands.rs");

/** 関数の本体を、次の関数の手前まで切り出す。 */
function body(source: string, name: string): string {
  const start = source.indexOf(`function ${name}(`);
  expect(start, `${name} が無い`).toBeGreaterThan(-1);
  const next = source.indexOf("\nfunction ", start + 1);
  return source.slice(start, next === -1 ? undefined : next);
}

describe("押した側", () => {
  it("会話ペインはパスの印をリンクより先に受け、発言した個体の作業フォルダを渡す", () => {
    const handler = body(chat, "onMarkdownClick");
    const path = handler.indexOf('closest<HTMLElement>("[data-path]")');
    expect(path).toBeGreaterThan(-1);
    expect(handler.indexOf('closest("a")')).toBeGreaterThan(path);
    expect(handler).toContain("pathMenu.open(event, pathMark.dataset.path ?? \"\", workDirOfSender(message));");
    expect(chat).toContain('@click="onMarkdownClick($event, entry.row.message)"');
  });

  it("黒板は付箋の作業フォルダ（dir）を渡す", () => {
    const handler = body(blackboard, "onMarkdownClick");
    expect(handler).toContain('pathMenu.open(event, pathMark.dataset.path ?? "", note.dir);');
    expect(handler.indexOf('closest("a")')).toBeGreaterThan(handler.indexOf("[data-path]"));
  });
});

describe("メニュー", () => {
  it("開く経路は案内（revealItemInDir）と外部エディタ（open_in_editor）の 2 つだけ", () => {
    expect(host).toContain("await revealItemInDir(resolved.path);");
    expect(host).toContain("await ipc.openInEditor(resolved.path, state.ref?.line ?? null);");
    // OS の既定のアプリで開く経路を持たない（実行ファイルなら実行される）。
    expect(host).not.toMatch(/openPath|openUrl/);
  });

  it("メニューは全画面の層の側（グリッドの外）に 1 つだけ置かれる", () => {
    expect(app.split("<PathMenuHost").length - 1).toBe(1);
    expect(app.indexOf("<PathMenuHost")).toBeGreaterThan(app.indexOf("<ToastHost"));
  });
});

describe("コア側", () => {
  it("open_in_editor は起動するコマンドを画面から受け取らない（棚から読む）", () => {
    const start = commandsRs.indexOf("pub fn open_in_editor(");
    expect(start).toBeGreaterThan(-1);
    const signature = commandsRs.slice(start, commandsRs.indexOf(") -> ", start));
    expect(signature).toContain("path: String");
    expect(signature).toContain("line: Option<u32>");
    expect(signature).not.toMatch(/command|args/);
  });

  it("エディタはシェルを通さず、解決した実行ファイルを直接起動する", () => {
    const start = editorRs.indexOf("pub fn open_in_editor(");
    const fn = editorRs.slice(start, editorRs.indexOf("\n}\n", start));
    expect(fn).toContain("std::process::Command::new(&program)");
    expect(fn).toContain("resolve_program(command)");
    expect(fn).not.toMatch(/"cmd"|"sh"|"\/C"|"-c"|open_path|opener/);
  });
});
