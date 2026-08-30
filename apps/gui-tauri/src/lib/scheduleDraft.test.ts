/**
 * スケジュールの下書き（2 ペイン化・2026-08-30）の変換規則。
 *
 * 主題は**往復** — 保存済みの予定 → 下書き → ワイヤ型、で元の内容が
 * 1 欄も落ちないこと。編集の入口が増えた時点で、変換の片道だけを見ると
 * 「新規では送れるのに編集で欄が落ちる」形が緑のまま入る。
 */
import { describe, expect, it } from "vitest";

import {
  draftFromSchedule,
  draftValid,
  emptyDraft,
  optionsFromDraft,
  recurrenceFromDraft,
} from "./scheduleDraft";
import type { ScheduleView } from "../types";

/** 全部盛りの予定（前判定・後判定・fresh・要約 — 欄が最も多い形）。 */
function fullTask(): ScheduleView {
  return {
    id: "task-1",
    to: "agent_01",
    message: "done.txt を確かめて",
    recurrence: { kind: "interval", everyMinutes: 5 },
    createdAtMs: 0,
    lastConsumedDueMs: null,
    enabled: true,
    probe: {
      command: "python",
      args: ["-c", "print('GO')"],
      expect: "GO",
      timeoutSecs: 30,
      cwd: "D:\\work",
    },
    sessionMode: "fresh",
    summarizeAfter: true,
    acceptance: {
      command: "python",
      args: ["check.py"],
      expect: "OK",
      timeoutSecs: 90,
      cwd: null,
      maxAttempts: 3,
    },
    nextDueMs: null,
    recurrenceLabel: "5 分ごと",
    probeApproved: true,
    lastProbe: null,
    acceptanceApproved: true,
    lastAcceptance: null,
  };
}

describe("draftFromSchedule → optionsFromDraft / recurrenceFromDraft の往復", () => {
  it("全部盛りの予定が 1 欄も落ちずに元へ戻る", () => {
    const draft = draftFromSchedule(fullTask());
    expect(recurrenceFromDraft(draft)).toEqual({ kind: "interval", everyMinutes: 5 });
    expect(optionsFromDraft(draft)).toEqual({
      probe: {
        command: "python",
        args: ["-c", "print('GO')"],
        expect: "GO",
        timeoutSecs: 30,
        cwd: "D:\\work",
      },
      sessionMode: "fresh",
      summarizeAfter: true,
      acceptance: {
        command: "python",
        args: ["check.py"],
        expect: "OK",
        timeoutSecs: 90,
        cwd: null,
        maxAttempts: 3,
      },
    });
    expect(draftValid(draft)).toBe(true);
  });

  it("素の予定（欄がワイヤに現れない形）は既定へ畳まれる", () => {
    // probe / sessionMode / summarizeAfter / acceptance は既定だとワイヤに
    // 現れない（Rust 側の skip_serializing_if）。undefined を既定として
    // 読めないと、既存の予定を開いて保存しただけで挙動が変わる。
    const task: ScheduleView = {
      ...fullTask(),
      recurrence: { kind: "weekly", weekday: "thu", hour: 17, minute: 0 },
      probe: undefined,
      sessionMode: undefined,
      summarizeAfter: undefined,
      acceptance: undefined,
    };
    const draft = draftFromSchedule(task);
    expect(recurrenceFromDraft(draft)).toEqual({
      kind: "weekly",
      weekday: "thu",
      hour: 17,
      minute: 0,
    });
    expect(optionsFromDraft(draft)).toEqual({
      probe: null,
      sessionMode: "continue",
      summarizeAfter: false,
      acceptance: null,
    });
  });

  it("引数のテキストは 1 行 1 引数で往復する（各行の前後の空白は落ちる）", () => {
    const draft = draftFromSchedule(fullTask());
    expect(draft.probeArgs).toBe("-c\nprint('GO')");
    // 貼り付けで行頭に空白が付いても argv には乗らない（parseProbeArgs が
    // 各行をトリムする）。**行の途中の空白はそのまま残る** — `-c` とコードを
    // 1 行に書くと 1 個の argv になり、python は `-c` の残り（先頭の空白ごと）を
    // コードとして読む = 実機の IndentationError（2026-08-30）の機序。
    draft.accArgs = "  -c  \n  print('NG')";
    expect(optionsFromDraft(draft).acceptance?.args).toEqual(["-c", "print('NG')"]);
  });

  it("新規の初期値はそのままでは送信できない（宛先と依頼が要る）", () => {
    expect(draftValid(emptyDraft())).toBe(false);
  });
});
