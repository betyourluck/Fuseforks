// @ts-expect-error @types/node を入れない方針のため（editorSaveWiring.test.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import en from "../locales/en.json";
import ja from "../locales/ja.json";

/**
 * 「続けさせる」の配線（Spec 56 P2 → Spec 72 P2）。
 *
 * **押せないことが安全の一部**なので、`:disabled` から述語が落ちる形を
 * 機械で留める — 落ちても**型検査にも lint にも掛からず**、判断待ちが残る個体や
 * 停止中の個体で押せるボタンが増えるだけに見える（実際には再開したターンが最初の
 * `Unknown` でまた止まるか、配送が落ちて中途半端な状態が増える）。
 *
 * 2026-09-17 の作業状況タブの印で踏んだ形の再発防止でもある — あのときは
 * 判定を `.vue` に直書きしたまま走査が字面しか見ておらず、**判定を
 * `return false` に倒しても緑のまま**だった。判定は `resumeGate.ts` の
 * 純関数が持ち、ここが確かめるのは**画面がそれを読んでいること**だけ。
 *
 * Spec 72 で変わった形: 判断待ち 1 件ごとの「承認して続けさせる」は無く、
 * **個体の節の末尾に「続けさせる」1 つ**。承認と続行は別のボタン。
 */
const dialog = readFileSync(
  new URL("../components/CommandApprovalDialog.vue", import.meta.url),
  "utf8",
);

describe("続けさせるの配線", () => {
  it("押せるかの判定は純関数を読む（部品に直書きしない）", () => {
    expect(dialog).toContain('from "../lib/resumeGate"');
    expect(dialog).toContain("canResumeNow(view, statusOf(view.agentId))");
    // 状態の文字列をテンプレートで直に比べない（比較を足すと純関数と 2 実装になる）。
    expect(dialog).not.toMatch(/status\s*===\s*["']running["']/);
    // 判断待ちの件数で押せるかを部品で判定しない（純関数の `resumeBlock` が持つ）。
    // 空状態の v-if は件数を見てよいので、見るのはボタンの :disabled の式だけ。
    expect(dialog).not.toMatch(/:disabled="[^"]*pending\.length/);
  });

  it("ボタンの disabled は busy と canResumeNow の両方を見る", () => {
    expect(dialog).toMatch(/busy !== null \|\| !canResumeNow\(view, statusOf\(view\.agentId\)\)/);
  });

  it("押せないときだけ理由をツールチップに出す（理由は 3 つに分かれる）", () => {
    expect(dialog).toContain(':title="resumeTitle(view)"');
    for (const key of [
      "commandApproval.resumeBlockedPending",
      "commandApproval.resumeBlockedBroken",
      "commandApproval.resumeDisabled",
    ]) {
      expect(dialog).toContain(key);
    }
    // 押せるときは undefined（常時 title を出すと「押せません」が浮く）。
    expect(dialog).toMatch(/case null:\s*\n\s*case "notStopped":\s*\n\s*return undefined;/);
  });

  it("ボタンは個体ごとに 1 つで、判断待ち 1 件ごとの「承認して続けさせる」は無い", () => {
    expect(dialog).toContain("commandApproval.approve");
    expect(dialog).toContain("commandApproval.resume");
    expect(dialog).not.toContain("approveAndResume");
    // decide は承認 / 却下だけで、続行の引数を持たない。
    expect(dialog).not.toMatch(/decide\([^)]*,\s*true,\s*true\)/);
    expect(dialog).toContain('v-if="view.stoppedForApproval"');
    expect(dialog).toContain('@click="resume(view)"');
  });

  it("節は判断待ちが 0 件でも止まった個体なら残し、空状態の条件も対で広げる", () => {
    expect(dialog).toContain("v.pending.length > 0 || v.broken || v.stoppedForApproval");
    expect(dialog).toContain("v.pending.length === 0 && !v.broken && !v.stoppedForApproval");
  });

  it("続けさせた後に取り直す（節は押した直後に畳まれ、答えは後から来る）", () => {
    const resumeFn = dialog.slice(dialog.indexOf("async function resume("));
    expect(resumeFn).toContain("resumeAfterApproval(view.agentId)");
    expect(resumeFn.indexOf("resumeAfterApproval")).toBeLessThan(
      resumeFn.indexOf("refreshCommandRequests()"),
    );
  });

  it("辞書は ja / en とも必要な鍵がそろい、消した鍵は残っていない", () => {
    for (const dict of [ja, en]) {
      const section = (dict as { commandApproval: Record<string, string> }).commandApproval;
      for (const key of [
        "resume",
        "stoppedNote",
        "resumeBlockedPending",
        "resumeBlockedBroken",
        "resumeDisabled",
        "resumeFailed",
      ]) {
        expect(section[key], `commandApproval.${key}`).toBeTruthy();
      }
      expect(section.approveAndResume, "approveAndResume は Spec 72 で消した").toBeUndefined();
      // 続行の失敗の文は 1 段（承認の成否を言わない — Spec 56 D8 は到達不能になった）。
      expect(section.resumeFailed).not.toMatch(/承認は保存|approval was saved/);
      const op = (dict as { orchestrator: { op: Record<string, string> } }).orchestrator.op;
      expect(op.resumeAfterApproval, "orchestrator.op.resumeAfterApproval").toBeTruthy();
    }
  });
});
