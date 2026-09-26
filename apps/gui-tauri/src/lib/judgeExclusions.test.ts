// @ts-expect-error @types/node を入れない方針のため（editorSaveWiring.test.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

/**
 * 判断役（Spec 62 D2）が**入ってはいけない場所**を機械で留める。
 *
 * 判断役は受信箱もターンも持たない関数で、会話の相手ではない。だから起動のトグル・
 * 一括起動・グループ・Alt+↑↓ の選択・会話の宛先・`@@` の候補のどれにも入らない。
 *
 * **今これが成り立っているのは構造による** — 判断役は `state.agents` ではなく
 * `state.judges` という別の一覧に住み、上の機構はどれも `state.agents` しか読まない。
 * 壊れるのは「親切心で判断役も足す」形なので、**それらの規則のファイルが `judges` を
 * 読まない**ことを留める。型検査は通る（`JudgeView` にも `id` と `name` がある）ので、
 * 足されると判断役が一括起動の対象や会話の宛先に化け、選んだ瞬間に会話ペインが壊れる
 * （`selectedAgentId` はサーヴァントの id が前提）。
 */
function src(path: string): string {
  return readFileSync(new URL(path, import.meta.url), "utf8");
}

/** 判断役を読んではいけない規則（起動・一括・グループ・選択・宛先・参照）。 */
const MUST_NOT_READ_JUDGES = [
  "./batchStart.ts",
  "./agentGroups.ts",
  "./agentNav.ts",
  "./pathComplete.ts",
  "./quoteRef.ts",
  "../components/ChatInput.vue",
  "../components/AgentCard.vue",
  "../components/GroupDialog.vue",
  "../components/BatchWorkDirDialog.vue",
];

describe("判断役が入らない場所（Spec 62 D2）", () => {
  it.each(MUST_NOT_READ_JUDGES)("%s は判断役の一覧を読まない", (path) => {
    expect(src(path)).not.toMatch(/judges/);
  });

  it("一覧の判断役の行は、選択・起動・一括起動へ触れない", () => {
    const list = src("../components/JudgeList.vue");
    expect(list).not.toMatch(/orchestrator\.(select|toggleRunning|runBatch|setBatchStart)\(/);
    // 行を押したときに開くのは編集ダイアログ。
    expect(list).toContain("dialog.open(judge.id)");
  });

  it("地図のノードを押すと、判断役は選択ではなく編集ダイアログへ", () => {
    const map = src("../components/TopologyMap.vue");
    expect(map).toContain("isJudge(node) ? judgeDialog.open(node as AgentId) : orchestrator.select(");
  });

  it("選択の正規化はサーヴァントの一覧だけから選ぶ", () => {
    // 一覧に無い選択を先頭へ戻す行が judges を見ると、判断役が選択に入る。
    const orch = src("../composables/useOrchestrator.ts");
    expect(orch).toContain("state.selectedAgentId = agents[0]?.id ?? null;");
  });
});

describe("判断役の線（Spec 62 D9）", () => {
  it("判断役 → サーヴァントの線は有効な判断役の to から合成し、地図から切らない", () => {
    const map = src("../components/TopologyMap.vue");
    expect(map).toContain("judgeEdges(state.judges)");
    expect(map).toContain("if (edge && !edges.value[edge]?.judge) void removeEdge(edge);");
  });

  it("カードの drop は判断役を受け側にできる", () => {
    const list = src("../components/AgentList.vue");
    expect(list).toContain("tieAddition(state.agents, source.id, targetId, state.judges.map((j) => j.id))");
  });

  it("id の導出は 2 つの一覧をまたぐ（名前空間は 1 つ）", () => {
    for (const path of ["../components/AgentList.vue", "../components/JudgeList.vue"]) {
      const s = src(path);
      expect(s).toContain("...state.agents.map((a) => a.id), ...state.judges.map((j) => j.id)");
    }
  });
});
