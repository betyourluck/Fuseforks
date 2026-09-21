import { describe, expect, it } from "vitest";

import type { AgentMessage, Endpoint } from "../types";

import {
  MAX_QUOTES,
  QUOTE_HEAD_CHARS,
  addChip,
  chipOf,
  countChars,
  quoteCandidates,
  rankQuotes,
  restoreDraft,
  type QuoteChip,
} from "./quoteRef";

const USER: Endpoint = { kind: "user" };
const agent = (id: string): Endpoint => ({ kind: "agent", id });

function message(id: string, from: Endpoint, to: Endpoint, content: string, tsMs = 0): AgentMessage {
  return { id, from, to, content, tokens: 0, tsMs, hop: 0 };
}

const NAMES: Record<string, string> = { agent_3: "ジェミー", agent_5: "ルナ" };
const nameOf = (id: string): string | null => NAMES[id] ?? null;

const LOG: AgentMessage[] = [
  message("u1", USER, agent("agent_3"), "SQL を調べて", 1),
  message("a1", agent("agent_3"), USER, "SQL の結論は JOIN です", 2),
  message("s1", { kind: "system" }, USER, "ルナが入室しました", 3),
  message("a2", agent("agent_5"), agent("agent_3"), "委譲の答え\n  2 行目", 4),
  message("e1", { kind: "external", client: "cli" }, agent("agent_3"), "外部の依頼", 5),
  message("a3", agent("agent_gone"), USER, "削除済みの個体の答え", 6),
];

describe("quoteCandidates", () => {
  it("サーヴァントが書いた発話だけを、新しい順に出す", () => {
    const ids = quoteCandidates(LOG, nameOf).map((c) => c.id);
    // 利用者発（u1）・System（s1）・外部発（e1）は出ない。
    expect(ids).toEqual(["a3", "a2", "a1"]);
  });

  it("宛先は問わない（委譲の答えも出る）", () => {
    const a2 = quoteCandidates(LOG, nameOf).find((c) => c.id === "a2");
    expect(a2?.to).toEqual(agent("agent_3"));
    expect(a2?.fromName).toBe("ルナ");
  });

  it("削除済みの個体は id で出す", () => {
    expect(quoteCandidates(LOG, nameOf)[0].fromName).toBe("agent_gone");
  });

  it("本文の先頭は 1 行へ潰し、字数は code point で数える", () => {
    const a2 = quoteCandidates(LOG, nameOf).find((c) => c.id === "a2");
    expect(a2?.head).toBe("委譲の答え 2 行目");
    expect(a2?.chars).toBe(countChars("委譲の答え\n  2 行目"));

    const long = message("x", agent("agent_3"), USER, "𠮷".repeat(QUOTE_HEAD_CHARS + 5));
    const c = quoteCandidates([long], nameOf)[0];
    expect(countChars(c.head)).toBe(QUOTE_HEAD_CHARS);
    expect(c.chars).toBe(QUOTE_HEAD_CHARS + 5);
  });
});

describe("rankQuotes", () => {
  const candidates = quoteCandidates(LOG, nameOf);

  it("クエリが空なら新しい順のまま", () => {
    expect(rankQuotes(candidates, "").map((c) => c.id)).toEqual(["a3", "a2", "a1"]);
    expect(rankQuotes(candidates, "", 2)).toHaveLength(2);
  });

  it("表示名の前方一致 → 表示名の部分一致 → 本文の部分一致", () => {
    // 「ル」: ルナは前方一致。本文に「ル」を含む発話は無い。
    expect(rankQuotes(candidates, "ル").map((c) => c.id)).toEqual(["a2"]);
    // 「ミ」: ジェミーは部分一致（段 2）。
    expect(rankQuotes(candidates, "ミ").map((c) => c.id)).toEqual(["a1"]);
    // 「答え」: 表示名には無く、本文で 2 件。同点は新しい順。
    expect(rankQuotes(candidates, "答え").map((c) => c.id)).toEqual(["a3", "a2"]);
  });

  it("表示名の一致は、新しい本文の一致より上に来る", () => {
    const log = [
      message("old", agent("agent_3"), USER, "古い答え", 1),
      message("new", agent("agent_5"), USER, "ジェミーに聞いた話", 2),
    ];
    // `new` のほうが新しいが、`old` は送り手が「ジェミー」。
    expect(rankQuotes(quoteCandidates(log, nameOf), "ジェミー").map((c) => c.id)).toEqual([
      "old",
      "new",
    ]);
  });

  it("大文字小文字を無視し、一致しなければ落とす", () => {
    expect(rankQuotes(candidates, "sql").map((c) => c.id)).toEqual(["a1"]);
    expect(rankQuotes(candidates, "どこにも無い語")).toEqual([]);
  });
});

describe("addChip", () => {
  const chip = (id: string): QuoteChip => ({ id, fromName: "ジェミー", tsMs: 0, chars: 1 });

  it("同じ発話は 1 件", () => {
    const one = addChip([], chip("a"));
    expect(addChip(one, chip("a"))).toBe(one);
  });

  it("上限を超えて足せない", () => {
    let chips: readonly QuoteChip[] = [];
    for (let i = 0; i < MAX_QUOTES + 2; i++) chips = addChip(chips, chip(`m${i}`));
    expect(chips.map((c) => c.id)).toEqual(["m0", "m1", "m2"]);
  });

  it("候補からチップへ写す", () => {
    const c = quoteCandidates(LOG, nameOf)[2];
    expect(chipOf(c)).toEqual({ id: "a1", fromName: "ジェミー", tsMs: 2, chars: c.chars });
  });
});

describe("restoreDraft", () => {
  const chip = (id: string): QuoteChip => ({ id, fromName: "ジェミー", tsMs: 0, chars: 1 });
  const empty = { text: "", attachment: null, quotes: [] };

  it("空の入力欄へは、失敗した下書きをそのまま戻す", () => {
    const failed = { text: "依頼文", attachment: "画像", quotes: [chip("a")] };
    expect(restoreDraft(empty, failed)).toEqual(failed);
  });

  it("待っている間に打たれた文を上書きしない", () => {
    const current = { text: "次の文", attachment: null, quotes: [chip("b")] };
    const failed = { text: "依頼文", attachment: "画像", quotes: [chip("a")] };
    const restored = restoreDraft(current, failed);
    expect(restored.text).toBe("依頼文\n次の文");
    expect(restored.attachment).toBe("画像");
    expect(restored.quotes.map((c) => c.id)).toEqual(["a", "b"]);
  });

  it("貼り直された添付は新しいほうを残し、参照は上限までで重複させない", () => {
    const current = { text: "", attachment: "新", quotes: [chip("a"), chip("c"), chip("d")] };
    const failed = { text: "", attachment: "旧", quotes: [chip("a"), chip("b")] };
    const restored = restoreDraft(current, failed);
    expect(restored.attachment).toBe("新");
    expect(restored.quotes.map((c) => c.id)).toEqual(["a", "b", "c"]);
    expect(restored.text).toBe("");
  });
});
