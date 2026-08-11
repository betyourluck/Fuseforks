//! Markdown の見出し索引（Spec 18 の純機構層）。
//!
//! 「PageIndex」の考え方（見出し = 人が置いた意味の切れ目を索引にする）に基づく。
//! LLM に目次を推測させるのではなく、著者が書いた `#` の階層をそのまま読む。
//! 出自は利用者の MCP サーバー Manuale（`markdown.rs` / `read.rs`）の移植。
//!
//! Based on the PageIndex idea (MIT License, Vectify AI).
//! This is an independent implementation of the concept, not a code port.
//! <https://github.com/VectifyAI/PageIndex>
//!
//! この層はファイルシステムを知らない — 入力は Markdown の文字列、出力は
//! 見出しの列・木・節の範囲だけ。走査と囲い（どのフォルダを読んでよいか）は
//! `tools/rag.rs` の仕事で、混ぜると純関数のテストにディスクが要るようになる。

use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};

/// Markdown から抜き出した見出し 1 本（平坦な列の要素）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadingFlat {
    /// 見出しレベル（`#` の数、1〜6）。
    pub level: u8,
    /// 見出しの本文（インラインコードは地の文として畳む）。
    pub title: String,
    /// 見出しが始まる行番号（1 始まり）。
    pub line: usize,
}

/// 入れ子にした見出し木のノード。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadingNode {
    /// 見出しレベル。
    pub level: u8,
    /// 見出しの本文。
    pub title: String,
    /// 見出しが始まる行番号（1 始まり）。
    pub line: usize,
    /// 直下の子見出し。
    pub children: Vec<HeadingNode>,
}

fn level_to_u8(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn byte_offset_to_line(src: &str, offset: usize) -> usize {
    src[..offset.min(src.len())].bytes().filter(|&b| b == b'\n').count() + 1
}

/// Markdown から全見出しを平坦な列として抜き出す。
///
/// 自前の行スキャンではなくパーサを通すのは、コードブロック内の `#` や
/// setext 見出し（下線式）を正しく扱うため — そこを自前で書くと
/// CommonMark パーサの再発明になる。
pub fn parse_headings(md: &str) -> Vec<HeadingFlat> {
    let parser = Parser::new(md).into_offset_iter();
    let mut out = Vec::new();
    let mut current_level: Option<u8> = None;
    let mut current_text = String::new();
    let mut current_line: usize = 0;

    for (event, range) in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                current_level = Some(level_to_u8(level));
                current_text.clear();
                current_line = byte_offset_to_line(md, range.start);
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(level) = current_level.take() {
                    out.push(HeadingFlat {
                        level,
                        title: std::mem::take(&mut current_text).trim().to_owned(),
                        line: current_line,
                    });
                }
            }
            Event::Text(text) | Event::Code(text) if current_level.is_some() => {
                current_text.push_str(&text);
            }
            _ => {}
        }
    }
    out
}

/// 平坦な見出し列をレベルで入れ子の木にする。
///
/// レベルの飛び（H1 の直下に H4）は受け入れる — スタック上にある
/// より浅い見出しの子としてぶら下げる。人が書いた文書は飛ぶ。
pub fn build_tree(flat: Vec<HeadingFlat>) -> Vec<HeadingNode> {
    let mut roots: Vec<HeadingNode> = Vec::new();
    let mut stack: Vec<HeadingNode> = Vec::new();

    fn push_finished(stack: &mut Vec<HeadingNode>, roots: &mut Vec<HeadingNode>) {
        if let Some(finished) = stack.pop() {
            if let Some(parent) = stack.last_mut() {
                parent.children.push(finished);
            } else {
                roots.push(finished);
            }
        }
    }

    for heading in flat {
        while stack.last().is_some_and(|top| top.level >= heading.level) {
            push_finished(&mut stack, &mut roots);
        }
        stack.push(HeadingNode {
            level: heading.level,
            title: heading.title,
            line: heading.line,
            children: Vec::new(),
        });
    }
    while !stack.is_empty() {
        push_finished(&mut stack, &mut roots);
    }
    roots
}

/// 指定した行を含む見出しの経路（祖先 → 葉の順）を返す。
///
/// `search` の一致行へ「どの節の話か」を添えるための関数。これが無いと
/// 一致行は平坦な grep と同じで、見出し索引の価値が消える。
pub fn section_path_for_line(flat: &[HeadingFlat], line: usize) -> Vec<String> {
    let mut stack: Vec<&HeadingFlat> = Vec::new();
    for heading in flat {
        if heading.line > line {
            break;
        }
        while stack.last().is_some_and(|top| top.level >= heading.level) {
            stack.pop();
        }
        stack.push(heading);
    }
    stack.iter().map(|h| h.title.clone()).collect()
}

/// 節名の照合がどの段で解決したか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchMode {
    /// 文字列の完全一致。
    Exact,
    /// 大文字小文字を無視した一致。
    CaseInsensitive,
    /// 大文字小文字を無視した部分一致。
    Substring,
}

impl MatchMode {
    /// モデルへ返す表記。
    pub fn as_str(self) -> &'static str {
        match self {
            MatchMode::Exact => "完全一致",
            MatchMode::CaseInsensitive => "大文字小文字を無視",
            MatchMode::Substring => "部分一致",
        }
    }
}

/// 節名の解決結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionResolution {
    /// 一意に決まった。`index` は `flat` の添字。
    Found {
        /// 解決した見出しの `flat` 内の位置。
        index: usize,
        /// どの段で解決したか。
        mode: MatchMode,
    },
    /// どの段でも 1 件も当たらなかった。
    NotFound,
    /// 同じ段で複数当たった。候補の添字を返す — **黙って 1 つ選ばない**
    /// （rag_tool_contract。選ぶと「読んだつもりの節」と実際の節がずれる）。
    Ambiguous(Vec<usize>),
}

/// 節名を 3 段（完全一致 → 大文字小文字無視 → 部分一致）で解決する。
///
/// 各段で 1 件なら即決、複数なら曖昧として止める。段は最初に候補が出た
/// ところで打ち切る — 完全一致が 1 件あるのに部分一致まで見ると、
/// 短い問い合わせが常に曖昧になる。
pub fn resolve_section(flat: &[HeadingFlat], query: &str) -> SectionResolution {
    fn tier(
        flat: &[HeadingFlat],
        mode: MatchMode,
        predicate: impl Fn(&HeadingFlat) -> bool,
    ) -> Option<SectionResolution> {
        let hits: Vec<usize> = flat
            .iter()
            .enumerate()
            .filter(|(_, h)| predicate(h))
            .map(|(i, _)| i)
            .collect();
        match hits.len() {
            0 => None,
            1 => Some(SectionResolution::Found { index: hits[0], mode }),
            _ => Some(SectionResolution::Ambiguous(hits)),
        }
    }

    let lower = query.to_lowercase();
    tier(flat, MatchMode::Exact, |h| h.title == query)
        .or_else(|| tier(flat, MatchMode::CaseInsensitive, |h| h.title.to_lowercase() == lower))
        .or_else(|| tier(flat, MatchMode::Substring, |h| h.title.to_lowercase().contains(&lower)))
        .unwrap_or(SectionResolution::NotFound)
}

/// 節の本文が占める行範囲（0 始まりの半開区間）を返す。
///
/// 始まりは見出し行そのもの、終わりは**同じかより浅いレベルの次の見出し**の
/// 直前（無ければファイル末尾）。子見出しは節の一部として含む。
pub fn section_bounds(flat: &[HeadingFlat], index: usize, line_count: usize) -> (usize, usize) {
    let head = &flat[index];
    let start = head.line.saturating_sub(1);
    let end = flat
        .iter()
        .skip(index + 1)
        .find(|h| h.level <= head.level)
        .map(|h| h.line.saturating_sub(1))
        .unwrap_or(line_count);
    (start, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_headings() {
        let md = "# Top\n\n## Mid\n\n### Deep\n\n## Mid2\n";
        let flat = parse_headings(md);
        assert_eq!(flat.len(), 4);
        assert_eq!(flat[0].title, "Top");
        assert_eq!(flat[0].level, 1);
        assert_eq!(flat[2].level, 3);
    }

    #[test]
    fn builds_nested_tree() {
        let md = "# A\n## A1\n### A1a\n## A2\n# B\n";
        let tree = build_tree(parse_headings(md));
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0].children.len(), 2);
        assert_eq!(tree[0].children[0].children[0].title, "A1a");
        assert_eq!(tree[1].title, "B");
    }

    #[test]
    fn handles_skipped_levels() {
        let md = "# A\n#### Deep\n## Mid\n";
        let tree = build_tree(parse_headings(md));
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].children.len(), 2);
        assert_eq!(tree[0].children[0].level, 4);
        assert_eq!(tree[0].children[1].level, 2);
    }

    #[test]
    fn ignores_hash_inside_code_blocks() {
        // 自前の行スキャンだと拾ってしまう代表例。パーサを通す理由の固定。
        let md = "# Real\n\n```sh\n# not a heading\n```\n";
        let flat = parse_headings(md);
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].title, "Real");
    }

    #[test]
    fn japanese_headings_survive() {
        let md = "# 台帳の構成\n\n## 現在地（2026-08-05）\n\n本文\n";
        let flat = parse_headings(md);
        assert_eq!(flat[0].title, "台帳の構成");
        assert_eq!(flat[1].title, "現在地（2026-08-05）");
    }

    #[test]
    fn inline_code_in_heading_is_flattened() {
        let md = "## `run.json` の改名\n";
        let flat = parse_headings(md);
        assert_eq!(flat[0].title, "run.json の改名");
    }

    #[test]
    fn section_path_walks_correctly() {
        let md = "# A\n\nfirst\n\n## A1\n\nsecond\n\n### A1a\n\nthird\n";
        let flat = parse_headings(md);
        assert_eq!(section_path_for_line(&flat, 3), vec!["A"]);
        assert_eq!(section_path_for_line(&flat, 7), vec!["A", "A1"]);
        assert_eq!(section_path_for_line(&flat, 11), vec!["A", "A1", "A1a"]);
    }

    #[test]
    fn resolve_exact_wins_over_substring() {
        let md = "## Configuration\n\n## Configuration Notes\n";
        let flat = parse_headings(md);
        // "Configuration" は部分一致なら 2 件当たるが、完全一致の段で 1 件に決まる。
        let SectionResolution::Found { index, mode } = resolve_section(&flat, "Configuration")
        else {
            panic!("完全一致で解決すること");
        };
        assert_eq!(index, 0);
        assert_eq!(mode, MatchMode::Exact);
    }

    #[test]
    fn resolve_ambiguous_returns_candidates() {
        let md = "## Configuration\n\n## Configuration Notes\n";
        let flat = parse_headings(md);
        let SectionResolution::Ambiguous(hits) = resolve_section(&flat, "config") else {
            panic!("部分一致 2 件は曖昧になること");
        };
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn resolve_not_found() {
        let flat = parse_headings("# A\n");
        assert_eq!(resolve_section(&flat, "zzz"), SectionResolution::NotFound);
    }

    #[test]
    fn section_bounds_include_children_and_stop_at_sibling() {
        let md = "# Top\n\n## A\n\nbody-a\n\n### A1\n\nbody-a1\n\n## B\n\nbody-b\n";
        let flat = parse_headings(md);
        let lines = md.lines().count();
        // "A"（添字 1）は子 A1 を含み、兄弟 "B" の直前で終わる。
        let (start, end) = section_bounds(&flat, 1, lines);
        let body: Vec<&str> = md.lines().collect();
        let section = body[start..end].join("\n");
        assert!(section.contains("body-a"));
        assert!(section.contains("body-a1"));
        assert!(!section.contains("body-b"));
    }

    #[test]
    fn last_section_runs_to_end_of_file() {
        let md = "# Only\n\ntail\n";
        let flat = parse_headings(md);
        let (start, end) = section_bounds(&flat, 0, md.lines().count());
        assert_eq!((start, end), (0, 3));
    }
}
