/**
 * 作業フォルダの一括切り替えの規則（Spec 29 D2 / D3）。
 *
 * **査読 #9 の裁定でここが部分失敗の固定先になった** — 実機で削除競合を
 * 起こすのは狙わないと出ないので、名指しの結果表示はこの層で留める。
 */
import { describe, expect, it, vi } from "vitest";

import { applyWorkDir, canApply, type BatchTarget } from "./batchWorkDir";

const TARGETS: BatchTarget[] = [
  { id: "agent_2", name: "ロボットくん１号" },
  { id: "agent_5", name: "ロボットくん２号" },
  { id: "agent_9", name: "ミュゼ" },
];

describe("canApply", () => {
  it("パスがあり対象が 1 体以上なら押せる", () => {
    expect(canApply("D:\\work", 1)).toBe(true);
  });

  it("空白だけのパスでは押せない", () => {
    // trim して見る。空白だけを通すと work_dir に空白が保存される。
    for (const path of ["", "   ", "\t"]) {
      expect(canApply(path, 3)).toBe(false);
    }
  });

  it("対象が 0 体なら押せない", () => {
    expect(canApply("D:\\work", 0)).toBe(false);
  });

  it("存在しないパスでも押せる（実在検査はしない）", () => {
    // D2 — 囲いはツール実行時の resolve_in_work_dir が持つ。ここで先回りすると
    // 同じ規律が 2 箇所に生える。**これは仕様であって手抜きではない。**
    expect(canApply("Z:\\そんなフォルダは無い", 1)).toBe(true);
  });
});

describe("applyWorkDir", () => {
  it("全部通れば失敗は空", async () => {
    const summary = await applyWorkDir(TARGETS, async () => {}, () => "理由");

    expect(summary.succeeded).toBe(3);
    expect(summary.failed).toEqual([]);
  });

  it("1 体が失敗しても残りは続行し、失敗を名指しで返す", async () => {
    // 査読 #9 の本体。**真ん中を落とす**のは、最後を落とすと「続行した」のか
    // 「そこで終わった」のか区別が付かないため。
    const update = vi.fn(async (target: BatchTarget) => {
      if (target.id === "agent_5") throw new Error("見つかりません");
    });

    const summary = await applyWorkDir(TARGETS, update, (error) => (error as Error).message);

    expect(update).toHaveBeenCalledTimes(3);
    expect(summary.succeeded).toBe(2);
    expect(summary.failed).toEqual([
      { id: "agent_5", name: "ロボットくん２号", reason: "見つかりません" },
    ]);
  });

  it("結末は対象の順に並び、成功した個体の理由は null", async () => {
    const summary = await applyWorkDir(
      TARGETS,
      async (target) => {
        if (target.id === "agent_2") throw new Error("だめ");
      },
      () => "だめ",
    );

    expect(summary.outcomes.map((o) => o.id)).toEqual(["agent_2", "agent_5", "agent_9"]);
    expect(summary.outcomes.map((o) => o.reason)).toEqual(["だめ", null, null]);
  });

  it("逐次に呼ぶ（前の 1 体が終わるまで次を始めない）", async () => {
    // 並列にすると world.json の書き込みが交錯する形を自分から作る（D2）。
    const running: string[] = [];
    let maxConcurrent = 0;

    await applyWorkDir(
      TARGETS,
      async (target) => {
        running.push(target.id);
        maxConcurrent = Math.max(maxConcurrent, running.length);
        await Promise.resolve();
        running.pop();
      },
      () => "",
    );

    expect(maxConcurrent).toBe(1);
  });

  it("進捗は 1 体ごとに、総数つきで報告する", async () => {
    const seen: Array<[number, number]> = [];
    await applyWorkDir(TARGETS, async () => {}, () => "", (done, total) =>
      seen.push([done, total]),
    );

    expect(seen).toEqual([
      [1, 3],
      [2, 3],
      [3, 3],
    ]);
  });

  it("対象が空なら 1 度も呼ばない", async () => {
    const update = vi.fn(async () => {});
    const summary = await applyWorkDir([], update, () => "");

    expect(update).not.toHaveBeenCalled();
    expect(summary.succeeded).toBe(0);
  });
});
