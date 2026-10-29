import { describe, expect, it } from "vitest";
import { canResume, canResumeNow, resumeBlock } from "./resumeGate";
import type { AgentStatus, CommandPolicyView } from "../types";

function view(over: Partial<CommandPolicyView> = {}): CommandPolicyView {
  return {
    agentId: "agent_1",
    name: "ザリ",
    pending: [],
    broken: false,
    stoppedForApproval: true,
    ...over,
  };
}

const pending = { command: "git", args: ["status"], firstRequestedAtMs: 0, count: 1 };

describe("resumeBlock / canResumeNow（Spec 72 D5）", () => {
  it("止まっていて・判断待ち 0・読めて・稼働中なら押せる", () => {
    expect(resumeBlock(view(), "running")).toBeNull();
    expect(canResumeNow(view(), "running")).toBe(true);
    expect(canResumeNow(view(), "starting")).toBe(true);
  });

  it("止まっていなければ押す対象が無い", () => {
    expect(resumeBlock(view({ stoppedForApproval: false }), "running")).toBe("notStopped");
  });

  it("判断待ちが残っていれば先に片付けさせる（再開しても最初の Unknown でまた止まる）", () => {
    expect(resumeBlock(view({ pending: [pending] }), "running")).toBe("pending");
    expect(canResumeNow(view({ pending: [pending] }), "running")).toBe(false);
  });

  it("run.json が読めなければ直させる（全部 Unknown に落ちるので同じ所で止まる）", () => {
    expect(resumeBlock(view({ broken: true }), "running")).toBe("broken");
  });

  it("稼働していなければ Spec 56 と同じ理由", () => {
    for (const status of ["idle", "stopping", "failed", undefined] as (AgentStatus | undefined)[]) {
      expect(resumeBlock(view(), status), String(status)).toBe("notRunning");
      expect(canResume(status)).toBe(false);
    }
  });

  it("理由の順は 判断待ち → 壊れている → 稼働していない", () => {
    expect(resumeBlock(view({ pending: [pending], broken: true }), "idle")).toBe("pending");
    expect(resumeBlock(view({ broken: true }), "idle")).toBe("broken");
  });
});
