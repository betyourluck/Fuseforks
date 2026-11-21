//! 本文の取り出し（Spec 78 D2 / `web_fetch_contract` 5〜7）。純関数（CPU だけ。ネットワークを知らない）。
//!
//! HTML は dom_smoothie（Mozilla Readability の移植）で本文を取り、本文の HTML（`Article.content`）を
//! dom_query で読み直して `md()` に渡す。`links: false` なら `md()` の前に `<a>` を外して文字だけ残し、
//! 画像と動画を消す — **URL を落とすのは DOM の上で、Markdown を正規表現で削らない**。`links: true` も
//! 同じ経路を通す（1 つの実装で両方を出す。dom_smoothie 自身の Markdown と 26 本中 25 本で同じ字数 — P0a）。
//!
//! dom_smoothie が失敗したか本文が空のときだけ、`<body>` 全体を `md()` に渡す（ページ全体）。**比で切り替えない**
//! — 比では一覧の取りこぼし（0.103）と記事の成功（0.147 から）を分けられない（P0a）。比は出すだけ。

use dom_query::Document;
use dom_smoothie::{Config, Readability};

/// どこから取ったか（ログの `mode=` と、枠の「取り出し:」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// dom_smoothie の本文。
    Readability,
    /// `<body>` 全体（dom_smoothie が失敗したか空）。
    Body,
    /// どちらも空（`<frameset>` のページなど）。
    Empty,
}

/// 取り出した結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted {
    /// どこから取ったか。
    pub mode: Mode,
    /// 題名（取れたときだけ）。
    pub title: Option<String>,
    /// 本文の Markdown（`Empty` なら空）。
    pub markdown: String,
    /// 本文のテキストが `<body>` のテキストの何 %（整数・四捨五入）。`Readability` のときだけ。
    pub share: Option<u32>,
}

/// `<body>` 全体へ戻るとき、中身ごと飛ばす要素（`md()` の `skip_tags`）。
const BODY_SKIP: [&str; 13] = [
    "script", "style", "meta", "head", "nav", "header", "footer", "aside", "form", "noscript", "template", "svg", "iframe",
];

/// 比の分母から外す要素（テキストを持つが本文ではないもの）。
const TEXT_SKIP: &str = "script, style, noscript, template, svg";

/// `links: false` で消す要素（画像と動画。`<a>` は外して文字を残す）。
const MEDIA: &str = "img, picture, source, video, audio";

fn visible_chars(s: &str) -> usize {
    s.chars().filter(|c| !c.is_whitespace()).count()
}

/// `links: false` なら、リンクを外して文字だけ残し、画像と動画を消す。
fn drop_links(doc: &Document) {
    doc.select(MEDIA).remove();
    doc.root().strip_elements(&["a"]);
}

fn body_text_chars(html: &str) -> usize {
    let doc = Document::from(html);
    doc.select(TEXT_SKIP).remove();
    visible_chars(&doc.select("body").text())
}

/// 百分率（整数・四捨五入）。分母が 0 なら `None`。
fn percent(part: usize, whole: usize) -> Option<u32> {
    (whole > 0).then(|| ((part as f64 / whole as f64) * 100.0).round() as u32)
}

fn page_title(doc: &Document) -> Option<String> {
    let title = doc.select("title").text().trim().to_owned();
    (!title.is_empty()).then_some(title)
}

/// HTML から本文を取り出す（D2）。`url` は相対リンクを絶対にするのに使う（dom_smoothie が読む）。
pub fn extract_html(html: &str, url: &str, links: bool) -> Extracted {
    let article = Readability::new(html, Some(url), Some(Config::default())).and_then(|mut r| r.parse());
    if let Ok(article) = article {
        let doc = Document::from(article.content.as_ref());
        let article_chars = visible_chars(&doc.root().text());
        if article_chars > 0 {
            let share = percent(article_chars, body_text_chars(html));
            if !links {
                drop_links(&doc);
            }
            let markdown = doc.root().md(None).trim().to_owned();
            let markdown = if links { unescape_destinations(&markdown) } else { markdown };
            let title = Some(article.title.trim().to_owned()).filter(|t| !t.is_empty());
            return Extracted { mode: Mode::Readability, title, markdown, share };
        }
    }

    // ページ全体（dom_smoothie が失敗したか、本文が空だった）。
    let doc = Document::from(html);
    let title = page_title(&doc);
    if !links {
        drop_links(&doc);
    }
    let body = doc.select("body");
    let markdown = body
        .nodes()
        .first()
        .map(|node| node.md(Some(&BODY_SKIP[..])).trim().to_owned())
        .unwrap_or_default();
    if visible_chars(&markdown) == 0 {
        return Extracted { mode: Mode::Empty, title, markdown: String::new(), share: None };
    }
    let markdown = if links { unescape_destinations(&markdown) } else { markdown };
    Extracted { mode: Mode::Body, title, markdown, share: None }
}

/// Markdown のリンクと画像の行き先（`](` の直後から対応する `)` まで。`<…>` で囲まれた形も）の中だけで、
/// `&amp;` → `&`・`&quot;` → `"`・`&nbsp;` → NBSP を**左から 1 回だけ**戻す（D2 / 契約 7）。
///
/// dom_query 0.29 の `push_md_url` は行き先にだけ `html_escape` を掛け、`&` を例外なく書き換えるので、
/// 行き先の中の `&` は必ず 3 つのどれかの頭で、この 1 回で元の URL に戻る。**地の文は触らない**
/// （`&amp;` という文字列そのものを書いた本文を壊さない）。囲みのコードブロック（``` / ~~~）の中も触らない。
pub fn unescape_destinations(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut in_fence = false;
    for line in md.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            out.push_str(line);
            continue;
        }
        if in_fence || !line.contains("](") {
            out.push_str(line);
            continue;
        }
        out.push_str(&unescape_line(line));
    }
    out
}

fn unescape_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(at) = rest.find("](") {
        let (before, after) = rest.split_at(at + 2);
        out.push_str(before);
        let end = destination_end(after);
        out.push_str(&unescape_entities(&after[..end]));
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// 行き先の終わり（`)` の手前。`<…>` なら `>` まで含める）。
fn destination_end(s: &str) -> usize {
    if s.starts_with('<') {
        return s.find('>').map_or(s.len(), |i| i + 1);
    }
    let mut depth = 0i32;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => return i,
            ')' => depth -= 1,
            '\n' => return i,
            _ => {}
        }
    }
    s.len()
}

fn unescape_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let (replacement, len) = if tail.starts_with("&amp;") {
            ("&", 5)
        } else if tail.starts_with("&quot;") {
            ("\"", 6)
        } else if tail.starts_with("&nbsp;") {
            ("\u{a0}", 6)
        } else {
            ("&", 1)
        };
        out.push_str(replacement);
        rest = &tail[len..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P0a で見た形を写した合成の記事（他人のページを repo に置かない）。ナビが多く、本文は 1 つの記事。
    fn article_page() -> String {
        let nav: String = (0..40).map(|i| format!("<li><a href=\"/c/{i}\">カテゴリ {i}</a></li>")).collect();
        let paras: String = (0..8)
            .map(|i| {
                format!(
                    "<p>第 {i} 段落。村の黒板と作業フォルダの話を続ける。\
                     詳しくは<a href=\"/doc?a=1&amp;b={i}\">手引き</a>を見る。ここには十分な長さの文章がある。</p>"
                )
            })
            .collect();
        format!(
            "<html><head><title>村の記事</title></head><body><nav><ul>{nav}</ul></nav>\
             <article><h1>村の記事</h1>{paras}<p><img src=\"/a.png\" alt=\"図\"></p></article>\
             <footer>著作権表示</footer></body></html>"
        )
    }

    #[test]
    fn an_article_is_extracted_with_a_share() {
        let got = extract_html(&article_page(), "https://example.com/news/1", false);
        assert_eq!(got.mode, Mode::Readability);
        assert!(got.markdown.contains("第 0 段落"), "{}", got.markdown);
        assert!(got.markdown.contains("第 7 段落"));
        assert!(!got.markdown.contains("カテゴリ 3"), "ナビは本文に入らない");
        let share = got.share.expect("比が出る");
        assert!(share > 0 && share < 100, "share={share}");
    }

    /// `links: false` は URL と画像を DOM の上で落とし、リンクの文字は残す。
    #[test]
    fn links_off_keeps_the_text_and_drops_urls_and_images() {
        let got = extract_html(&article_page(), "https://example.com/news/1", false);
        assert!(!got.markdown.contains("]("), "リンクも画像も残らない: {}", got.markdown);
        assert!(!got.markdown.contains("&amp;"));
        assert!(got.markdown.contains("手引き"), "リンクの文字は残る");
    }

    /// `links: true` は行き先を絶対 URL で残し、`&amp;` を `&` へ戻す。
    #[test]
    fn links_on_keeps_absolute_urls_with_plain_ampersands() {
        let got = extract_html(&article_page(), "https://example.com/news/1", true);
        assert!(got.markdown.contains("[手引き](https://example.com/doc?a=1&b=0)"), "{}", got.markdown);
        assert!(!got.markdown.contains("&amp;"));
        assert!(got.markdown.contains("![図]("), "画像も残る");
    }

    /// 本文の無いページ（`<frameset>`）は空で、`raw` を案内する側へ倒れる。
    #[test]
    fn a_frameset_page_is_empty() {
        let page = "<html><head><title>阿部のページ</title></head>\
                    <frameset cols=\"20%,*\"><frame src=\"menu.htm\"><frame src=\"top.htm\"></frameset></html>";
        let got = extract_html(page, "https://example.com/", false);
        assert_eq!(got.mode, Mode::Empty);
        assert!(got.markdown.is_empty());
    }

    /// dom_smoothie が本文を見つけられない短いページは、`<body>` 全体へ戻る（ナビ・フッターは飛ばす）。
    #[test]
    fn a_page_without_an_article_falls_back_to_the_body() {
        let page = "<html><head><title>短い</title></head><body><nav>メニュー</nav><div>本文だけ</div>\
                    <footer>下</footer></body></html>";
        let got = extract_html(page, "https://example.com/", false);
        assert!(matches!(got.mode, Mode::Readability | Mode::Body), "{got:?}");
        assert!(got.markdown.contains("本文だけ"));
        if got.mode == Mode::Body {
            assert!(!got.markdown.contains("メニュー"), "ページ全体でもナビは飛ばす");
            assert_eq!(got.share, None, "ページ全体のときは比を出さない");
        }
    }

    #[test]
    fn destinations_are_unescaped_once_and_text_is_left_alone() {
        let md = "本文に &amp; と書く [a](https://x.test/?a=1&amp;b=2&quot;c&nbsp;d) と <x>\n\
                  ![i](<https://x.test/a b?c=1&amp;amp;d>)\n";
        let out = unescape_destinations(md);
        assert!(out.starts_with("本文に &amp; と書く"), "地の文は触らない");
        assert!(out.contains("[a](https://x.test/?a=1&b=2\"c\u{a0}d)"));
        assert!(out.contains("![i](<https://x.test/a b?c=1&amp;d>)"), "1 回だけ戻す（&amp;amp; → &amp;）: {out}");
    }

    #[test]
    fn fenced_code_is_not_touched() {
        let md = "```\nlet x = a[i](b &amp; c);\n```\n[r](https://x.test/?a&amp;b)\n";
        let out = unescape_destinations(md);
        assert!(out.contains("a[i](b &amp; c)"), "コードの中は触らない");
        assert!(out.contains("[r](https://x.test/?a&b)"));
    }

    #[test]
    fn nested_parentheses_close_at_the_matching_paren() {
        let out = unescape_destinations("[w](https://x.test/A_(B)?x=1&amp;y=2) 後ろ &amp;\n");
        assert_eq!(out, "[w](https://x.test/A_(B)?x=1&y=2) 後ろ &amp;\n");
    }
}
