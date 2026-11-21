//! 文字コードの判定（Spec 78 D3 / `web_fetch_contract` 8）。純関数。
//!
//! 順は WHATWG の「エンコーディングの判定」と同じ: **BOM → 応答の `Content-Type` の `charset` → 本文の先頭
//! 1,024 バイトの宣言 → UTF-8**。最初に決まったものを使う。
//!
//! 宣言の UTF-16 は UTF-8 に、`x-user-defined` は windows-1252 に置き換え、**それ以外は ASCII 互換でなくても
//! 宣言どおりに読む**。ISO-2022-JP の宣言を持つページが実在し（P0a で 2 本）、上流の main の「ASCII 互換で
//! ない宣言は無視」だと UTF-8 に落ちてエスケープ列（`ESC $ B`）が本文に残る。
//!
//! 自動推定（chardetng）は持たない（P0a で宣言の無い非 UTF-8 は 0 本。裁定 3）。

use encoding_rs::Encoding;

/// どこで決まったか（ログの `encoding=` に名前と一緒に出す）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// BOM。
    Bom,
    /// 応答の `Content-Type` の `charset`。
    Header,
    /// 本文の先頭の宣言（`<meta>` か XML の宣言）。
    Declaration,
    /// どれも無く UTF-8。
    Default,
}

/// 判定の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Detected {
    /// 読むときの文字コード。
    pub encoding: &'static Encoding,
    /// どこで決まったか。
    pub source: Source,
}

/// 宣言を探す先頭の長さ（WHATWG の prescan と同じ）。
const PRESCAN_BYTES: usize = 1024;

/// `Content-Type` の値から `charset` の値を取る。値が空なら `None`（`charset=` だけのサーバーが実在する）。
pub fn header_charset(content_type: &str) -> Option<&str> {
    content_type.split(';').skip(1).find_map(|part| {
        let (key, value) = part.split_once('=')?;
        if !key.trim().eq_ignore_ascii_case("charset") {
            return None;
        }
        let value = value.trim().trim_matches(['"', '\'']).trim();
        (!value.is_empty()).then_some(value)
    })
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c')
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// タグの属性を `>` まで読む（WHATWG の「属性を得る」の簡略形）。引用符の中の `>` でタグを閉じない。
/// 返すのは（小文字の名前・値）の列と、`>` の次の位置。
fn read_attributes(b: &[u8], mut i: usize) -> (Vec<(String, String)>, usize) {
    let mut out = Vec::new();
    loop {
        while i < b.len() && (is_space(b[i]) || b[i] == b'/') {
            i += 1;
        }
        if i >= b.len() {
            return (out, i);
        }
        if b[i] == b'>' {
            return (out, i + 1);
        }
        let name_start = i;
        while i < b.len() && !is_space(b[i]) && !matches!(b[i], b'/' | b'>' | b'=') {
            i += 1;
        }
        let name = String::from_utf8_lossy(&b[name_start..i]).to_ascii_lowercase();
        while i < b.len() && is_space(b[i]) {
            i += 1;
        }
        let mut value = String::new();
        if i < b.len() && b[i] == b'=' {
            i += 1;
            while i < b.len() && is_space(b[i]) {
                i += 1;
            }
            if i < b.len() && matches!(b[i], b'"' | b'\'') {
                let quote = b[i];
                i += 1;
                let value_start = i;
                while i < b.len() && b[i] != quote {
                    i += 1;
                }
                value = String::from_utf8_lossy(&b[value_start..i.min(b.len())]).into_owned();
                i = (i + 1).min(b.len());
            } else {
                let value_start = i;
                while i < b.len() && !is_space(b[i]) && b[i] != b'>' {
                    i += 1;
                }
                value = String::from_utf8_lossy(&b[value_start..i]).into_owned();
            }
        }
        if !name.is_empty() {
            out.push((name, value));
        }
    }
}

/// `content="text/html; charset=Shift_JIS"` から `Shift_JIS`。
fn charset_in_content(content: &str) -> Option<String> {
    let at = content.to_ascii_lowercase().find("charset")?;
    let rest = content[at + "charset".len()..].trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let rest = rest.trim_start_matches(['"', '\'']);
    let end = rest
        .find(|c: char| c == ';' || c == '"' || c == '\'' || c.is_whitespace())
        .unwrap_or(rest.len());
    let value = &rest[..end];
    (!value.is_empty()).then(|| value.to_owned())
}

/// HTML の先頭 1,024 バイトから `<meta charset>` / `<meta http-equiv="Content-Type" content="…">` を探す。
/// コメントは飛ばし、他のタグは属性ごと飛ばす（`data-charset` のような別の属性を宣言と読まない）。
pub fn meta_declaration(bytes: &[u8]) -> Option<String> {
    let b = &bytes[..bytes.len().min(PRESCAN_BYTES)];
    let mut i = 0;
    while i < b.len() {
        if b[i..].starts_with(b"<!--") {
            i += 4 + find(&b[i + 4..], b"-->")? + 3;
            continue;
        }
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        let tag = &b[i + 1..];
        let is_meta = tag.len() > 4 && tag[..4].eq_ignore_ascii_case(b"meta") && (is_space(tag[4]) || tag[4] == b'/');
        if is_meta {
            let (attrs, next) = read_attributes(b, i + 5);
            i = next;
            let mut charset = None;
            let mut content_type = false;
            let mut content = None;
            for (name, value) in attrs {
                match name.as_str() {
                    "charset" if charset.is_none() => charset = Some(value),
                    "http-equiv" => content_type |= value.trim().eq_ignore_ascii_case("content-type"),
                    "content" if content.is_none() => content = Some(value),
                    _ => {}
                }
            }
            if let Some(value) = charset.filter(|v| !v.trim().is_empty()) {
                return Some(value.trim().to_owned());
            }
            if content_type && let Some(value) = content.as_deref().and_then(charset_in_content) {
                return Some(value);
            }
            continue;
        }
        let starts_tag = tag.first().is_some_and(u8::is_ascii_alphabetic)
            || (tag.first() == Some(&b'/') && tag.get(1).is_some_and(u8::is_ascii_alphabetic));
        if starts_tag {
            let mut j = i + 1;
            while j < b.len() && !is_space(b[j]) && b[j] != b'>' {
                j += 1;
            }
            i = read_attributes(b, j).1;
            continue;
        }
        if matches!(tag.first(), Some(b'!' | b'/' | b'?')) {
            i += 1 + tag.iter().position(|&c| c == b'>')? + 1;
            continue;
        }
        i += 1;
    }
    None
}

/// 本文の頭の `<?xml … encoding="…"?>` の値。
pub fn xml_declaration(bytes: &[u8]) -> Option<String> {
    let head = &bytes[..bytes.len().min(PRESCAN_BYTES)];
    if !head.starts_with(b"<?xml") {
        return None;
    }
    let decl = String::from_utf8_lossy(&head[..find(head, b"?>")?]).into_owned();
    let at = decl.find("encoding")?;
    let rest = decl[at + "encoding".len()..].trim_start().strip_prefix('=')?.trim_start();
    let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let rest = &rest[1..];
    let value = &rest[..rest.find(quote)?];
    (!value.is_empty()).then(|| value.to_owned())
}

/// 中身の種類から、本文の宣言の探し方。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Markup {
    /// `text/html` / `application/xhtml+xml` / 型が無い — `<meta>`、無ければ XML の宣言。
    Html,
    /// `application/xml` / `text/xml` / `*+xml` — XML の宣言。
    Xml,
    /// それ以外 — 本文の宣言は見ない。
    Other,
}

/// 判定する（D3）。`content_type` は応答の `Content-Type` の値（無ければ空）。
pub fn detect(bytes: &[u8], content_type: &str, markup: Markup) -> Detected {
    if let Some((encoding, _)) = Encoding::for_bom(bytes) {
        return Detected { encoding, source: Source::Bom };
    }
    if let Some(encoding) = header_charset(content_type).and_then(|l| Encoding::for_label(l.as_bytes())) {
        return Detected { encoding, source: Source::Header };
    }
    let declared = match markup {
        Markup::Html => meta_declaration(bytes).or_else(|| xml_declaration(bytes)),
        Markup::Xml => xml_declaration(bytes),
        Markup::Other => None,
    };
    if let Some(encoding) = declared.as_deref().and_then(|l| Encoding::for_label(l.as_bytes())) {
        // ASCII のバイトで書かれた UTF-16 の宣言は真であり得ない（HTML 標準と同じ置き換え）。
        let encoding = if encoding == encoding_rs::UTF_16LE || encoding == encoding_rs::UTF_16BE {
            encoding_rs::UTF_8
        } else if encoding == encoding_rs::X_USER_DEFINED {
            encoding_rs::WINDOWS_1252
        } else {
            encoding
        };
        return Detected { encoding, source: Source::Declaration };
    }
    Detected { encoding: encoding_rs::UTF_8, source: Source::Default }
}

/// 判定した文字コードで読む。BOM があれば BOM が勝つ（`decode` の振る舞い = WHATWG と同じ）。
pub fn decode(bytes: &[u8], detected: Detected) -> String {
    detected.encoding.decode(bytes).0.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_value_is_read_and_an_empty_one_is_ignored() {
        assert_eq!(header_charset("text/html; charset=EUC-JP"), Some("EUC-JP"));
        assert_eq!(header_charset("text/html;charset=\"utf-8\""), Some("utf-8"));
        assert_eq!(header_charset("text/html; charset="), None, "値が空（P0a の nurs.or.jp）");
        assert_eq!(header_charset("text/html"), None);
    }

    /// 青空文庫の頭の形（XML の宣言と `<meta http-equiv>` の両方が Shift_JIS）。応答ヘッダに charset は無い。
    #[test]
    fn a_meta_declaration_decides_when_the_header_is_silent() {
        let head = b"<?xml version=\"1.0\" encoding=\"Shift_JIS\"?>\r\n<!DOCTYPE html>\r\n<html><head>\r\n\
                     <meta http-equiv=\"Content-Type\" content=\"text/html;charset=Shift_JIS\" />";
        let d = detect(head, "text/html", Markup::Html);
        assert_eq!(d, Detected { encoding: encoding_rs::SHIFT_JIS, source: Source::Declaration });
    }

    #[test]
    fn the_order_is_bom_then_header_then_declaration() {
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice(b"<meta charset=\"shift_jis\">");
        assert_eq!(detect(&bom, "text/html; charset=euc-jp", Markup::Html).source, Source::Bom, "BOM が先");
        let meta = b"<meta charset=\"shift_jis\">";
        let d = detect(meta, "text/html; charset=euc-jp", Markup::Html);
        assert_eq!((d.encoding, d.source), (encoding_rs::EUC_JP, Source::Header), "ヘッダが宣言より先");
        assert_eq!(detect(b"<p>x</p>", "", Markup::Html).source, Source::Default);
    }

    /// **ASCII 互換でない宣言も宣言どおりに読む**（P0a の jp.freebsd.org・st.rim.or.jp）。
    #[test]
    fn iso_2022_jp_is_honoured() {
        let page = b"<html><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=iso-2022-jp\"></head>\
                     <body>\x1b$B$3$s$K$A$O\x1b(B</body></html>";
        let d = detect(page, "text/html", Markup::Html);
        assert_eq!(d.encoding, encoding_rs::ISO_2022_JP);
        assert!(decode(page, d).contains("こんにちは"), "エスケープ列が残らない");
    }

    #[test]
    fn utf16_and_x_user_defined_declarations_are_replaced() {
        assert_eq!(detect(b"<meta charset=utf-16>", "", Markup::Html).encoding, encoding_rs::UTF_8);
        assert_eq!(detect(b"<meta charset=x-user-defined>", "", Markup::Html).encoding, encoding_rs::WINDOWS_1252);
    }

    #[test]
    fn other_attributes_and_comments_are_not_declarations() {
        assert_eq!(meta_declaration(b"<div data-charset=\"shift_jis\"><meta name=x>"), None);
        assert_eq!(meta_declaration(b"<!-- <meta charset=\"shift_jis\"> --><p>"), None);
        assert_eq!(meta_declaration(b"<a title=\"<meta charset=sjis>\">x</a>"), None, "引用符の中の < は飛ばす");
        assert_eq!(meta_declaration(b"<META CHARSET=EUC-JP>").as_deref(), Some("EUC-JP"));
        assert_eq!(meta_declaration(b"<meta http-equiv=content-type content='text/html; charset=shift-jis'>").as_deref(), Some("shift-jis"));
    }

    #[test]
    fn a_declaration_past_1024_bytes_is_not_seen() {
        let mut page = vec![b' '; 1024];
        page.extend_from_slice(b"<meta charset=shift_jis>");
        assert_eq!(meta_declaration(&page), None);
    }

    #[test]
    fn an_xml_declaration_is_read_for_xml() {
        assert_eq!(xml_declaration(b"<?xml version='1.0' encoding='EUC-JP'?><a/>").as_deref(), Some("EUC-JP"));
        assert_eq!(detect(b"<?xml version=\"1.0\" encoding=\"Shift_JIS\"?><a/>", "application/xml", Markup::Xml).encoding, encoding_rs::SHIFT_JIS);
        assert_eq!(detect(b"<meta charset=sjis>", "text/plain", Markup::Other).source, Source::Default, "text/plain は宣言を見ない");
    }

    /// 宣言は壊れたバイト列でも panic しない（境界の外を読まない）。
    #[test]
    fn truncated_tags_do_not_panic() {
        for page in [&b"<meta"[..], b"<meta charset=\"", b"<!--", b"<a href='", b"<", b"<?xml encoding=", b"<meta http-equiv=content-type content="] {
            let _ = detect(page, "", Markup::Html);
        }
    }
}
