import { describe, expect, it } from "vitest";

import { renderMarkdown, renderMarkdownCached } from "./markdown";

describe("renderMarkdown", () => {
  it("見出し・リスト・コードを HTML へ描画する", () => {
    const html = renderMarkdown("# 結論\n\n- 項目1\n- 項目2\n\n`code` と **強調**");
    expect(html).toContain("<h1>結論</h1>");
    expect(html).toContain("<li>項目1</li>");
    expect(html).toContain("<code>code</code>");
    expect(html).toContain("<strong>強調</strong>");
  });

  it("フェンスコードブロックを pre/code で描画する", () => {
    const html = renderMarkdown("```rust\nfn main() {}\n```");
    expect(html).toContain("<pre>");
    expect(html).toContain("fn main() {}");
  });

  it("単独の改行も改行として見える（従来の pre-wrap 表示からの退行を防ぐ）", () => {
    const html = renderMarkdown("1行目\n2行目");
    expect(html).toContain("<br");
  });

  it("生の HTML はタグとして解釈せずエスケープする（LLM 出力は信頼しない）", () => {
    const html = renderMarkdown('<script>alert(1)</script><img src=x onerror=alert(1)>');
    expect(html).not.toContain("<script>");
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;script&gt;");
  });

  it("javascript: スキームのリンクはリンクにしない", () => {
    const html = renderMarkdown("[click](javascript:alert(1))");
    expect(html).not.toContain('href="javascript:');
  });

  it("http リンクは新しいウィンドウ扱いの属性を持つ（webview 内で遷移しない保険）", () => {
    const html = renderMarkdown("[docs](https://example.com)");
    expect(html).toContain('href="https://example.com"');
    expect(html).toContain('target="_blank"');
    expect(html).toContain('rel="noopener noreferrer"');
  });
});

describe("コードブロックのコピーボタン（2026-10-15）", () => {
  const fenced = "```rust\nfn main() {}\n```";

  it("ラベルを渡さなければ出力は従来どおり（包みもボタンも生えない）", () => {
    const html = renderMarkdown(fenced);
    expect(html).not.toContain("code-block");
    expect(html).not.toContain("data-code-copy");
    expect(renderMarkdown(fenced, {})).toBe(html);
  });

  it("ラベルを渡すと pre を包んでボタンを添える。pre 自体は既定の描画のまま", () => {
    const plain = renderMarkdown(fenced);
    const html = renderMarkdown(fenced, { codeCopyLabel: "コードをコピー" });
    expect(html).toContain(`<div class="code-block">${plain.trimEnd()}`);
    expect(html).toContain('<button type="button" class="code-copy" data-code-copy');
    expect(html).toContain('aria-label="コードをコピー"');
  });

  it("インデントコードにも同じボタンが付く", () => {
    const html = renderMarkdown("本文\n\n    indented()\n", { codeCopyLabel: "Copy" });
    expect(html).toContain("code-block");
    expect(html).toContain("indented()");
  });

  it("ラベルは属性として安全に埋め込む（引用符・タグをエスケープ）", () => {
    const html = renderMarkdown(fenced, { codeCopyLabel: 'a"b<c>' });
    expect(html).toContain('aria-label="a&quot;b&lt;c&gt;"');
    expect(html).not.toContain('aria-label="a"b');
  });

  it("インラインの code には付かない", () => {
    const html = renderMarkdown("`x` だけ", { codeCopyLabel: "Copy" });
    expect(html).not.toContain("code-block");
  });
});

describe("renderMarkdownCached", () => {
  it("同じ ID は再 parse せず同一の結果を返す", () => {
    const first = renderMarkdownCached("msg-1", "# a");
    const second = renderMarkdownCached("msg-1", "変わっても無視される");
    expect(second).toBe(first);
  });

  it("ラベルが変わったら描き直す（言語の切り替えで古いラベルのボタンを残さない）", () => {
    const fenced = "```\nx\n```";
    const ja = renderMarkdownCached("msg-2", fenced, { codeCopyLabel: "コードをコピー" });
    expect(ja).toContain('aria-label="コードをコピー"');
    const en = renderMarkdownCached("msg-2", fenced, { codeCopyLabel: "Copy code" });
    expect(en).toContain('aria-label="Copy code"');
    expect(en).not.toContain("コードをコピー");
    // 同じラベルなら再 parse しない。
    expect(renderMarkdownCached("msg-2", "別の本文", { codeCopyLabel: "Copy code" })).toBe(en);
  });
});

/**
 * 数式（2026-10-24）。区切りの判定は `lib/markdownMath.ts`、描画は KaTeX の `renderToString`。
 * 出力の形（`.katex` / `.katex-display`）を留めるのは、描画を外しても `$` が文字として残る =
 * 「壊れていない」と「描けていない」が区別できないため（変異で赤になる側を置く）。
 */
describe("数式（2026-10-24）", () => {
  const dollars = (s: string) => (s.match(/\$/g) ?? []).length;

  it("$…$ は行内の数式、$$…$$ は表示数式として KaTeX の HTML になる", () => {
    const inline = renderMarkdown("二乗は $x^2$ です");
    expect(inline).toContain('class="katex"');
    expect(inline).not.toContain("katex-display");
    expect(inline).not.toContain("$x^2$");

    const block = renderMarkdown("$$\n\\sum_{i=1}^{n} i\n$$");
    expect(block).toContain('class="katex-display"');
    expect(block).not.toContain("<p>");
  });

  it("```math と \\(…\\) \\[…\\] も数式。```math はコードブロックにもコピーの包みにもしない", () => {
    const fence = renderMarkdown("```math\n\\frac{1}{2}\n```", { codeCopyLabel: "Copy" });
    expect(fence).toContain("katex-display");
    expect(fence).not.toContain("<pre>");
    expect(fence).not.toContain("code-block");
    expect(renderMarkdown("行内 \\(a_1\\) です")).toContain('class="katex"');
    expect(renderMarkdown("\\[\nE = mc^2\n\\]")).toContain("katex-display");
  });

  it("数式の中の _ と * を Markdown の強調にしない（$`…`$ も）", () => {
    const html = renderMarkdown("$a_1 * b_2 * c_3$ と $`x_i * y_j`$");
    expect(html).not.toMatch(/<em>|<strong>/);
    expect(html).not.toContain("<code>");
    expect((html.match(/class="katex"/g) ?? []).length).toBe(2);
  });

  it("金額の $ は数式にせず文字として残す", () => {
    for (const line of [
      "usage credits（$10/100万 input・$50/100万 output）の従量になる",
      "| Standard | $1.25 | $4.25 | 3,000 RPM |",
      "概算約$0.050、修正版が$0.0477でした。",
      "US$5 と US$6",
      "$ x$ と $x $",
      "パスは a${b}c${d} で組む",
      "$$ は閉じない",
    ]) {
      const html = renderMarkdown(line);
      expect(html, line).not.toContain("katex");
      expect(dollars(html), line).toBe(dollars(line));
    }
  });

  it("金額と数式が同じ行でも、金額から数式の閉じまでを 1 つの数式にしない", () => {
    const html = renderMarkdown("料金は $10/100万 input・$50/100万 output。式 $\\frac{a}{b}$ を見よ");
    expect(html).toContain("$10/100万 input・$50/100万 output");
    expect((html.match(/class="katex"/g) ?? []).length).toBe(1);
  });

  it("コードの中の $ は触らない。\\$ はリテラルの $", () => {
    const html = renderMarkdown("`$x$` と\n\n```bash\necho $HOME $x$\n```\n\n\\$5 と \\$6");
    expect(html).not.toContain("katex");
    expect(html).toContain("<code>$x$</code>");
    expect(html).toContain("echo $HOME $x$");
    expect(html).toContain("$5 と $6");
  });

  it("不正な TeX は例外にせず、色はテーマのトークンで出す（生の #cc0000 を持ち込まない）", () => {
    const html = renderMarkdown("式 $\\frac{a$ はエラー表示");
    expect(html).toContain("katex-error");
    expect(html).toContain("var(--color-warn)");
    expect(html).not.toContain("#cc0000");
  });

  it("\\href などの信頼が要るコマンドは効かない（trust: false）", () => {
    const html = renderMarkdown("$\\href{https://evil.example}{x}$");
    expect(html).not.toContain("https://evil.example");
    expect(html).not.toContain("<a ");
  });
});
