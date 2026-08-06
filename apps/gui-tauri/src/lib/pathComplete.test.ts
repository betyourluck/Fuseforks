import { describe, expect, it } from "vitest";

import {
  MAX_SUGGESTIONS,
  applyCompletion,
  findTrigger,
  rankCandidates,
  splitForDisplay,
  type Candidate,
} from "./pathComplete";

/** テスト用の候補を作る（v1 の種別は file だけ）。 */
function files(...paths: string[]): Candidate[] {
  return paths.map((id) => ({ id, kind: "file" as const }));
}

describe("findTrigger", () => {
  it("行頭の @ で開き、クエリは @ の直後からカーソルまで", () => {
    expect(findTrigger("@spec", 5)).toEqual({ at: 0, query: "spec" });
  });

  it("@ を打った直後はクエリが空でも開く", () => {
    expect(findTrigger("@", 1)).toEqual({ at: 0, query: "" });
  });

  it("空白の後の @ でも開く", () => {
    // 「これを見て 」= 6 文字なので `@` は index 6、カーソルは末尾の 11。
    expect(findTrigger("これを見て @spec", 11)).toEqual({ at: 6, query: "spec" });
  });

  it("単語の直後の @ では開かない（メールアドレスを邪魔しない）", () => {
    expect(findTrigger("user@example.com", 16)).toBeNull();
  });

  it("クエリに空白が入ったら閉じる（@ を含む普通の文章が通る）", () => {
    // S5 の担保。「@ で始まる語」を書いても補完に食われない。
    expect(findTrigger("@spec を見て", 8)).toBeNull();
    expect(findTrigger("@ とは何か", 6)).toBeNull();
  });

  it("カーソルより後ろは見ない", () => {
    // 「@a」まで打って、その後ろに既存の文字列がある状態。
    expect(findTrigger("@ab", 2)).toEqual({ at: 0, query: "a" });
  });

  it("@ が無ければ開かない", () => {
    expect(findTrigger("ふつうの本文", 6)).toBeNull();
  });

  it("複数の @ があれば直近のものを取る", () => {
    expect(findTrigger("@one @tw", 8)).toEqual({ at: 5, query: "tw" });
  });
});

describe("rankCandidates", () => {
  const all = files(
    "specs/01_sd-yq-write-tools.md",
    "specs/24_path-completion.md",
    "src/spec_helper.ts",
    "docs/readme.md",
  );

  it("クエリが空なら先頭から返す（@ を打った直後）", () => {
    const ranked = rankCandidates(all, "");
    expect(ranked).toHaveLength(4);
    expect(ranked[0].candidate.id).toBe("specs/01_sd-yq-write-tools.md");
  });

  it("ファイル名の前方一致が、部分一致より上（D5）", () => {
    // "01_…" はファイル名の先頭一致、"spec_helper" もファイル名の先頭一致。
    // "specs/…" はディレクトリ一致なので下。
    const ranked = rankCandidates(all, "spec");
    expect(ranked[0].candidate.id).toBe("src/spec_helper.ts");
  });

  it("ファイル名で当たらなければパス全体の部分一致で拾う", () => {
    const ranked = rankCandidates(files("specs/24_path-completion.md"), "specs");
    expect(ranked).toHaveLength(1);
    // ディレクトリで当てたので、ファイル名の中の強調位置は無い。
    expect(ranked[0].matchIndex).toBe(-1);
  });

  it("ファイル名だけ打って正しい 1 本に当たる（実測の使い方）", () => {
    const ranked = rankCandidates(all, "01_sd");
    expect(ranked[0].candidate.id).toBe("specs/01_sd-yq-write-tools.md");
  });

  it("大文字小文字を無視する", () => {
    expect(rankCandidates(all, "README")[0].candidate.id).toBe("docs/readme.md");
  });

  it("一致しない候補は落ちる", () => {
    expect(rankCandidates(all, "zzz")).toHaveLength(0);
  });

  it("同点はパスの辞書順で破る（並びが打鍵ごとに揺れない）", () => {
    // どちらもファイル名の先頭一致 = 同じ段・同じ位置。
    const ranked = rankCandidates(files("b/a.md", "a/a.md"), "a.md");
    expect(ranked.map((r) => r.candidate.id)).toEqual(["a/a.md", "b/a.md"]);
  });

  it("上限で切る", () => {
    const many = files(...Array.from({ length: 50 }, (_, i) => `f${i}.md`));
    expect(rankCandidates(many, "f")).toHaveLength(MAX_SUGGESTIONS);
    expect(rankCandidates(many, "f", 3)).toHaveLength(3);
  });
});

describe("splitForDisplay", () => {
  it("ファイル名とフォルダへ割る（表示は basename が主）", () => {
    expect(splitForDisplay("specs/24_path-completion.md")).toEqual({
      base: "24_path-completion.md",
      dir: "specs",
    });
  });

  it("深い階層はフォルダ側にまとめる", () => {
    expect(splitForDisplay("crates/agent-core/src/lib.rs")).toEqual({
      base: "lib.rs",
      dir: "crates/agent-core/src",
    });
  });

  it("直下のファイルはフォルダが空", () => {
    expect(splitForDisplay("README.md")).toEqual({ base: "README.md", dir: "" });
  });
});

describe("applyCompletion", () => {
  it("@ を残したまま相対パスを挿し、末尾に空白を足す", () => {
    const text = "@spec";
    const trigger = findTrigger(text, 5)!;
    const result = applyCompletion(text, trigger, 5, {
      id: "specs/24_path-completion.md",
      kind: "file",
    });

    expect(result.text).toBe("@specs/24_path-completion.md ");
    expect(result.caret).toBe(result.text.length);
  });

  it("確定した瞬間に補完が閉じる（末尾の空白が findTrigger を切る）", () => {
    const text = "@spec";
    const trigger = findTrigger(text, 5)!;
    const result = applyCompletion(text, trigger, 5, {
      id: "docs/readme.md",
      kind: "file",
    });

    // 閉じる処理を別に書かなくても、次の検出が null になる。
    expect(findTrigger(result.text, result.caret)).toBeNull();
  });

  it("前後の本文を壊さない", () => {
    const text = "これ @spec を見て";
    const caret = 8; // 「@spec」の直後
    const trigger = findTrigger(text, caret)!;
    const result = applyCompletion(text, trigger, caret, {
      id: "docs/readme.md",
      kind: "file",
    });

    expect(result.text).toBe("これ @docs/readme.md  を見て");
  });
});
