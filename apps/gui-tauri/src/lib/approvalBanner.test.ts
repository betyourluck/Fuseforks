import { describe, expect, it } from "vitest";
import { approvalBanner, pendingTotal } from "./approvalBanner";
import type { CommandPolicyView, PendingCommand } from "../types";

function pending(command: string): PendingCommand {
  return { command, args: [], firstRequestedAtMs: 0, count: 1 };
}

function view(name: string, commands: string[], stopped = false): CommandPolicyView {
  return {
    agentId: name.toLowerCase(),
    name,
    pending: commands.map(pending),
    broken: false,
    stoppedForApproval: stopped,
  };
}

describe("approvalBanner", () => {
  it("承認待ちも止まった個体も無ければ帯を出さない", () => {
    expect(approvalBanner([])).toBeNull();
    expect(approvalBanner([view("Luna", []), view("Zari", [])])).toBeNull();
  });

  it("判断待ちが無く止まった個体だけなら、止まっている帯を出す（Spec 72 D6）", () => {
    const banner = approvalBanner([view("Luna", []), view("Zari", [], true)]);
    expect(banner?.key).toBe("chat.approvalStoppedOne");
    expect(banner?.params).toEqual({ name: "Zari", others: 0, count: 0 });
    const many = approvalBanner([view("Luna", [], true), view("Mue", []), view("Zari", [], true)]);
    expect(many?.key).toBe("chat.approvalStoppedMany");
    expect(many?.params).toEqual({ name: "Luna", others: 1, count: 0 });
  });

  it("判断待ちがあれば止まった個体より先（承認が先・続行が後）", () => {
    const banner = approvalBanner([view("Luna", [], true), view("Zari", ["git"])]);
    expect(banner?.key).toBe("chat.approvalWaitingOne");
    expect(banner?.params.name).toBe("Zari");
  });

  it("1 体だけなら名指しして件数を出す", () => {
    const banner = approvalBanner([view("Luna", []), view("Zari", ["git", "curl"])]);
    expect(banner?.key).toBe("chat.approvalWaitingOne");
    expect(banner?.params).toEqual({ name: "Zari", others: 0, count: 2 });
  });

  it("複数なら一覧の並びで最初の個体を名指しし、残りは数で言う", () => {
    const banner = approvalBanner([view("Luna", ["git"]), view("Mue", []), view("Zari", ["curl", "lake"])]);
    expect(banner?.key).toBe("chat.approvalWaitingMany");
    expect(banner?.params.name).toBe("Luna");
    expect(banner?.params.others).toBe(1);
  });

  it("件数はタイトルバーのバッジと同じ数", () => {
    const views = [view("Luna", ["git"]), view("Mue", []), view("Zari", ["curl", "lake"])];
    expect(approvalBanner(views)?.params.count).toBe(pendingTotal(views));
    expect(pendingTotal(views)).toBe(3);
  });
});
