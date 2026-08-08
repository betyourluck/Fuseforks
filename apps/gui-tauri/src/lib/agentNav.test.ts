/**
 * Alt + ↑↓ でサーヴァント一覧の選択を移す規則（2026-08-08・マイルストーン 8）。
 *
 * 留めるのは 3 点:
 * - **並びは `order`**（左ペインと同じ）。別々に整列すると画面の並びと違う順で飛ぶ
 * - **Alt だけが押されていること**まで見る（他の修飾との合わせ技を奪わない）
 * - **効かせる場所は閉じた許容** — `body` と入力欄だけ。除外リストにすると、
 *   入力面が増えるたびに黙って奪う側へ倒れる
 */
import { describe, expect, it } from "vitest";

import {
  agentNavDelta,
  inListOrder,
  isNavigableFocus,
  nextAgentId,
  type NavKeyEvent,
} from "./agentNav";

/** `order` が id の並びと**わざと食い違う**一覧。 */
const AGENTS = [
  { id: "agent_9", order: 0 },
  { id: "agent_2", order: 1 },
  { id: "agent_5", order: 2 },
];

function keys(over: Partial<NavKeyEvent>): NavKeyEvent {
  return { key: "ArrowDown", altKey: true, ctrlKey: false, metaKey: false, shiftKey: false, ...over };
}

describe("inListOrder", () => {
  it("order で並べる（id の順ではない）", () => {
    // id の昇順なら agent_2 が先頭になる。**order で並んでいることが読める入力**。
    expect(inListOrder(AGENTS).map((a) => a.id)).toEqual(["agent_9", "agent_2", "agent_5"]);
  });

  it("元の配列を壊さない", () => {
    const original = [...AGENTS];
    inListOrder(AGENTS);
    expect(AGENTS).toEqual(original);
  });
});

describe("nextAgentId", () => {
  it("下は order の次へ進む", () => {
    expect(nextAgentId(AGENTS, "agent_9", 1)).toBe("agent_2");
  });

  it("上は order の前へ戻る", () => {
    expect(nextAgentId(AGENTS, "agent_2", -1)).toBe("agent_9");
  });

  it("端で巻き戻る", () => {
    expect(nextAgentId(AGENTS, "agent_5", 1)).toBe("agent_9");
    expect(nextAgentId(AGENTS, "agent_9", -1)).toBe("agent_5");
  });

  it("未選択なら下で先頭・上で末尾から入る", () => {
    expect(nextAgentId(AGENTS, null, 1)).toBe("agent_9");
    expect(nextAgentId(AGENTS, null, -1)).toBe("agent_5");
  });

  it("一覧に無い id は未選択と同じ扱い", () => {
    // 削除された個体を選んだまま押しても、行き止まりにしない。
    expect(nextAgentId(AGENTS, "agent_404", 1)).toBe("agent_9");
  });

  it("一覧が空なら null", () => {
    expect(nextAgentId([], null, 1)).toBeNull();
  });
});

describe("agentNavDelta", () => {
  it("Alt + ↑↓ を拾う", () => {
    expect(agentNavDelta(keys({ key: "ArrowDown" }))).toBe(1);
    expect(agentNavDelta(keys({ key: "ArrowUp" }))).toBe(-1);
  });

  it("Alt が無ければ拾わない", () => {
    // 素の ↑↓ は入力欄のパス補完（Spec 24）が使っている。
    expect(agentNavDelta(keys({ altKey: false }))).toBeNull();
  });

  it("他の修飾が混ざったら拾わない", () => {
    // **1 つずつ見る。** まとめて 1 本にすると、どの修飾を無視したのか分からない。
    expect(agentNavDelta(keys({ ctrlKey: true }))).toBeNull();
    expect(agentNavDelta(keys({ metaKey: true }))).toBeNull();
    expect(agentNavDelta(keys({ shiftKey: true }))).toBeNull();
  });

  it("↑↓ 以外は拾わない", () => {
    for (const key of ["ArrowLeft", "ArrowRight", "Enter", "Tab", "a"]) {
      expect(agentNavDelta(keys({ key }))).toBeNull();
    }
  });
});

describe("isNavigableFocus", () => {
  /** 最小限の DOM 代役。`tagName` と `hasAttribute` しか見ていない。 */
  function element(tagName: string, attrs: string[] = []): Element {
    return {
      tagName,
      hasAttribute: (name: string) => attrs.includes(name),
    } as unknown as Element;
  }

  it("何もフォーカスしていなければ効く", () => {
    expect(isNavigableFocus(null)).toBe(true);
    expect(isNavigableFocus(element("BODY"))).toBe(true);
  });

  it("チャット入力欄では効く（要件の本体）", () => {
    expect(isNavigableFocus(element("TEXTAREA", ["data-chat-input"]))).toBe(true);
  });

  it("印の無い入力面では効かない", () => {
    // CodeMirror は既定の keymap で Alt+↑↓ に行の移動を持っている。
    // **除外リストではなく許容側で書いているので、印が無ければ既定で安全。**
    expect(isNavigableFocus(element("DIV", ["contenteditable"]))).toBe(false);
    expect(isNavigableFocus(element("INPUT"))).toBe(false);
    expect(isNavigableFocus(element("TEXTAREA"))).toBe(false);
  });
});
