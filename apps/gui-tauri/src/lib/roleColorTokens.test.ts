/**
 * 役職色の CSS 変数が**実際に定義されているか**を機械で確かめる。
 *
 * # なぜこのテストが要るか（2026-08-04 実機で判明）
 *
 * **Tailwind v4 の `@theme` は「使われていない変数」を出力から落とす。**
 * 役職色は `var(--color-role-${色})` と**実行時に組み立てて**使うので、
 * スキャナからは 1 つも使われていないように見え、全部消えた。
 *
 * 実機では白ばかりになり、**teal だけが残った** — `roleLabel.test.ts` に
 * `"var(--color-role-teal)"` という文字列リテラルがあり、それを Tailwind の
 * スキャナが「使用」と読んだせい。**テストが偶然 1 色だけ延命し、
 * かつ「色は動く」という誤った確信まで与えていた。**
 *
 * `roleLabel.test.ts` は `roleBadge` が**正しい文字列を返すこと**しか見ておらず、
 * その文字列が**指す先が存在するか**は見ていない。ここがその穴を塞ぐ。
 *
 * # 読む前にコメントを剥がす
 *
 * このファイル自身が最初その罠を踏んだ — `@theme` を素で検索すると、
 * **`@theme` について説明した CSS コメント**に当たる。検索語で位置を指すときは、
 * その語を含む散文が同じファイルに居ないかを疑う（`failures.md` #51 の一族）。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

/** `RoleColor`（Rust 側の列挙・`types.ts` の union）と 1 対 1。 */
const ROLE_COLORS = [
  "red",
  "orange",
  "amber",
  "green",
  "teal",
  "blue",
  "violet",
  "pink",
] as const;

/**
 * コメントを落とした `style.css`。散文に含まれる語を「定義」と読み違えないため。
 *
 * **`?raw` では読めない** — vitest は `.css` の取り込みを既定で空へ潰すので、
 * `import css from "../style.css?raw"` は空文字になり、**全色が「未定義」に
 * 見えるのに原因が分からない**テストになる（実際にそう書いて踏んだ）。
 */
const css: string = readFileSync(
  fileURLToPath(new URL("../style.css", import.meta.url)),
  "utf8",
).replace(/\/\*[\s\S]*?\*\//g, "");

describe("役職色の CSS 変数", () => {
  it.each(ROLE_COLORS)("--color-role-%s が定義されている", (color) => {
    expect(css).toContain(`--color-role-${color}:`);
  });

  it("`@theme` の中に置かない（Tailwind v4 が未使用として落とすため）", () => {
    const themeStart = css.search(/^@theme\b/m);
    expect(themeStart, "@theme ブロックが見つからない").toBeGreaterThan(-1);
    const themeEnd = css.indexOf("\n}", themeStart);
    expect(css.slice(themeStart, themeEnd)).not.toContain("--color-role-");
  });

  it("明度と彩度が全色で揃っている（読みやすさを構造で保証する）", () => {
    // 色相だけを変える規律。1 色でも L/C がずれると、その色だけ読みにくくなる。
    //
    // **2026-08-05 に検査から構造へ変えた。** 元は 8 色それぞれに書かれた
    // L/C の値が 1 種類であることを数えていたが、テーマを増やすと同じ値を
    // 2 セット書くことになり、ずれる余地が倍になる。共有の変数を参照する形なら
    // **ずれること自体ができない** — ここが見るのは「参照になっているか」だけ。
    // テーマ側の定義漏れは `theme.test.ts` が見る。
    const found = [
      ...css.matchAll(/--color-role-[a-z]+:\s*oklch\(var\(--role-l\) var\(--role-c\) [\d.]+\)/g),
    ];
    expect(
      found,
      "役職色が --role-l / --role-c を参照していない（L/C を直接書くとテーマごとにずれる）",
    ).toHaveLength(ROLE_COLORS.length);
  });
});
