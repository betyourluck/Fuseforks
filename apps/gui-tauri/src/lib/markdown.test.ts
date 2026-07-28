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

describe("renderMarkdownCached", () => {
  it("同じ ID は再 parse せず同一の結果を返す", () => {
    const first = renderMarkdownCached("msg-1", "# a");
    const second = renderMarkdownCached("msg-1", "変わっても無視される");
    expect(second).toBe(first);
  });
});
