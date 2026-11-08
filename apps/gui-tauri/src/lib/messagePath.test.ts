import { describe, expect, it } from "vitest";
import { renderMarkdown } from "./markdown";
import { parsePathRef } from "./messagePath";

describe("parsePathRef", () => {
  it("区切りのあるパスは拡張子を問わず受ける", () => {
    expect(parsePathRef("src/lib/foo.ts")).toEqual({ path: "src/lib/foo.ts", line: null });
    expect(parsePathRef("specs/")).toEqual({ path: "specs/", line: null });
    expect(parsePathRef("D:\\Github\\Fuseforks\\x.weird")).toEqual({
      path: "D:\\Github\\Fuseforks\\x.weird",
      line: null,
    });
    expect(parsePathRef("/usr/local/bin")).toEqual({ path: "/usr/local/bin", line: null });
  });

  it("末尾の :行 と :行:列 は行番号として取り出し、ドライブの C: には当たらない", () => {
    expect(parsePathRef("src/foo.ts:42")).toEqual({ path: "src/foo.ts", line: 42 });
    expect(parsePathRef("src/foo.ts:42:7")).toEqual({ path: "src/foo.ts", line: 42 });
    expect(parsePathRef("C:\\a\\b.md")).toEqual({ path: "C:\\a\\b.md", line: null });
  });

  it("区切りの無い名前は拡張子の許容に入るときだけ受ける", () => {
    expect(parsePathRef("world.json")).toEqual({ path: "world.json", line: null });
    expect(parsePathRef("CLAUDE.md:12")).toEqual({ path: "CLAUDE.md", line: 12 });
    expect(parsePathRef(".gitignore")).toEqual({ path: ".gitignore", line: null });
    // コードの識別子・版番号は受けない。
    expect(parsePathRef("state.typing")).toBeNull();
    expect(parsePathRef("v0.6.7")).toBeNull();
    expect(parsePathRef("0.25")).toBeNull();
  });

  it("空白は絶対パスのときだけ許す", () => {
    expect(parsePathRef("C:\\Program Files\\a b.txt")).toEqual({
      path: "C:\\Program Files\\a b.txt",
      line: null,
    });
    expect(parsePathRef("foo bar/baz")).toBeNull();
  });

  it("URL・コード・フラグは受けない", () => {
    expect(parsePathRef("https://example.com/a.md")).toBeNull();
    expect(parsePathRef("file:///C:/a.md")).toBeNull();
    expect(parsePathRef("Vec<String>")).toBeNull();
    expect(parsePathRef("foo::bar/baz")).toBeNull();
    expect(parsePathRef("fn(a/b)")).toBeNull();
    expect(parsePathRef("--out/dir")).toBeNull();
    expect(parsePathRef("1/2")).toBeNull();
    expect(parsePathRef("")).toBeNull();
  });
});

describe("Markdown のパスの印", () => {
  it("バッククォートのパスにだけ data-path が付く", () => {
    const html = renderMarkdown("see `src/foo.ts:42` and `state.typing`");
    expect(html).toContain('<code data-path="src/foo.ts:42">src/foo.ts:42</code>');
    expect(html).toContain("<code>state.typing</code>");
  });

  it("パスの形をしたリンクは <a> ではなく data-path の span になり、URL のリンクはそのまま", () => {
    const html = renderMarkdown("[file](src/foo.ts) and [web](https://x.example)");
    expect(html).toContain('<span class="md-path" data-path="src/foo.ts">file</span>');
    expect(html).toContain('<a href="https://x.example"');
    expect(html).not.toContain('href="src/foo.ts"');
  });

  it("パスでもリンクでもない行き先は従来どおり文字のまま", () => {
    const html = renderMarkdown("[x](javascript:alert(1)) [y](fuseforks://a)");
    expect(html).not.toContain("<a ");
    expect(html).not.toContain("data-path");
  });

  it("属性の中の引用符はエスケープされる", () => {
    // バッククォートの中身はパスの判定で `"` を弾くので、印は付かない（属性を壊す経路が無い）。
    expect(renderMarkdown('`a"b/c.md`')).not.toContain("data-path");
  });
});
