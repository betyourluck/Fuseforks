import { describe, expect, it } from "vitest";

import { answerNotice, isOpenSession, NO_SESSION, sessionOfLabel } from "./sessionScope";

describe("会話の単位で絞る規則（Spec 75 D10）", () => {
  it("同じ会話だけが開いている会話", () => {
    expect(isOpenSession("session_1", "session_1")).toBe(true);
    expect(isOpenSession("session_9", "session_1")).toBe(false);
  });

  it("会話を持たないもの（保存先の無い村の印・欄の無い記録）は開いている扱い", () => {
    // 保存先の無い村では開いている会話の ID が空で、イベントの印は "-"。
    // 「別の会話」と出すと、その村では全部が「別の会話」になる。
    expect(isOpenSession(NO_SESSION, "")).toBe(true);
    expect(isOpenSession(NO_SESSION, "session_1")).toBe(true);
    expect(isOpenSession(null, "session_1")).toBe(true);
    expect(isOpenSession(undefined, "session_1")).toBe(true);
  });

  it("イベントの印を記録の形へ（\"-\" は null）", () => {
    expect(sessionOfLabel("-")).toBeNull();
    expect(sessionOfLabel("session_9")).toBe("session_9");
  });

  it("知らせは 1 件目と 2 件目以降で文を変え、件数を数え上げる", () => {
    expect(answerNotice(null)).toEqual({ key: "orchestrator.answerElsewhere", count: 1 });
    expect(answerNotice(1)).toEqual({ key: "orchestrator.answerElsewhereMany", count: 2 });
    expect(answerNotice(4).count).toBe(5);
  });
});
