// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { reorderGroupIds } from "./agentGroups";

describe("reorderGroupIds", () => {
  it("送られた順を先頭へ置き、送られなかったグループは元の順で後ろへ残す", () => {
    expect(reorderGroupIds(["a", "b", "c"], ["c", "a"])).toEqual(["c", "a", "b"]);
  });

  it("未知の id と重複は読み飛ばし、グループを 1 つも失わない", () => {
    expect(reorderGroupIds(["a", "b"], ["ghost", "b", "b"])).toEqual(["b", "a"]);
    expect(reorderGroupIds(["a", "b"], [])).toEqual(["a", "b"]);
  });
});

/** 配線の走査。外側の箱がカードの箱と混ざる形・掴み手以外から動く形を字面で留める。 */
describe("AgentList のグループの並び替えの配線", () => {
  const src = readFileSync(new URL("../components/AgentList.vue", import.meta.url), "utf8");

  it("区分けの箱はカードの箱と別の group 名で、掴み手からだけ動き、絞り込み中は止まる", () => {
    expect(src).toContain('const SECTION_DRAG_GROUP = "agent-sections";');
    expect(src).toContain(':group="SECTION_DRAG_GROUP"');
    expect(src).toContain('handle="[data-group-handle]"');
    expect(src).toContain('draggable="[data-group-section]"');
    expect(src).toContain(':disabled="searching || !hasGroups"');
  });

  it("箱へ渡す配列は動かせる区分け（グループ）だけ — 無所属を混ぜると入れ替えの位置が 1 つずれる", () => {
    // vue-draggable-plus は配列を draggable の子だけで数えた位置で入れ替える（2026-11-05 実機）。
    expect(src).toMatch(/<VueDraggable\s+:model-value="groupSections"/);
    expect(src).toContain("const groupSections = computed(() => shownSections.value.filter((e) => e.section.group));");
  });

  it("掴み手と並べ替えの印は所属グループにだけ付く（無所属は先頭に固定）", () => {
    expect(src).toMatch(/:data-group-section="section\.group \? 'true' : undefined"/);
    expect(src).toMatch(/v-if="section\.group && !searching"\s+data-group-handle/);
  });
});

describe("AgentCard のエラーの折り返し", () => {
  it("エラー本文は語の途中でも折る（空白の無い JSON や URL がはみ出さない）", () => {
    const card = readFileSync(new URL("../components/AgentCard.vue", import.meta.url), "utf8");
    const button = card.slice(card.indexOf('v-if="showsError"'), card.indexOf("formatError(agent.lastError!)"));
    expect(button).toContain("[overflow-wrap:anywhere]");
    expect(button).toContain("min-w-0");
  });
});
