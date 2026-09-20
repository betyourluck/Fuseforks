// @ts-expect-error @types/node を入れない方針のため（editorSaveWiring.test.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import en from "../locales/en.json";
import ja from "../locales/ja.json";

/**
 * 「承認して続けさせる」の配線（Spec 56 P2）。
 *
 * **押せないことが安全の一部**なので、`:disabled` から述語が落ちる形を
 * 機械で留める — 落ちても**型検査にも lint にも掛からず**、停止中の個体で
 * 押せるボタンが増えるだけに見える（実際には承認だけ済んで配送が落ち、
 * 中途半端な状態が増える）。
 *
 * 2026-09-17 の作業状況タブの印で踏んだ形の再発防止でもある — あのときは
 * 判定を `.vue` に直書きしたまま走査が字面しか見ておらず、**判定を
 * `return false` に倒しても緑のまま**だった。判定は `resumeGate.ts` の
 * 純関数が持ち、ここが確かめるのは**画面がそれを読んでいること**だけ。
 */
const dialog = readFileSync(
  new URL("../components/CommandApprovalDialog.vue", import.meta.url),
  "utf8",
);

describe("承認して続けさせるの配線", () => {
  it("押せるかの判定は純関数を読む（部品に直書きしない）", () => {
    expect(dialog).toContain('from "../lib/resumeGate"');
    expect(dialog).toContain("canResume(statusOf(view.agentId))");
    // 状態の文字列をテンプレートで直に比べない（比較を足すと純関数と 2 実装になる）。
    expect(dialog).not.toMatch(/status\s*===\s*["']running["']/);
  });

  it("ボタンの disabled は busy と canResume の両方を見る", () => {
    expect(dialog).toMatch(/busy !== null \|\| !canResume\(statusOf\(view\.agentId\)\)/);
  });

  it("押せないときだけ理由をツールチップに出す", () => {
    expect(dialog).toContain("commandApproval.resumeDisabled");
    // 常時 title を出すと、押せるときにも「押せません」が浮く。
    expect(dialog).toMatch(/canResume\(statusOf\(view\.agentId\)\)\s*\n?\s*\?\s*undefined/);
  });

  it("既存の「承認」を置き換えず、続行は別のボタンで送る", () => {
    // 承認だけしたい場面は実在する（D1）。2 つのボタンが両方あること。
    expect(dialog).toContain("commandApproval.approve");
    expect(dialog).toContain("commandApproval.approveAndResume");
    expect(dialog).toContain("decide(view.agentId, request, true, true)");
  });

  it("配送は承認が通った後にしか試みない", () => {
    // 承認が落ちた（notFound / null）のに続行を送ると、許容が 1 行も
    // 増えていない状態で同じ拒否を踏むターンが起きる。
    expect(dialog).toMatch(/if \(!resume \|\| outcome === null\) return;/);
    expect(dialog).toContain("resumeAfterApproval(agentId)");
  });

  it("辞書は ja / en とも 3 鍵そろっている", () => {
    for (const dict of [ja, en]) {
      const section = (dict as { commandApproval: Record<string, string> }).commandApproval;
      for (const key of ["approveAndResume", "resumeDisabled", "resumeFailed"]) {
        expect(section[key], `commandApproval.${key}`).toBeTruthy();
      }
      const op = (dict as { orchestrator: { op: Record<string, string> } }).orchestrator.op;
      expect(op.resumeAfterApproval, "orchestrator.op.resumeAfterApproval").toBeTruthy();
    }
  });
});
