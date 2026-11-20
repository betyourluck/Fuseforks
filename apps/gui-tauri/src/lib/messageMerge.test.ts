/**
 * 会話の読み直しと、読んでいる間に届いた発話（Spec 77 #18 + N13）。
 *
 * `sessionSwitched` と起動時は `listMessages` で会話を読み直す。読んでいる間にも `messageSent` は
 * 届いて投影へ積まれるので、読んだ結果で丸ごと置き換えると、その間の発話が消える（N13）。
 * また待っている間に別の会話へ切り替わったら、その読み取りは古い会話のもの（#18）。
 * 規則は `messageMerge.ts` の純関数で、配線はソースの字面で留める。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

import type { AgentMessage } from "../types";
import { mergeFetchedMessages } from "./messageMerge";

function msg(id: string): AgentMessage {
  return {
    id,
    from: { kind: "user" },
    to: { kind: "agent", id: "a" },
    content: id,
    tokens: 0,
    tsMs: 0,
    hop: 0,
  } as AgentMessage;
}

const ids = (list: AgentMessage[]) => list.map((m) => m.id);

describe("mergeFetchedMessages", () => {
  it("読んでいる間に届いた発話を、読んだ結果の後ろに残す", () => {
    expect(ids(mergeFetchedMessages([msg("a"), msg("b")], [msg("c")], 500))).toEqual(["a", "b", "c"]);
  });

  it("読んだ結果にもある発話は 2 回並べない", () => {
    expect(ids(mergeFetchedMessages([msg("a"), msg("b")], [msg("b"), msg("c")], 500))).toEqual([
      "a",
      "b",
      "c",
    ]);
  });

  it("上限を超えたら古いほうから落とす", () => {
    expect(ids(mergeFetchedMessages([msg("a"), msg("b")], [msg("c")], 2))).toEqual(["b", "c"]);
  });

  it("届いたものが無ければ読んだ結果のまま", () => {
    expect(ids(mergeFetchedMessages([msg("a")], [], 500))).toEqual(["a"]);
  });
});

const read = (path: string) =>
  readFileSync(fileURLToPath(new URL(`../../${path}`, import.meta.url)), "utf8");

/** コメントを落とす（走査テストはコメントも読むので、説明文の字面に当たらないように）。 */
const code = (src: string) => src.replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");

const orchestrator = code(read("src/composables/useOrchestrator.ts"));

describe("配線", () => {
  it("会話を読み直す所は、読んだ結果で丸ごと置き換えない", () => {
    expect(orchestrator).not.toMatch(/state\.messages = await ipc\.listMessages/);
    const merged = orchestrator.match(/mergeFetchedMessages\(/g) ?? [];
    expect(merged.length).toBe(2);
  });

  it("切り替えの読み直しは、待った後にいま開いている会話を確かめる（#18）", () => {
    const branch = /case "sessionSwitched":([\s\S]*?)\n {4}case "/.exec(orchestrator)?.[1] ?? "";
    expect(branch).toMatch(
      /await ipc\.listMessages\(MESSAGE_LIMIT\);[\s\S]*?if \(state\.currentSessionId === event\.sessionId\)[\s\S]*?mergeFetchedMessages\(/,
    );
  });
});
