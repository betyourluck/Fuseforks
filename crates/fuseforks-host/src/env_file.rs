//! コンテナの `.env` を組む純関数（Spec 66 D3 / D4 / D6・`container_contract` 16）。
//!
//! I/O は持たない — 読むのは呼び出し側が渡した既存の本文、作るのは書く本文と報告だけ。
//! **値を報告に載せない**（名前と件数だけ。`TZ` の食い違いだけは時刻帯の名前を載せる — 秘密ではない）。
//!
//! ## compose の読み方（Spec 66 の前提の実測）
//!
//! - 引用符なしの `$x` は**展開される**（未定義なら空）。空白の後の `#` からはコメント
//! - **単一引用符の中は `$`・`#`・`"`・空白がそのまま**。エスケープは `\'` だけ
//!
//! だから値は常に単一引用符で囲み、`'`・`\`・改行・制御文字を含む値は**書かない**（D4。`\` で終わる値と
//! `\'` を区別できないので、エスケープの規則を作らない）。

use std::collections::BTreeSet;

/// `bake` が置いたコメントの印（D3）。この印のある `# NAME=` だけが「`bake` が置いた空き」。
pub const MARKER: &str = "# bake:";

/// 鍵の種類（書き出す順でもある）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// 時刻帯（`bake.json` の `sourceTimeZone`）。
    Tz,
    /// モデルテンプレートの鍵。
    Template,
    /// MCP の `${secret:NAME}`。
    Mcp,
    /// Jev の鍵。
    Jev,
    /// 扉の合鍵（コンテナ用。GUI の扉とは別に作る — D5）。
    Door,
}

impl Kind {
    /// 資格情報ストアから読む鍵か（`ENV_VALUE_DIFFERS` の対象。D6）。
    fn read_from_store(self) -> bool {
        matches!(self, Self::Template | Self::Mcp | Self::Jev)
    }

    fn heading(self) -> &'static str {
        match self {
            Self::Tz => "# 時刻帯（bake.json の sourceTimeZone。予定が GUI と同じ時刻に発火する）",
            Self::Template => "# モデルテンプレートの鍵（村の個体が使うものだけ）",
            Self::Mcp => "# MCP の headers の ${secret:NAME}（村の有効な http のサーバー）",
            Self::Jev => "# Jev（ツール結果の圧縮・判断役）",
            Self::Door => "# 扉の合鍵（コンテナ用。GUI の扉の合鍵とは別。serve --door-port で使う）",
        }
    }
}

/// 書き出したい鍵 1 つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    /// 環境変数の名前（`TZ` か `FUSEFORKS_SECRET_…`）。
    pub name: String,
    /// 種類。
    pub kind: Kind,
    /// 値。`None` は資格情報ストアに無い（扉の合鍵では「作る」）。
    pub value: Option<String>,
}

/// 書いた結果（D7 — 名前と件数だけ）。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvReport {
    /// 値を書いた名前（`--update` では新しく値が入った名前だけ）。
    pub written: Vec<String>,
    /// 資格情報ストアに無いので印つきコメントにした名前。
    pub missing: Vec<String>,
    /// `.env` に書けない文字を含むので印つきコメントにした名前（`ENV_VALUE_UNWRITABLE`）。
    pub unwritable: Vec<String>,
    /// 既にある値がストアと違う名前（`ENV_VALUE_DIFFERS`。値は出さない）。
    pub differs: Vec<String>,
    /// 村が要らなくなった `FUSEFORKS_SECRET_*`（`ENV_UNUSED`。消さない）。
    pub unused: Vec<String>,
    /// 印の無いコメントで止められている名前（`ENV_COMMENTED_OUT`。触らない）。
    pub commented_out: Vec<String>,
    /// 既にある `TZ` と `sourceTimeZone`（違うときだけ。`ENV_TZ_DIFFERS`）。
    pub tz_differs: Option<(String, String)>,
    /// 扉の合鍵を新しく作ったか。
    pub door_token_created: bool,
}

/// 既にある行の分類（D6 の表）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// 有効な代入 `NAME=…`（値は compose と同じ規則で読んだもの）。
    Assign {
        /// 変数名。
        name: String,
        /// compose と同じ規則で読んだ値。
        value: String,
    },
    /// `bake` の印つきコメント `# NAME=   # bake: …`。
    Placeholder {
        /// 変数名。
        name: String,
    },
    /// 印の無いコメントの `# NAME=…`（運用者が止めた名前）。
    CommentedOut {
        /// 変数名。
        name: String,
    },
    /// その他（空行・説明のコメント・読めない行）。
    Other,
}

fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && !name.as_bytes()[0].is_ascii_digit()
}

/// 1 行を分類する。
pub fn classify(line: &str) -> Line {
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed.strip_prefix('#') {
        let body = rest.trim_start();
        if let Some((name, _)) = body.split_once('=')
            && is_name(name.trim_end())
            && !name.contains(char::is_whitespace)
        {
            let name = name.to_owned();
            return if body.contains(MARKER) || rest.contains(MARKER) {
                Line::Placeholder { name }
            } else {
                Line::CommentedOut { name }
            };
        }
        return Line::Other;
    }
    let body = trimmed.strip_prefix("export ").unwrap_or(trimmed);
    match body.split_once('=') {
        Some((name, raw)) if is_name(name) => Line::Assign {
            name: name.to_owned(),
            value: read_value(raw),
        },
        _ => Line::Other,
    }
}

/// 値を compose と同じ規則で読む（D6 — 比べる前の正規化）。単一引用符は中身をそのまま（`\'` だけ `'`）、
/// 二重引用符は中身（`\"` と `\\` を戻す）、引用符なしは空白の後の `#` から先を落として前後の空白を落とす。
pub fn read_value(raw: &str) -> String {
    let raw = raw.trim_start();
    if let Some(inner) = raw.strip_prefix('\'') {
        let mut out = String::new();
        let mut chars = inner.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' if chars.peek() == Some(&'\'') => {
                    out.push('\'');
                    chars.next();
                }
                '\'' => return out,
                other => out.push(other),
            }
        }
        return out;
    }
    if let Some(inner) = raw.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => match chars.next() {
                    Some(next @ ('"' | '\\')) => out.push(next),
                    Some(next) => {
                        out.push('\\');
                        out.push(next);
                    }
                    None => out.push('\\'),
                },
                '"' => return out,
                other => out.push(other),
            }
        }
        return out;
    }
    let cut = raw
        .char_indices()
        .find(|&(i, c)| c == '#' && i > 0 && raw[..i].ends_with(char::is_whitespace))
        .map_or(raw.len(), |(i, _)| i);
    raw[..cut].trim().to_owned()
}

/// 単一引用符で囲んで書ける値か（D4）。`'`・`\`・制御文字（改行を含む）を含めば書かない。
pub fn writable(value: &str) -> bool {
    !value.chars().any(|c| c == '\'' || c == '\\' || c.is_control())
}

fn assign_line(name: &str, value: &str) -> String {
    format!("{name}='{value}'")
}

fn placeholder_line(name: &str, reason: &str) -> String {
    format!("# {name}=   {MARKER} {reason}")
}

const REASON_MISSING: &str = "資格情報ストアに無い";
const REASON_UNWRITABLE: &str = ".env に書けない文字（' か \\ か改行）を含む — この 1 行だけ手で書く";

/// 1 つの鍵を書く行（`report` に名前を積む）。扉の合鍵は値が無ければ `new_token` で作る。
fn render(wanted: &Wanted, report: &mut EnvReport, new_token: &mut dyn FnMut() -> String) -> String {
    let value = match (&wanted.value, wanted.kind) {
        (Some(value), _) => value.clone(),
        (None, Kind::Door) => {
            report.door_token_created = true;
            new_token()
        }
        (None, _) => {
            report.missing.push(wanted.name.clone());
            return placeholder_line(&wanted.name, REASON_MISSING);
        }
    };
    if !writable(&value) {
        report.unwritable.push(wanted.name.clone());
        return placeholder_line(&wanted.name, REASON_UNWRITABLE);
    }
    if wanted.kind != Kind::Door {
        report.written.push(wanted.name.clone());
    }
    assign_line(&wanted.name, &value)
}

fn render_sections(
    wanted: &[Wanted],
    report: &mut EnvReport,
    new_token: &mut dyn FnMut() -> String,
) -> Vec<String> {
    let mut sorted: Vec<&Wanted> = wanted.iter().collect();
    sorted.sort_by(|a, b| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
    let mut lines = Vec::new();
    let mut current: Option<Kind> = None;
    for item in sorted {
        if current != Some(item.kind) {
            if current.is_some() {
                lines.push(String::new());
            }
            lines.push(item.kind.heading().to_owned());
            current = Some(item.kind);
        }
        lines.push(render(item, report, new_token));
    }
    lines
}

const HEADER: [&str; 3] = [
    "# fuseforks-cli bake --env-out が書いた .env（Spec 66）。**値は平文** — git に入れない・配らない。",
    "# 値は単一引用符で囲んである（compose が $ を展開しないように）。`# bake:` の付いた行は bake が置いた空き。",
    "",
];

/// 書く本文と報告を作る。`existing` は既にある `.env` の本文（`--update` のとき）。
///
/// - 無ければ: 見出しつきで全部を書く
/// - あれば: 既にある行は書き換えない（印つきコメントは、値が書けるなら代入へ置き換える）。どの形でも
///   出てこない名前だけを末尾に足す。違い・要らない名前・止められている名前は報告に名前だけを積む
pub fn plan(
    existing: Option<&str>,
    wanted: &[Wanted],
    new_token: &mut dyn FnMut() -> String,
) -> (String, EnvReport) {
    let mut report = EnvReport::default();
    let Some(existing) = existing else {
        let mut lines: Vec<String> = HEADER.iter().map(|s| (*s).to_owned()).collect();
        lines.extend(render_sections(wanted, &mut report, new_token));
        return (lines.join("\n") + "\n", report);
    };

    let by_name = |name: &str| wanted.iter().find(|w| w.name == name);
    let wanted_names: BTreeSet<&str> = wanted.iter().map(|w| w.name.as_str()).collect();
    let door_name = wanted.iter().find(|w| w.kind == Kind::Door).map(|w| w.name.clone());
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out: Vec<String> = Vec::new();
    for line in existing.lines() {
        match classify(line) {
            Line::Assign { name, value } => {
                if let Some(item) = by_name(&name) {
                    match (item.kind, &item.value) {
                        (Kind::Tz, Some(tz)) if &value != tz => {
                            report.tz_differs = Some((value.clone(), tz.clone()));
                        }
                        (kind, Some(stored)) if kind.read_from_store() && &value != stored => {
                            report.differs.push(name.clone());
                        }
                        _ => {}
                    }
                } else if name.starts_with(fuseforks_core::secret::ENV_SECRET_PREFIX)
                    && door_name.as_deref() != Some(name.as_str())
                {
                    report.unused.push(name.clone());
                }
                seen.insert(name);
                out.push(line.to_owned());
            }
            Line::Placeholder { name } => {
                match by_name(&name) {
                    Some(item) if item.value.as_deref().is_some_and(writable) => {
                        out.push(render(item, &mut report, new_token));
                    }
                    Some(item) if item.value.is_some() => {
                        report.unwritable.push(name.clone());
                        out.push(line.to_owned());
                    }
                    _ => out.push(line.to_owned()),
                }
                seen.insert(name);
            }
            Line::CommentedOut { name } => {
                if wanted_names.contains(name.as_str()) {
                    report.commented_out.push(name.clone());
                }
                seen.insert(name);
                out.push(line.to_owned());
            }
            Line::Other => out.push(line.to_owned()),
        }
    }
    let missing: Vec<Wanted> = wanted.iter().filter(|w| !seen.contains(&w.name)).cloned().collect();
    if !missing.is_empty() {
        while out.last().is_some_and(|l| l.trim().is_empty()) {
            out.pop();
        }
        out.push(String::new());
        out.push("# ---- bake --update で足した名前 ----".to_owned());
        out.extend(render_sections(&missing, &mut report, new_token));
    }
    (out.join("\n") + "\n", report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(name: &str, kind: Kind, value: Option<&str>) -> Wanted {
        Wanted { name: name.to_owned(), kind, value: value.map(str::to_owned) }
    }

    fn token() -> impl FnMut() -> String {
        || "0123456789abcdef0123456789abcdef".to_owned()
    }

    /// compose の実測の表（Spec 66 の前提）をそのまま写す。
    #[test]
    fn values_are_read_the_way_compose_reads_them() {
        for (raw, expected) in [
            ("abc#123", "abc#123"),
            ("abc #123", "abc"),
            ("'a#b$x \"q\"'", "a#b$x \"q\""),
            ("\"a\\\"b\\\\c\"", "a\"b\\c"),
            ("'a\\'b'", "a'b"),
            ("'a\\b'", "a\\b"),
            ("  spaced  ", "spaced"),
        ] {
            assert_eq!(read_value(raw), expected, "{raw}");
        }
    }

    #[test]
    fn only_quote_backslash_and_control_characters_are_unwritable() {
        assert!(writable("sk-ant_AB.cd$x#y \"z\""));
        for bad in ["a'b", "a\\b", "a\nb", "a\tb", "trailing\\"] {
            assert!(!writable(bad), "{bad:?}");
        }
    }

    #[test]
    fn lines_are_classified_by_the_marker() {
        assert_eq!(
            classify("FUSEFORKS_SECRET_A='x'"),
            Line::Assign { name: "FUSEFORKS_SECRET_A".into(), value: "x".into() }
        );
        assert_eq!(
            classify("# FUSEFORKS_SECRET_A=   # bake: 資格情報ストアに無い"),
            Line::Placeholder { name: "FUSEFORKS_SECRET_A".into() }
        );
        assert_eq!(
            classify("#FUSEFORKS_SECRET_A='old'"),
            Line::CommentedOut { name: "FUSEFORKS_SECRET_A".into() }
        );
        assert_eq!(classify("# 説明のコメント"), Line::Other);
        assert_eq!(classify(""), Line::Other);
    }

    /// 初回: 種類ごと・名前の順。ストアに無い鍵と書けない値は印つきコメント、扉の合鍵は作る。
    #[test]
    fn a_new_file_writes_every_wanted_name_and_never_reports_values() {
        let wanted = [
            w("FUSEFORKS_SECRET_MCP_B", Kind::Mcp, None),
            w("FUSEFORKS_SECRET_STUB", Kind::Template, Some("sk-$x#y")),
            w("TZ", Kind::Tz, Some("Asia/Tokyo")),
            w("FUSEFORKS_SECRET_ODD", Kind::Template, Some("a'b")),
            w("FUSEFORKS_SECRET_DOOR_TOKEN", Kind::Door, None),
        ];
        let (text, report) = plan(None, &wanted, &mut token());
        let body: Vec<&str> = text.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).collect();
        assert_eq!(
            body,
            [
                "TZ='Asia/Tokyo'",
                "FUSEFORKS_SECRET_STUB='sk-$x#y'",
                "FUSEFORKS_SECRET_DOOR_TOKEN='0123456789abcdef0123456789abcdef'",
            ]
        );
        assert!(text.contains("# FUSEFORKS_SECRET_MCP_B=   # bake: 資格情報ストアに無い"));
        assert!(text.contains("# FUSEFORKS_SECRET_ODD=   # bake: .env に書けない"));
        assert_eq!(report.written, ["TZ", "FUSEFORKS_SECRET_STUB"]);
        assert_eq!(report.missing, ["FUSEFORKS_SECRET_MCP_B"]);
        assert_eq!(report.unwritable, ["FUSEFORKS_SECRET_ODD"]);
        assert!(report.door_token_created);
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains("sk-") && !json.contains("0123456789abcdef"), "{json}");
    }

    /// `--update`: 既にある行は書き換えない。印つきコメントは値が揃えば置き換え、足りない名前だけ末尾へ。
    #[test]
    fn an_update_keeps_existing_lines_and_fills_only_the_gaps() {
        let existing = "\
FUSEFORKS_DOMAIN=fuseforks.example.com
TZ=UTC
FUSEFORKS_SECRET_STUB=\"older\"
# FUSEFORKS_SECRET_MCP_B=   # bake: 資格情報ストアに無い
#FUSEFORKS_SECRET_JEV_API_TOKEN='paused'
FUSEFORKS_SECRET_OLD='gone'
FUSEFORKS_SECRET_DOOR_TOKEN='keep-me'
";
        let wanted = [
            w("TZ", Kind::Tz, Some("Asia/Tokyo")),
            w("FUSEFORKS_SECRET_STUB", Kind::Template, Some("newer")),
            w("FUSEFORKS_SECRET_MCP_B", Kind::Mcp, Some("now-here")),
            w("FUSEFORKS_SECRET_JEV_API_TOKEN", Kind::Jev, Some("jev")),
            w("FUSEFORKS_SECRET_MCP_C", Kind::Mcp, Some("added")),
            w("FUSEFORKS_SECRET_DOOR_TOKEN", Kind::Door, None),
        ];
        let mut made = 0;
        let (text, report) = plan(Some(existing), &wanted, &mut || {
            made += 1;
            "should-not-be-used".to_owned()
        });
        assert_eq!(made, 0, "扉の合鍵は作り直さない");
        for kept in [
            "FUSEFORKS_DOMAIN=fuseforks.example.com",
            "TZ=UTC",
            "FUSEFORKS_SECRET_STUB=\"older\"",
            "#FUSEFORKS_SECRET_JEV_API_TOKEN='paused'",
            "FUSEFORKS_SECRET_OLD='gone'",
            "FUSEFORKS_SECRET_DOOR_TOKEN='keep-me'",
        ] {
            assert!(text.lines().any(|l| l == kept), "消えた: {kept}\n{text}");
        }
        assert!(text.lines().any(|l| l == "FUSEFORKS_SECRET_MCP_B='now-here'"), "{text}");
        assert!(!text.contains("# FUSEFORKS_SECRET_MCP_B="), "{text}");
        assert!(text.lines().any(|l| l == "FUSEFORKS_SECRET_MCP_C='added'"), "{text}");
        assert_eq!(text.matches("FUSEFORKS_SECRET_JEV_API_TOKEN").count(), 1, "止めた名前を足さない");
        assert_eq!(report.written, ["FUSEFORKS_SECRET_MCP_B", "FUSEFORKS_SECRET_MCP_C"]);
        assert_eq!(report.differs, ["FUSEFORKS_SECRET_STUB"]);
        assert_eq!(report.unused, ["FUSEFORKS_SECRET_OLD"]);
        assert_eq!(report.commented_out, ["FUSEFORKS_SECRET_JEV_API_TOKEN"]);
        assert_eq!(report.tz_differs, Some(("UTC".to_owned(), "Asia/Tokyo".to_owned())));
        assert!(!report.door_token_created);
    }

    /// 引用符の付け方が違うだけなら「違う」にしない（compose と同じ規則で読んでから比べる）。
    #[test]
    fn quoting_alone_is_not_a_difference() {
        let wanted = [w("FUSEFORKS_SECRET_STUB", Kind::Template, Some("same"))];
        for existing in ["FUSEFORKS_SECRET_STUB=same\n", "FUSEFORKS_SECRET_STUB=\"same\"\n", "FUSEFORKS_SECRET_STUB='same'\n"] {
            let (_, report) = plan(Some(existing), &wanted, &mut token());
            assert!(report.differs.is_empty(), "{existing}");
        }
    }
}
