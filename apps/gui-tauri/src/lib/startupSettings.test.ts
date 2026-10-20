import { describe, expect, it } from "vitest";

import { machineRegistrationLeft, registeredElsewhere, runsUnattended } from "./startupSettings";
import type { AutostartView } from "../types";

const exe = "C:\\Program Files\\fuseforks\\fuseforks.exe";

function view(patch: Partial<AutostartView>): AutostartView {
  return { enabled: true, scope: "user", registeredCommand: `"${exe}" --autostart`, currentExe: exe, ...patch };
}

describe("registeredElsewhere（Spec 69 D4）", () => {
  it("今の exe を指していれば知らせない（大文字小文字は区別しない）", () => {
    expect(registeredElsewhere(view({}))).toBe(false);
    expect(registeredElsewhere(view({ registeredCommand: `"${exe.toUpperCase()}" --autostart` }))).toBe(false);
  });
  it("別の場所の exe を指していれば知らせる（開発ビルドや NSIS と MSI の並存）", () => {
    expect(
      registeredElsewhere(view({ registeredCommand: '"D:\\Github\\Fuseforks\\target\\debug\\fuseforks.exe" --autostart' })),
    ).toBe(true);
  });
  it("登録が無いときは知らせない", () => {
    expect(registeredElsewhere(view({ registeredCommand: null, enabled: false, scope: "none" }))).toBe(false);
  });
});

describe("machineRegistrationLeft", () => {
  it("全ユーザー向けの登録があるときだけ知らせる", () => {
    expect(machineRegistrationLeft(view({ scope: "machine" }))).toBe(true);
    expect(machineRegistrationLeft(view({ scope: "both" }))).toBe(true);
    expect(machineRegistrationLeft(view({ scope: "user" }))).toBe(false);
    expect(machineRegistrationLeft(view({ scope: "none" }))).toBe(false);
  });
});

describe("runsUnattended", () => {
  const all = { minimizeToTray: true, batchStartOnLaunch: true };
  it("3 つとも ON のときだけ真", () => {
    expect(runsUnattended(all, view({ enabled: true }))).toBe(true);
    expect(runsUnattended(all, view({ enabled: false }))).toBe(false);
    expect(runsUnattended({ ...all, minimizeToTray: false }, view({}))).toBe(false);
    expect(runsUnattended({ ...all, batchStartOnLaunch: false }, view({}))).toBe(false);
  });
  it("自動起動をまだ読めていなければ偽", () => {
    expect(runsUnattended(all, null)).toBe(false);
  });
});
