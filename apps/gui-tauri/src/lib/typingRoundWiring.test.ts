/**
 * 入力中バブルの周回数の**配線**を機械で留める（2026-10-15）。
 *
 * 値は 4 つの場所を手で揃えて運ぶ — コアの `ToolInvoked.round`（`event.rs`）→ `types.ts` の
 * `toolInvoked.round` → `useOrchestrator` の `toolRound` → `ChatPanel` のバブル。
 * ワイヤ形はコアの `ipc_contract` が凍結する。ここで留めるのはフロントの側 —
 * 写し忘れると**バブルは出るのに周回が永遠に出ない**か、**前のターンの周回が残る**
 * （終了で消す側を落とすと）。型検査は `round` を読まない配線でも通る。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

function read(rel: string): string {
  return readFileSync(fileURLToPath(new URL(rel, import.meta.url)), "utf8");
}

const panel = read("../components/ChatPanel.vue");
const orchestrator = read("../composables/useOrchestrator.ts");
const types = read("../types.ts");
const eventRs = read("../../../../crates/fuseforks-core/src/event.rs");
const ja = JSON.parse(read("../locales/ja.json")) as { chat: Record<string, string> };
const en = JSON.parse(read("../locales/en.json")) as { chat: Record<string, string> };

describe("入力中バブルの周回数の配線", () => {
  it("コアの ToolInvoked と types.ts の toolInvoked が round を持つ", () => {
    const variant = /ToolInvoked \{[\s\S]*?\n {4}\},/.exec(eventRs);
    expect(variant, "event.rs に ToolInvoked が無い").not.toBeNull();
    expect(variant![0]).toMatch(/\bround: u8,/);
    const front = /type: "toolInvoked";[\s\S]*?\n {4}\}/.exec(types);
    expect(front, "types.ts に toolInvoked が無い").not.toBeNull();
    expect(front![0]).toMatch(/\bround: number;/);
  });

  it("useOrchestrator は toolInvoked で写し、ターンの終わりと停止で消す", () => {
    expect(orchestrator).toContain("toolRound: Record<AgentId, number>;");
    expect(orchestrator).toContain("state.toolRound[event.agentId] = event.round;");
    // 消す側は 2 箇所（agentTyping の終了 / 稼働でなくなったとき）。
    const deletes = orchestrator.match(/delete state\.toolRound\[event\.agentId\];/g) ?? [];
    expect(deletes).toHaveLength(2);
    // ToolRun にも写す（時系列の行が周を持つ）。
    expect(orchestrator).toContain("round: event.round,");
  });

  it("ChatPanel は入力中バブルの中でだけ読み、バッジは純関数の形・全文は title", () => {
    const bubble = /class="typing-bubble[\s\S]*?<\/div>/.exec(panel);
    expect(bubble, "入力中バブルが無い").not.toBeNull();
    expect(bubble![0]).toContain('v-if="state.toolRound[agent.id]"');
    // バッジの文字と警告は lib/typingRound.ts の規則から（画面で数を組まない）。
    expect(bubble![0]).toContain("roundBadgeOf(agent).label");
    expect(bubble![0]).toContain("roundBadgeOf(agent).nearLimit ? 'typing-round-near'");
    expect(panel).toContain("roundBadge(state.toolRound[agent.id] ?? 0, agent.maxToolIterations)");
    // 全文（「ツール N 周目」）は title / aria-label に残す。
    expect(bubble![0]).toContain(':title="typingRoundText(agent)"');
    expect(panel).toContain('t("chat.typingRoundOf", { round, max: agent.maxToolIterations })');
    expect(panel).toContain('t("chat.typingRound", { round })');
    // 警告色のクラスは scoped CSS に在る。
    expect(panel).toMatch(/\.typing-round-near\s*\{/);
  });

  it("辞書の鍵が ja と en の両方に在り、引数が揃っている", () => {
    for (const [lang, dict] of [["ja", ja], ["en", en]] as const) {
      expect(typeof dict.chat.typingRound, lang).toBe("string");
      expect(typeof dict.chat.typingRoundOf, lang).toBe("string");
      const params = (text: string) => [...text.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort();
      expect(params(dict.chat.typingRound), lang).toEqual(["round"]);
      expect(params(dict.chat.typingRoundOf), lang).toEqual(["max", "round"]);
    }
  });
});
