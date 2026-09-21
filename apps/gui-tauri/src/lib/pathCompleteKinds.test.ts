/**
 * 補完の種別は「語の頭に並んだ `@` の個数」で決まる
 * （Spec 58 D1 / `path_completion_contract` 凍結 6）。
 *
 * `@` のファイル補完の既存の規則は `pathComplete.test.ts` が留めている。
 * ここは種別の判定と、Spec 58 で変わった 1 点（クエリの中の `@`）だけを見る。
 */
import { describe, expect, it } from "vitest";

import { findTrigger, removeTrigger } from "./pathComplete";

const at = (text: string) => findTrigger(text, text.length);

describe("findTrigger の種別", () => {
  it("`@` 1 個はファイル、2 個は会話の参照", () => {
    expect(at("@spec")).toEqual({ at: 0, query: "spec", kind: "file" });
    expect(at("@@ジェミー")).toEqual({ at: 0, query: "ジェミー", kind: "message" });
    expect(at("@@")).toEqual({ at: 0, query: "", kind: "message" });
  });

  it("3 個以上は何の入口でもない", () => {
    expect(at("@@@")).toBeNull();
    expect(at("@@@foo")).toBeNull();
    expect(at("@@@@")).toBeNull();
  });

  it("入口の条件は並びの先頭に掛かる（行頭・空白・開き括弧の直後）", () => {
    expect(at("これを見て @@SQL")).toEqual({ at: 6, query: "SQL", kind: "message" });
    expect(at("（@@ルナ")).toEqual({ at: 1, query: "ルナ", kind: "message" });
    expect(at("改行\n@@ル")).toEqual({ at: 3, query: "ル", kind: "message" });
    // 単語文字の直後は入口ではない（メールアドレスの途中で開かない）。
    expect(at("user@@example")).toBeNull();
    expect(at("user@example.com")).toBeNull();
  });

  it("クエリの中の `@` は、クエリの一部として読む", () => {
    // Spec 58 より前は `lastIndexOf("@")` だったので、ここで補完が閉じていた。
    expect(at("@@user@example")).toEqual({ at: 0, query: "user@example", kind: "message" });
    expect(at("@node_modules/@types")).toEqual({
      at: 0,
      query: "node_modules/@types",
      kind: "file",
    });
  });

  it("語の中に入口が 2 つあれば、後ろのほうを採る", () => {
    // `(` の直後は入口。`lastIndexOf` の頃と同じ答えになる。
    expect(at("@a(@b")).toEqual({ at: 3, query: "b", kind: "file" });
    expect(at("@a(@@b")).toEqual({ at: 3, query: "b", kind: "message" });
    // クエリの中の開き括弧だけでは、語は切れない。
    expect(at("@docs/file(1")).toEqual({ at: 0, query: "docs/file(1", kind: "file" });
  });

  it("空白を打ったら、どちらの種別も閉じる", () => {
    expect(at("@@ジェミー ")).toBeNull();
    expect(at("@@ジェミー SQL")).toBeNull();
    expect(at("@spec ")).toBeNull();
  });

  it("打っている途中で種別が入れ替わる（1 個目はファイル、2 個目で参照）", () => {
    expect(findTrigger("@@", 1)?.kind).toBe("file");
    expect(findTrigger("@@", 2)?.kind).toBe("message");
  });
});

describe("removeTrigger", () => {
  it("打ちかけのクエリごと消し、前後の文面は残す", () => {
    const text = "これを踏まえて @@ジェミ 答えて";
    const caret = "これを踏まえて @@ジェミ".length;
    const trigger = findTrigger(text, caret)!;
    const result = removeTrigger(text, trigger, caret);
    expect(result.text).toBe("これを踏まえて  答えて");
    expect(result.caret).toBe("これを踏まえて ".length);
    // 消した後は補完が開かない。
    expect(findTrigger(result.text, result.caret)).toBeNull();
  });

  it("本文がクエリだけなら空になる", () => {
    const trigger = findTrigger("@@", 2)!;
    expect(removeTrigger("@@", trigger, 2)).toEqual({ text: "", caret: 0 });
  });
});
