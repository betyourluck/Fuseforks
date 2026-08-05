/**
 * テーマの**取りこぼし**を機械で留める（2026-08-05）。
 *
 * ライトを足したときに一番起きやすい壊れ方は「1 トークンだけ暗いまま残る」。
 * 実害は**その色を使った場所だけが読めない**という形で出るので、画面を
 * 一通り見ても気づかない（`failures.md` #54 の「一部だけ動く状態は、
 * 全く動かない状態より診断が遅れる」と同じ性質）。
 *
 * ここで見るのは 2 点だけ:
 * - `@theme` の色トークンが**すべて**ライトで上書きされている
 * - 個体別の色（役職・アバター）の L/C が**両テーマで定義されている**
 *
 * **値の良し悪しは見ない。** 読みやすさは人が実機で判断するもので、
 * ここで閾値を検査すると「テストが通ったから読める」という誤った確信を作る
 * （`failures.md` #54 の教訓そのもの）。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

/** コメントを落とした `style.css`。散文の語を定義と読み違えないため。 */
const css: string = readFileSync(
  fileURLToPath(new URL("../style.css", import.meta.url)),
  "utf8",
).replace(/\/\*[\s\S]*?\*\//g, "");

/** `{` から対応する `}` までを素朴に切り出す（このファイルは入れ子を持たない）。 */
function block(head: RegExp): string {
  const start = css.search(head);
  expect(start, `${head} が見つからない`).toBeGreaterThan(-1);
  const open = css.indexOf("{", start);
  const close = css.indexOf("\n}", open);
  return css.slice(open, close);
}

const themeBlock = block(/^@theme\s*\{/m);
const lightBlock = block(/^:root\[data-theme="light"\]\s*\{/m);
const rootBlock = block(/^:root\s*\{/m);

/**
 * `:root` 側にだけ住む色トークン。**ユーティリティを生やさない**ので
 * `@theme` には置けない（Tailwind v4 が未使用として落とす — `failures.md` #54）。
 */
const ROOT_ONLY_COLORS = new Set(["--color-wordmark"]);

/** ブロック内で定義されているカスタムプロパティ名。 */
function declared(source: string): string[] {
  return [...source.matchAll(/^\s*(--[a-z0-9-]+):/gm)].map((m) => m[1]).sort();
}

describe("テーマ", () => {
  it("`@theme` の色トークンはすべてライトで上書きされている", () => {
    const dark = declared(themeBlock).filter((name) => name.startsWith("--color-"));
    const light = new Set(declared(lightBlock));

    expect(dark.length, "@theme に色トークンが無い").toBeGreaterThan(0);
    const missing = dark.filter((name) => !light.has(name));
    expect(
      missing,
      `ライトで上書きされていないトークンがある（その色を使った場所だけ暗いまま残る）: ${missing.join(", ")}`,
    ).toEqual([]);
  });

  it("ライトは `@theme` に無いトークンを増やさない", () => {
    // 片方にしか無い色は、もう一方のテーマでは「定義済みの別の値」に化ける。
    // 新しい色を足すなら **`@theme` が起点**（ユーティリティを生やす側）。
    const dark = new Set(declared(themeBlock));
    const strays = declared(lightBlock)
      .filter((name) => name.startsWith("--color-"))
      .filter((name) => !dark.has(name) && !ROOT_ONLY_COLORS.has(name));
    expect(strays, `@theme に対応の無い色トークン: ${strays.join(", ")}`).toEqual([]);
  });

  it("個体別の色（役職・アバター）の L/C が両テーマにある", () => {
    for (const name of ["--role-l", "--role-c", "--avatar-l", "--avatar-c"]) {
      expect(rootBlock, `${name} が :root に無い`).toContain(`${name}:`);
      expect(lightBlock, `${name} がライトに無い`).toContain(`${name}:`);
    }
  });

  it("役職色は L/C を直接書かず共有の変数を参照する", () => {
    // 8 色ぶん L/C を書き並べると、テーマを増やすたびに 8 箇所へ同じ値を
    // 書くことになり、**色ごとにずれる余地**が生まれる。参照にすれば
    // 「全色で揃っている」が検査ではなく構造になる。
    const roleDecls = [...rootBlock.matchAll(/--color-role-[a-z]+:\s*([^;]+);/g)].map(
      (m) => m[1].trim(),
    );
    expect(roleDecls).toHaveLength(8);
    for (const value of roleDecls) {
      expect(value, `L/C が直接書かれている: ${value}`).toMatch(
        /^oklch\(var\(--role-l\) var\(--role-c\) [\d.]+\)$/,
      );
    }
  });

  it("配色は `color-scheme` も切り替える（ネイティブ部品を置き去りにしない）", () => {
    // チェックボックス・ラジオ・select の矢印・既定のスクロールバーは
    // OS が描く。`color-scheme` を切り替えないと、ライトの画面に暗い部品が残る。
    expect(rootBlock).toContain("color-scheme: dark");
    expect(lightBlock).toContain("color-scheme: light");
  });
});
