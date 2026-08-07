import { describe, expect, it } from "vitest";

import {
  parseProbeArgs,
  probeCommandLine,
  probeDisplay,
  probeFormValid,
} from "./scheduleProbe";
import type { ProbeReport, ScheduleView } from "../types";

function view(probe: ScheduleView["probe"]): ScheduleView {
  return {
    id: "t1",
    to: "agent_01",
    message: "見張って",
    recurrence: { kind: "interval", everyMinutes: 5 },
    createdAtMs: 0,
    lastConsumedDueMs: null,
    enabled: true,
    probe,
    nextDueMs: null,
    recurrenceLabel: "5 分ごと",
    probeApproved: true,
    lastProbe: null,
  };
}

describe("parseProbeArgs", () => {
  it("は 1 行 1 引数として読む", () => {
    expect(parseProbeArgs("watch.py\n--verbose")).toEqual(["watch.py", "--verbose"]);
  });

  it("は空行を落とす", () => {
    // 末尾改行で空文字の引数が生えると、それがそのまま argv の 1 要素になる。
    expect(parseProbeArgs("watch.py\n\n")).toEqual(["watch.py"]);
    expect(parseProbeArgs("")).toEqual([]);
  });

  it("は空白を含む 1 行を分割しない", () => {
    // 空白区切りにするとシェルの引用規則が要る（Spec 28 D4 と衝突する）。
    expect(parseProbeArgs("--path=C:/Program Files/x")).toEqual([
      "--path=C:/Program Files/x",
    ]);
  });
});

describe("probeCommandLine", () => {
  it("はコマンドと引数を並べる", () => {
    const task = view({
      command: "python",
      args: ["watch.py", "--json"],
      expect: "CHANGED",
      timeoutSecs: 60,
      cwd: null,
    });
    expect(probeCommandLine(task)).toBe("python watch.py --json");
  });

  it("は cwd があれば添える", () => {
    // **承認の判断材料**。どこで走るかは何が走るかと同じくらい効く。
    const task = view({
      command: "python",
      args: [],
      expect: "CHANGED",
      timeoutSecs: 60,
      cwd: "D:/work",
    });
    expect(probeCommandLine(task)).toBe("python (cwd: D:/work)");
  });

  it("は前判定が無ければ空文字", () => {
    expect(probeCommandLine(view(null))).toBe("");
  });
});

describe("probeDisplay", () => {
  it("は走っていなければ null を返す", () => {
    // 空文字を返すと「判定はしたが結末が無い」という別の主張になる。
    expect(probeDisplay(null)).toBeNull();
    expect(probeDisplay(undefined)).toBeNull();
  });

  it("は結末の辞書キーを返す", () => {
    // **理由の規則は別のテストが見る。** ここで `reason` まで固定すると、
    // 理由の分岐を壊したときにこのテストも一緒に落ち、どちらの規則が
    // 効いて緑だったのかが読めなくなる（Spec 25 P3 で踏んだ形 —
    // AND で並ぶ条件の 1 つを検査するテストは、他の条件をすり抜ける入力で書く）。
    const report: ProbeReport = { outcome: "no_match", reason: "-", atMs: 1000 };
    const display = probeDisplay(report);
    expect(display?.labelKey).toBe("schedule.probeOutcome.no_match");
    expect(display?.atMs).toBe(1000);
  });

  it("は error のときだけ理由を持つ", () => {
    // 他の結末では計器が "-" を入れるので、そのまま出すと画面に "-" が並ぶ。
    const failed: ProbeReport = { outcome: "error", reason: "not_found", atMs: 5 };
    expect(probeDisplay(failed)?.reason).toBe("not_found");

    for (const outcome of ["match", "no_match", "timeout", "unapproved"] as const) {
      const report: ProbeReport = { outcome, reason: "-", atMs: 5 };
      expect(probeDisplay(report)?.reason, outcome).toBeNull();
    }
  });
});

describe("probeFormValid", () => {
  it("はコマンドと合図の両方を要求する", () => {
    expect(probeFormValid({ command: "python", expect: "CHANGED", timeoutSecs: 60 })).toBe(
      true,
    );
    expect(probeFormValid({ command: "  ", expect: "CHANGED", timeoutSecs: 60 })).toBe(false);
    expect(probeFormValid({ command: "python", expect: "  ", timeoutSecs: 60 })).toBe(false);
  });

  it("は打ち切り時間の値域を見る", () => {
    expect(probeFormValid({ command: "p", expect: "C", timeoutSecs: 0 })).toBe(false);
    expect(probeFormValid({ command: "p", expect: "C", timeoutSecs: 3600 })).toBe(true);
    expect(probeFormValid({ command: "p", expect: "C", timeoutSecs: 3601 })).toBe(false);
  });
});
