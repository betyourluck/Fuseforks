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
