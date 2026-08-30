/**
 * スケジュールダイアログの下書き（2026-08-30 の 2 ペイン化で新設）。**純関数だけ。**
 *
 * フォーム ⇄ ワイヤ型の往復をコンポーネントの外に置く — 編集（既存の予定を
 * フォームへ流し込む）が増えた時点で、変換が template の中に散らばると
 * 「新規では送れるのに編集で欄が落ちる」形が生まれる（Spec 14 P1 の
 * 「投影から組み直して欄が消える」と同族の穴）。往復はテストで留める。
 *
 * 下書きは**捨てられる前提**（`RoleDialog` の draft と同じ設計 —
 * 一覧の別項目を選ぶと確認なしで上書きされる）。
 */

import {
  acceptanceFormValid,
  parseProbeArgs,
  probeFormValid,
} from "./scheduleProbe";
import type {
  Recurrence,
  ScheduleOptions,
  ScheduleView,
  SessionMode,
  Weekday,
} from "../types";

/** フォームの全欄。テキスト欄は未トリムのまま持ち、送信時に畳む。 */
export interface ScheduleDraft {
  to: string;
  message: string;
  kind: Recurrence["kind"];
  weekday: Weekday;
  hour: number;
  minute: number;
  everyMinutes: number;
  probeOn: boolean;
  probeCommand: string;
  /** 引数は **1 行 1 引数** のテキスト（`parseProbeArgs` で配列へ）。 */
  probeArgs: string;
  probeExpect: string;
  probeTimeout: number;
  probeCwd: string;
  sessionMode: SessionMode;
  summarizeAfter: boolean;
  acceptanceOn: boolean;
  accCommand: string;
  accArgs: string;
  accExpect: string;
  accTimeout: number;
  accCwd: string;
  accMaxAttempts: number;
}

/** 新規作成の初期値。既定はコア側の既定（timeout 60 / maxAttempts 2）と揃える。 */
export function emptyDraft(): ScheduleDraft {
  return {
    to: "",
    message: "",
    kind: "weekly",
    weekday: "thu",
    hour: 17,
    minute: 0,
    everyMinutes: 60,
    probeOn: false,
    probeCommand: "",
    probeArgs: "",
    probeExpect: "",
    probeTimeout: 60,
    probeCwd: "",
    sessionMode: "continue",
    summarizeAfter: false,
    acceptanceOn: false,
    accCommand: "",
    accArgs: "",
    accExpect: "",
    accTimeout: 60,
    accCwd: "",
    accMaxAttempts: 2,
  };
}

/**
 * 既存の予定をフォームへ流し込む（編集の入口）。
 *
 * **選ばれなかった種別の欄は初期値のまま残す** — weekly の予定を開いても
 * interval の欄（60 分）は生きており、種別を切り替えた瞬間から編集できる。
 */
export function draftFromSchedule(task: ScheduleView): ScheduleDraft {
  const draft = emptyDraft();
  draft.to = task.to;
  draft.message = task.message;
  const recurrence = task.recurrence;
  draft.kind = recurrence.kind;
  if (recurrence.kind === "interval") {
    draft.everyMinutes = recurrence.everyMinutes;
  } else {
    draft.hour = recurrence.hour;
    draft.minute = recurrence.minute;
    if (recurrence.kind === "weekly") draft.weekday = recurrence.weekday;
  }
  if (task.probe) {
    draft.probeOn = true;
    draft.probeCommand = task.probe.command;
    draft.probeArgs = task.probe.args.join("\n");
    draft.probeExpect = task.probe.expect;
    draft.probeTimeout = task.probe.timeoutSecs;
    draft.probeCwd = task.probe.cwd ?? "";
  }
  draft.sessionMode = task.sessionMode ?? "continue";
  draft.summarizeAfter = task.summarizeAfter ?? false;
  if (task.acceptance) {
    draft.acceptanceOn = true;
    draft.accCommand = task.acceptance.command;
    draft.accArgs = task.acceptance.args.join("\n");
    draft.accExpect = task.acceptance.expect;
    draft.accTimeout = task.acceptance.timeoutSecs;
    draft.accCwd = task.acceptance.cwd ?? "";
    draft.accMaxAttempts = task.acceptance.maxAttempts;
  }
  return draft;
}

/** 送信できる状態か。数値の範囲はコア側でも検証される（二重化 — 本体はコア）。 */
export function draftValid(draft: ScheduleDraft): boolean {
  if (!draft.to || !draft.message.trim()) return false;
  if (
    draft.probeOn &&
    !probeFormValid({
      command: draft.probeCommand,
      expect: draft.probeExpect,
      timeoutSecs: draft.probeTimeout,
    })
  ) {
    return false;
  }
  if (
    draft.acceptanceOn &&
    !acceptanceFormValid({
      command: draft.accCommand,
      expect: draft.accExpect,
      timeoutSecs: draft.accTimeout,
      maxAttempts: draft.accMaxAttempts,
    })
  ) {
    return false;
  }
  if (draft.kind === "interval") return draft.everyMinutes >= 1;
  return draft.hour >= 0 && draft.hour <= 23 && draft.minute >= 0 && draft.minute <= 59;
}

/** 下書き → 再現規則。 */
export function recurrenceFromDraft(draft: ScheduleDraft): Recurrence {
  switch (draft.kind) {
    case "interval":
      return { kind: "interval", everyMinutes: Math.floor(draft.everyMinutes) };
    case "daily":
      return { kind: "daily", hour: draft.hour, minute: draft.minute };
    case "weekly":
      return {
        kind: "weekly",
        weekday: draft.weekday,
        hour: draft.hour,
        minute: draft.minute,
      };
  }
}

/** 下書き → 追加指定。**既定のままの欄も送る** — 受け側が既定へ畳む。 */
export function optionsFromDraft(draft: ScheduleDraft): ScheduleOptions {
  return {
    probe: draft.probeOn
      ? {
          command: draft.probeCommand.trim(),
          args: parseProbeArgs(draft.probeArgs),
          expect: draft.probeExpect.trim(),
          timeoutSecs: Math.floor(draft.probeTimeout),
          cwd: draft.probeCwd.trim() || null,
        }
      : null,
    sessionMode: draft.sessionMode,
    summarizeAfter: draft.summarizeAfter,
    acceptance: draft.acceptanceOn
      ? {
          command: draft.accCommand.trim(),
          args: parseProbeArgs(draft.accArgs),
          expect: draft.accExpect.trim(),
          timeoutSecs: Math.floor(draft.accTimeout),
          cwd: draft.accCwd.trim() || null,
          maxAttempts: Math.floor(draft.accMaxAttempts),
        }
      : null,
  };
}
