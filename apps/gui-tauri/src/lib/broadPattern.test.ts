import { describe, expect, it } from "vitest";

import { broadAllowEntries, broadPatternKind } from "./broadPattern";

describe("broadPatternKind", () => {
  it("実際の村にあった 2 つを名指しする（2026-11-11）", () => {
    expect(broadPatternKind("bash *")).toBe("anyCommand");
    expect(broadPatternKind("ssh outcasts *")).toBe("remote");
  });

  it("実際の村の普通のパターンは広くない", () => {
    for (const pattern of ["git log *", "git status *", "git commit *", "rg *", "dyff betweeen *", "curl -sI *"]) {
      expect(broadPatternKind(pattern), pattern).toBeNull();
    }
  });

  it("承認画面の「広いほう」が作るシェルとインタプリタの形", () => {
    expect(broadPatternKind("pwsh -Command *")).toBe("anyCommand");
    expect(broadPatternKind("pwsh -NoProfile *")).toBe("anyCommand");
    expect(broadPatternKind("cmd /c *")).toBe("anyCommand");
    expect(broadPatternKind("python -c *")).toBe("anyCode");
    expect(broadPatternKind("node -e *")).toBe("anyCode");
    expect(broadPatternKind("python -m *")).toBe("anyCode");
  });

  it("インタプリタは固定した引数がコードのフラグか、何も固定していないときだけ", () => {
    expect(broadPatternKind("python *")).toBe("anyCode");
    expect(broadPatternKind("python scripts/check.py *")).toBeNull();
    expect(broadPatternKind("node build.js *")).toBeNull();
  });

  it("git は何も固定しないか -c を固定したときだけ（alias と core.sshCommand で起動できる）", () => {
    expect(broadPatternKind("git *")).toBe("anyCommand");
    expect(broadPatternKind("git -c *")).toBe("anyCommand");
    expect(broadPatternKind("git diff *")).toBeNull();
  });

  it("ラッパーは後ろに何でも書ける", () => {
    expect(broadPatternKind("env *")).toBe("anyCommand");
    expect(broadPatternKind("timeout 10 *")).toBe("anyCommand");
  });

  it("完全一致は書いた 1 通りしか通さないので広くない", () => {
    expect(broadPatternKind("bash")).toBeNull();
    expect(broadPatternKind('bash -c "echo hi"')).toBeNull();
    expect(broadPatternKind("python --version")).toBeNull();
    expect(broadPatternKind("ssh -v outcasts docker exec db psql")).toBeNull();
  });

  it("名前はパス・大文字・Windows の拡張子を寄せて見る", () => {
    expect(broadPatternKind("C:\\Windows\\System32\\cmd.exe /c *")).toBe("anyCommand");
    expect(broadPatternKind("/usr/bin/Bash *")).toBe("anyCommand");
    expect(broadPatternKind("PowerShell.EXE -Command *")).toBe("anyCommand");
  });

  it("空・`*` だけ・余分な空白", () => {
    expect(broadPatternKind("")).toBeNull();
    expect(broadPatternKind("*")).toBeNull();
    expect(broadPatternKind("  bash   *  ")).toBe("anyCommand");
  });
});

describe("broadAllowEntries", () => {
  it("allow の広いパターンを並び順のまま拾う", () => {
    const json = JSON.stringify({ allow: ["git status *", "bash *", "rg *", "ssh outcasts *"], deny: ["sh *"] });
    expect(broadAllowEntries(json)).toEqual([
      { pattern: "bash *", kind: "anyCommand" },
      { pattern: "ssh outcasts *", kind: "remote" },
    ]);
  });

  it("deny と pending は見ない（広い deny は安全側）", () => {
    const json = JSON.stringify({ allow: [], deny: ["bash *"], pending: [{ command: "bash", args: [] }] });
    expect(broadAllowEntries(json)).toEqual([]);
  });

  it("壊れた JSON・allow が配列でない・要素が文字列でない は読み飛ばす", () => {
    expect(broadAllowEntries("{ allow: [")).toEqual([]);
    expect(broadAllowEntries("")).toEqual([]);
    expect(broadAllowEntries(JSON.stringify({ allow: "bash *" }))).toEqual([]);
    expect(broadAllowEntries(JSON.stringify({ allow: [1, null, "bash *"] }))).toEqual([
      { pattern: "bash *", kind: "anyCommand" },
    ]);
  });
});
