//! 判断役の問いと規則（Spec 62・`judge_contract`）。
//!
//! **純機構** — ファイルの I/O も Jev も知らない。`judge.toml` の文字列を受けて検査済みの
//! [`JudgeFile`] を返し、Jev の答え（[`Answer`]）を受けて当たった規則を返すだけ。
//!
//! 規則の評価は**決定的**。同じ答えなら必ず同じ規則に当たる。揺らぐのは Jev の答えだけ。
//!
//! 条件式は**閉じた小さな文法**（比較・`in`・`and` / `or` / `not`・括弧、値は `q` / `q.p` /
//! `q.margin` / `q.mean`）。四則演算・関数・変数・文字列の加工は持たない — if 文より大きな言語に
//! すると台本を書く言語になり、`run` が 3 rev かけて退けた「汎用インタプリタ」と同じ種類の穴が開く。
//! **型と定義域の合わない式は読み込み（= 保存時）に拒否する。** 実行時に「偽」として黙って通すと、
//! 書き手の意図と違う枝へ進んでも気づけない。

use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;

use crate::model::AgentId;

/// Choice の選択肢の数の下限。**Jev は 1 択を受ける**（P0 実測）が、`.margin` の 2 位が無くなるので
/// 本 Spec の規則として 2 以上にする。
pub const CHOICE_MIN: usize = 2;
/// Choice の選択肢の数の上限（Jev の上限）。
pub const CHOICE_MAX: usize = 255;
/// Score の段階の数の下限（Jev の上限。1 は 400 — P0 実測）。
pub const SCORE_MIN: usize = 2;
/// Score の段階の数の上限（Jev の上限。11 は 400 — P0 実測）。
pub const SCORE_MAX: usize = 10;
/// `note` の上限。**文字数 = Unicode のコードポイント数**（`len()` で数えると日本語では枠の 1/3）。
pub const MAX_NOTE_CHARS: usize = 200;
/// 式の予約語。問いの名前と Choice の鍵には使えない（式が読めなくなる）。
pub const RESERVED: [&str; 4] = ["and", "or", "not", "in"];

// ---------------------------------------------------------------------------
// 検査済みの形
// ---------------------------------------------------------------------------

/// 検査を通った `judge.toml`。
#[derive(Debug, Clone, PartialEq)]
pub struct JudgeFile {
    /// 問い。名前の順（`BTreeMap`）— Jev は問いを独立に評価するので順は意味を持たない。
    pub questions: BTreeMap<String, Question>,
    /// 規則。**ファイルの順**に上から評価する。
    pub rules: Vec<Rule>,
    /// どの規則にも当たらなかったとき。
    pub otherwise: Action,
}

/// 問い 1 つ。
#[derive(Debug, Clone, PartialEq)]
pub enum Question {
    /// 順序の無い選択肢から 1 つ。`options` は**ファイルの順**（鍵, 説明文）。
    Choice {
        /// 問いの文面。
        ask: String,
        /// （鍵, 説明文）。ファイルの順。
        options: Vec<(String, String)>,
    },
    /// 順序つきの段階。`levels` は低い順。規則では 1 始まり。
    Score {
        /// 問いの文面。
        ask: String,
        /// 段階の説明文。低い順。
        levels: Vec<String>,
    },
    /// 命題が成り立つ確率。
    Noul {
        /// 問いの文面。
        ask: String,
        /// 真と判定する条件の一文。
        true_if: String,
        /// 偽と判定する条件の一文。
        false_if: String,
    },
}

impl Question {
    fn kind(&self) -> &'static str {
        match self {
            Self::Choice { .. } => "choice",
            Self::Score { .. } => "score",
            Self::Noul { .. } => "noul",
        }
    }
}

/// 規則 1 つ。
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    /// 構文解析と型検査を通った条件。
    pub when: Expr,
    /// 書き手が書いた式の文字列（画面と「試す」の表示用）。
    pub source: String,
    /// 当たったときにすること。
    pub action: Action,
}

/// 規則が当たったとき（または `otherwise`）にすること。
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    /// 行き先。
    pub target: Target,
    /// 添え書き。200 字まで。
    pub note: Option<String>,
}

/// 行き先。
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// サーヴァントへ渡す。**1 体以上・重複なし**。1 体なら中継、2 体以上なら撒いて束ねる。
    To(Vec<AgentId>),
    /// 誰にも渡さず、判定 1 行と `note` を呼び出し元へ返す。
    Return,
}

/// 検査に落ちた理由。`location` は `questions.kind` / `rules[2].when` / `otherwise` / `TOML` の形。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JudgeFileError {
    /// どこで落ちたか（`questions.kind` / `rules[2].when` / `otherwise` / `TOML`）。
    pub location: String,
    /// 何が悪いか（日本語。画面に出す）。
    pub message: String,
}

impl JudgeFileError {
    fn new(location: impl Into<String>, message: impl Into<String>) -> Self {
        Self { location: location.into(), message: message.into() }
    }
}

impl fmt::Display for JudgeFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.location, self.message)
    }
}

impl std::error::Error for JudgeFileError {}

// ---------------------------------------------------------------------------
// 読み込み（TOML → 検査済みの形）
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default)]
    questions: BTreeMap<String, RawQuestion>,
    #[serde(default)]
    rules: Vec<RawRule>,
    otherwise: Option<RawAction>,
}

/// 型ごとの欄を全部 `Option` で受け、型に合わない欄は**名指しで**拒否する（内部タグの enum に
/// `deny_unknown_fields` を掛けると、どの欄が悪いかを言わない）。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawQuestion {
    #[serde(rename = "type")]
    ty: String,
    ask: String,
    /// **`serde_json::Map` で受けるのはファイルの順を保つため**（ワークスペースの serde_json は
    /// `preserve_order`）。`BTreeMap` だと選択肢の並びが鍵の文字順に化ける。
    options: Option<serde_json::Map<String, serde_json::Value>>,
    levels: Option<Vec<String>>,
    true_if: Option<String>,
    false_if: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    when: String,
    to: Option<Vec<String>>,
    #[serde(rename = "do")]
    do_: Option<String>,
    note: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAction {
    to: Option<Vec<String>>,
    #[serde(rename = "do")]
    do_: Option<String>,
    note: Option<String>,
}

/// 名前と Choice の鍵の規則: `[a-z][a-z0-9_]*`・予約語は不可。
fn is_valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some('a'..='z'))
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_'))
        && !RESERVED.contains(&name)
}

fn name_rule_message(what: &str, name: &str) -> String {
    if RESERVED.contains(&name) {
        format!("{what}「{name}」は式の予約語（and / or / not / in）なので使えません")
    } else {
        format!("{what}「{name}」は英小文字で始まり、英小文字・数字・_ だけで書いてください")
    }
}

fn nonblank(location: &str, field: &str, value: &str) -> Result<(), JudgeFileError> {
    if value.trim().is_empty() {
        Err(JudgeFileError::new(location, format!("{field} が空です")))
    } else {
        Ok(())
    }
}

impl JudgeFile {
    /// `judge.toml` の本文を読み、形・文法・型・定義域を検査する。
    ///
    /// **`to` のサーヴァントが実在するかは見ない**（村の状態に依存するので [`Self::missing_targets`]
    /// が別に持つ）。
    ///
    /// # Errors
    ///
    /// 最初に見つかった 1 つを返す。
    pub fn parse(text: &str) -> Result<Self, JudgeFileError> {
        let raw: RawFile = toml_edit::de::from_str(text)
            .map_err(|e| JudgeFileError::new("TOML", e.to_string().trim().to_owned()))?;

        if raw.questions.is_empty() {
            return Err(JudgeFileError::new("questions", "問いが 1 つもありません"));
        }
        let mut questions = BTreeMap::new();
        for (name, q) in raw.questions {
            let location = format!("questions.{name}");
            if !is_valid_name(&name) {
                return Err(JudgeFileError::new(location, name_rule_message("問いの名前", &name)));
            }
            let question = build_question(&location, q)?;
            questions.insert(name, question);
        }

        let mut rules = Vec::with_capacity(raw.rules.len());
        for (index, rule) in raw.rules.into_iter().enumerate() {
            let location = format!("rules[{}]", index + 1);
            let when = parse_expr(&rule.when)
                .and_then(|expr| type_check(&expr, &questions).map(|()| expr))
                .map_err(|m| JudgeFileError::new(format!("{location}.when"), m))?;
            let action = build_action(&location, rule.to, rule.do_, rule.note)?;
            rules.push(Rule { when, source: rule.when, action });
        }

        let otherwise = raw.otherwise.ok_or_else(|| {
            JudgeFileError::new("otherwise", "[otherwise] がありません（どの規則にも当たらなかったときの行き先）")
        })?;
        let otherwise = build_action("otherwise", otherwise.to, otherwise.do_, otherwise.note)?;

        Ok(Self { questions, rules, otherwise })
    }

    /// `to` に書かれた ID のうち、`is_servant` が偽を返すもの（重複なし・出現順）。
    ///
    /// 判断役の ID もここで落ちる（判断役はサーヴァントではない）。
    pub fn missing_targets(&self, is_servant: impl Fn(&AgentId) -> bool) -> Vec<AgentId> {
        let mut missing: Vec<AgentId> = Vec::new();
        for id in self.targets() {
            if !is_servant(id) && !missing.contains(id) {
                missing.push(id.clone());
            }
        }
        missing
    }

    /// 概形（地図のホバー）。**問いの文面・選択肢・規則の中身は出さない** — 名前と型と数だけ。
    pub fn outline(&self) -> JudgeOutline {
        JudgeOutline {
            questions: self
                .questions
                .iter()
                .map(|(name, q)| QuestionOutline { name: name.clone(), kind: q.question_kind() })
                .collect(),
            rules: self.rules.len(),
        }
    }

    /// 行き先になりうる相手（全規則と `otherwise` の `to` の和・重複なし・出現順）。
    pub fn targets(&self) -> Vec<&AgentId> {
        let mut out: Vec<&AgentId> = Vec::new();
        let actions = self.rules.iter().map(|r| &r.action).chain(std::iter::once(&self.otherwise));
        for action in actions {
            if let Target::To(ids) = &action.target {
                for id in ids {
                    if !out.contains(&id) {
                        out.push(id);
                    }
                }
            }
        }
        out
    }
}

fn build_question(location: &str, q: RawQuestion) -> Result<Question, JudgeFileError> {
    nonblank(location, "ask", &q.ask)?;
    let stray = |field: &str| {
        JudgeFileError::new(location, format!("type = \"{}\" の問いに {field} は書けません", q.ty))
    };
    match q.ty.as_str() {
        "choice" => {
            if q.levels.is_some() {
                return Err(stray("levels"));
            }
            if q.true_if.is_some() || q.false_if.is_some() {
                return Err(stray("true_if / false_if"));
            }
            let options = q
                .options
                .ok_or_else(|| JudgeFileError::new(location, "choice の問いに options がありません"))?;
            if !(CHOICE_MIN..=CHOICE_MAX).contains(&options.len()) {
                return Err(JudgeFileError::new(
                    location,
                    format!("options は {CHOICE_MIN}〜{CHOICE_MAX} 個です（{} 個）", options.len()),
                ));
            }
            let mut out = Vec::with_capacity(options.len());
            for (key, value) in options {
                if !is_valid_name(&key) {
                    return Err(JudgeFileError::new(location, name_rule_message("選択肢の鍵", &key)));
                }
                let serde_json::Value::String(text) = value else {
                    return Err(JudgeFileError::new(location, format!("選択肢「{key}」の説明文は文字列で書いてください")));
                };
                nonblank(location, &format!("選択肢「{key}」の説明文"), &text)?;
                out.push((key, text));
            }
            Ok(Question::Choice { ask: q.ask, options: out })
        }
        "score" => {
            if q.options.is_some() {
                return Err(stray("options"));
            }
            if q.true_if.is_some() || q.false_if.is_some() {
                return Err(stray("true_if / false_if"));
            }
            let levels = q
                .levels
                .ok_or_else(|| JudgeFileError::new(location, "score の問いに levels がありません"))?;
            if !(SCORE_MIN..=SCORE_MAX).contains(&levels.len()) {
                return Err(JudgeFileError::new(
                    location,
                    format!("levels は {SCORE_MIN}〜{SCORE_MAX} 段階です（{} 段階）", levels.len()),
                ));
            }
            for (i, text) in levels.iter().enumerate() {
                nonblank(location, &format!("levels の {} 段階目", i + 1), text)?;
            }
            Ok(Question::Score { ask: q.ask, levels })
        }
        "noul" => {
            if q.options.is_some() {
                return Err(stray("options"));
            }
            if q.levels.is_some() {
                return Err(stray("levels"));
            }
            let (Some(true_if), Some(false_if)) = (q.true_if, q.false_if) else {
                return Err(JudgeFileError::new(location, "noul の問いには true_if と false_if の両方が要ります"));
            };
            nonblank(location, "true_if", &true_if)?;
            nonblank(location, "false_if", &false_if)?;
            Ok(Question::Noul { ask: q.ask, true_if, false_if })
        }
        other => Err(JudgeFileError::new(
            location,
            format!("type = \"{other}\" は使えません（choice / score / noul のどれか）"),
        )),
    }
}

fn build_action(
    location: &str,
    to: Option<Vec<String>>,
    do_: Option<String>,
    note: Option<String>,
) -> Result<Action, JudgeFileError> {
    let target = match (to, do_) {
        (Some(_), Some(_)) => {
            return Err(JudgeFileError::new(location, "to と do は同時に書けません（どちらか 1 つ）"));
        }
        (None, None) => {
            return Err(JudgeFileError::new(location, "to か do = \"return\" のどちらかが要ります"));
        }
        (None, Some(d)) if d == "return" => Target::Return,
        (None, Some(d)) => {
            return Err(JudgeFileError::new(location, format!("do = \"{d}\" は使えません（\"return\" だけ）")));
        }
        (Some(ids), None) => {
            if ids.is_empty() {
                return Err(JudgeFileError::new(location, "to が空です（渡さないなら do = \"return\"）"));
            }
            let mut out: Vec<AgentId> = Vec::with_capacity(ids.len());
            for raw in ids {
                let id = AgentId::new(raw.clone());
                if !id.is_safe() {
                    return Err(JudgeFileError::new(location, format!("to の「{raw}」は ID として使えない文字を含みます")));
                }
                if out.contains(&id) {
                    return Err(JudgeFileError::new(location, format!("to に「{raw}」が 2 回あります")));
                }
                out.push(id);
            }
            Target::To(out)
        }
    };
    if let Some(n) = &note {
        let count = n.chars().count();
        if count > MAX_NOTE_CHARS {
            return Err(JudgeFileError::new(
                location,
                format!("note は {MAX_NOTE_CHARS} 字までです（{count} 字）"),
            ));
        }
    }
    Ok(Action { target, note })
}

// ---------------------------------------------------------------------------
// 条件式
// ---------------------------------------------------------------------------

/// 条件式。
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// どれかが真。
    Or(Vec<Expr>),
    /// 全部が真。
    And(Vec<Expr>),
    /// 否定。
    Not(Box<Expr>),
    /// 比較。
    Cmp {
        /// 左辺。
        value: ValueRef,
        /// 演算子。
        op: CmpOp,
        /// 右辺。
        literal: Literal,
    },
    /// `value in [...]`。左辺は Choice か Score の `q` だけ（型検査が保証する）。
    In {
        /// 左辺。
        value: ValueRef,
        /// 候補（1 つ以上）。
        literals: Vec<Literal>,
    },
}

/// 値の参照（`kind` / `kind.p` / `kind.margin` / `size.mean`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueRef {
    /// 問いの名前。
    pub question: String,
    /// 欄。
    pub field: Field,
}

/// 値の欄。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// `q` — Choice の鍵 / Score の段階の番号 / Noul の確率。
    Value,
    /// `q.p` — 選ばれた鍵・段階の確率。
    P,
    /// `q.margin` — 1 位と 2 位の確率の差。
    Margin,
    /// `q.mean` — Score の期待値（1 始まり）。
    Mean,
}

impl Field {
    fn suffix(self) -> &'static str {
        match self {
            Self::Value => "",
            Self::P => ".p",
            Self::Margin => ".margin",
            Self::Mean => ".mean",
        }
    }
}

/// 比較演算子。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    /// `==`
    Eq,
    /// `!=`
    Ne,
    /// `>=`
    Ge,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `<`
    Lt,
}

impl CmpOp {
    fn is_equality(self) -> bool {
        matches!(self, Self::Eq | Self::Ne)
    }
}

/// 右辺。
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    /// 数。`integer` は小数点を書かなかったか（Score の段階の番号は整数で書く）。
    Number {
        /// 値。
        value: f64,
        /// 小数点を書かなかったか。
        integer: bool,
    },
    /// 名前（Choice の鍵）。
    Name(String),
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    Number { value: f64, integer: bool },
    Dot,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Op(CmpOp),
}

fn tokenize(src: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = src.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\n' | '\r' => i += 1,
            '(' => {
                tokens.push(Token::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(Token::RParen);
                i += 1;
            }
            '[' => {
                tokens.push(Token::LBracket);
                i += 1;
            }
            ']' => {
                tokens.push(Token::RBracket);
                i += 1;
            }
            ',' => {
                tokens.push(Token::Comma);
                i += 1;
            }
            '.' => {
                tokens.push(Token::Dot);
                i += 1;
            }
            '=' | '!' | '>' | '<' => {
                let next = chars.get(i + 1).copied();
                let (op, len) = match (c, next) {
                    ('=', Some('=')) => (CmpOp::Eq, 2),
                    ('!', Some('=')) => (CmpOp::Ne, 2),
                    ('>', Some('=')) => (CmpOp::Ge, 2),
                    ('<', Some('=')) => (CmpOp::Le, 2),
                    ('>', _) => (CmpOp::Gt, 1),
                    ('<', _) => (CmpOp::Lt, 1),
                    _ => return Err(format!("「{c}」は使えません（比較は == != >= <= > <）")),
                };
                tokens.push(Token::Op(op));
                i += len;
            }
            '0'..='9' => {
                let start = i;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                let mut integer = true;
                // 小数点は後ろに数字が続くときだけ数の一部（`size.mean` の `.` と区別する）。
                if i + 1 < chars.len() && chars[i] == '.' && chars[i + 1].is_ascii_digit() {
                    integer = false;
                    i += 1;
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                let text: String = chars[start..i].iter().collect();
                let value: f64 = text.parse().map_err(|_| format!("数「{text}」が読めません"))?;
                tokens.push(Token::Number { value, integer });
            }
            'a'..='z' | 'A'..='Z' | '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                tokens.push(Token::Ident(chars[start..i].iter().collect()));
            }
            other => return Err(format!("「{other}」は式に書けません")),
        }
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn keyword(&self, word: &str) -> bool {
        matches!(self.peek(), Some(Token::Ident(w)) if w == word)
    }

    fn expr(&mut self) -> Result<Expr, String> {
        let mut items = vec![self.and()?];
        while self.keyword("or") {
            self.pos += 1;
            items.push(self.and()?);
        }
        Ok(if items.len() == 1 { items.pop().expect("1 要素") } else { Expr::Or(items) })
    }

    fn and(&mut self) -> Result<Expr, String> {
        let mut items = vec![self.not()?];
        while self.keyword("and") {
            self.pos += 1;
            items.push(self.not()?);
        }
        Ok(if items.len() == 1 { items.pop().expect("1 要素") } else { Expr::And(items) })
    }

    fn not(&mut self) -> Result<Expr, String> {
        if self.keyword("not") {
            self.pos += 1;
            return Ok(Expr::Not(Box::new(self.not()?)));
        }
        self.atom()
    }

    fn atom(&mut self) -> Result<Expr, String> {
        if matches!(self.peek(), Some(Token::LParen)) {
            self.pos += 1;
            let inner = self.expr()?;
            return match self.next() {
                Some(Token::RParen) => Ok(inner),
                _ => Err("「(」に対応する「)」がありません".to_owned()),
            };
        }
        let value = self.value()?;
        if self.keyword("in") {
            self.pos += 1;
            if !matches!(self.next(), Some(Token::LBracket)) {
                return Err("in の後には [ … ] を書いてください".to_owned());
            }
            let mut literals = vec![self.literal()?];
            loop {
                match self.next() {
                    Some(Token::Comma) => literals.push(self.literal()?),
                    Some(Token::RBracket) => break,
                    _ => return Err("in の [ … ] が閉じていません".to_owned()),
                }
            }
            return Ok(Expr::In { value, literals });
        }
        let op = match self.next() {
            Some(Token::Op(op)) => op,
            _ => {
                return Err(format!(
                    "「{}{}」の後に比較（== != >= <= > <）か in が要ります",
                    value.question,
                    value.field.suffix()
                ));
            }
        };
        let literal = self.literal()?;
        Ok(Expr::Cmp { value, op, literal })
    }

    fn value(&mut self) -> Result<ValueRef, String> {
        let question = match self.next() {
            Some(Token::Ident(name)) if !RESERVED.contains(&name.as_str()) => name,
            Some(Token::Ident(name)) => return Err(format!("「{name}」の位置には問いの名前が要ります")),
            _ => return Err("問いの名前が要ります".to_owned()),
        };
        let field = if matches!(self.peek(), Some(Token::Dot)) {
            self.pos += 1;
            match self.next() {
                Some(Token::Ident(f)) if f == "p" => Field::P,
                Some(Token::Ident(f)) if f == "margin" => Field::Margin,
                Some(Token::Ident(f)) if f == "mean" => Field::Mean,
                Some(Token::Ident(f)) => {
                    return Err(format!("「{question}.{f}」は書けません（.p / .margin / .mean のどれか）"));
                }
                _ => return Err(format!("「{question}.」の後に p / margin / mean が要ります")),
            }
        } else {
            Field::Value
        };
        Ok(ValueRef { question, field })
    }

    fn literal(&mut self) -> Result<Literal, String> {
        match self.next() {
            Some(Token::Number { value, integer }) => Ok(Literal::Number { value, integer }),
            Some(Token::Ident(name)) if !RESERVED.contains(&name.as_str()) => Ok(Literal::Name(name)),
            _ => Err("比較の右辺には数か選択肢の鍵を書いてください".to_owned()),
        }
    }
}

/// 式を構文解析する（型は見ない）。
fn parse_expr(src: &str) -> Result<Expr, String> {
    let tokens = tokenize(src)?;
    if tokens.is_empty() {
        return Err("when が空です".to_owned());
    }
    let mut parser = Parser { tokens, pos: 0 };
    let expr = parser.expr()?;
    if parser.pos < parser.tokens.len() {
        return Err("式の後ろに余計なものがあります（and / or でつないでください）".to_owned());
    }
    Ok(expr)
}

fn type_check(expr: &Expr, questions: &BTreeMap<String, Question>) -> Result<(), String> {
    match expr {
        Expr::Or(items) | Expr::And(items) => items.iter().try_for_each(|e| type_check(e, questions)),
        Expr::Not(inner) => type_check(inner, questions),
        Expr::Cmp { value, op, literal } => check_cmp(value, Some(*op), std::slice::from_ref(literal), questions),
        Expr::In { value, literals } => check_cmp(value, None, literals, questions),
    }
}

/// `op` が `None` なら `in`。
fn check_cmp(
    value: &ValueRef,
    op: Option<CmpOp>,
    literals: &[Literal],
    questions: &BTreeMap<String, Question>,
) -> Result<(), String> {
    let name = &value.question;
    let shown = format!("{name}{}", value.field.suffix());
    let question = questions
        .get(name)
        .ok_or_else(|| format!("問い「{name}」は questions にありません"))?;
    let kind = question.kind();
    let not_allowed = || format!("{kind} の問いに「{shown}」は書けません");

    // 確率（0〜1）を右辺に取る比較。
    let probability = |op: Option<CmpOp>| -> Result<(), String> {
        if op.is_none() {
            return Err(format!("「{shown}」は確率なので in は使えません"));
        }
        for literal in literals {
            match literal {
                Literal::Number { value, .. } if (0.0..=1.0).contains(value) => {}
                Literal::Number { value, .. } => {
                    return Err(format!("「{shown}」は 0〜1 の確率です（{value} は範囲の外）"));
                }
                Literal::Name(n) => return Err(format!("「{shown}」は確率なので「{n}」とは比べられません")),
            }
        }
        Ok(())
    };

    match (question, value.field) {
        (Question::Choice { options, .. }, Field::Value) => {
            if let Some(op) = op
                && !op.is_equality()
            {
                return Err(format!("choice の「{shown}」は == / != / in だけで比べます"));
            }
            for literal in literals {
                match literal {
                    Literal::Name(key) if options.iter().any(|(k, _)| k == key) => {}
                    Literal::Name(key) => return Err(format!("問い「{name}」に選択肢「{key}」はありません")),
                    Literal::Number { .. } => {
                        return Err(format!("choice の「{shown}」は選択肢の鍵と比べます（数は書けません）"));
                    }
                }
            }
            Ok(())
        }
        (Question::Choice { .. } | Question::Score { .. }, Field::P | Field::Margin) => probability(op),
        (Question::Choice { .. }, Field::Mean) => Err(not_allowed()),
        (Question::Score { levels, .. }, Field::Value) => {
            let n = levels.len();
            for literal in literals {
                match literal {
                    Literal::Number { value, integer: true } if *value >= 1.0 && *value <= n as f64 => {}
                    Literal::Number { integer: false, .. } => {
                        return Err(format!("score の「{shown}」は段階の番号なので整数で書いてください"));
                    }
                    Literal::Number { value, .. } => {
                        return Err(format!("問い「{name}」の段階は 1〜{n} です（{value} は範囲の外）"));
                    }
                    Literal::Name(n2) => return Err(format!("score の「{shown}」は段階の番号なので「{n2}」とは比べられません")),
                }
            }
            Ok(())
        }
        (Question::Score { levels, .. }, Field::Mean) => {
            if op.is_none() {
                return Err(format!("「{shown}」は小数なので in は使えません"));
            }
            let n = levels.len() as f64;
            for literal in literals {
                match literal {
                    Literal::Number { value, .. } if *value >= 1.0 && *value <= n => {}
                    Literal::Number { value, .. } => {
                        return Err(format!("「{shown}」は 1〜{n} の値です（{value} は範囲の外）"));
                    }
                    Literal::Name(n2) => return Err(format!("「{shown}」は数なので「{n2}」とは比べられません")),
                }
            }
            Ok(())
        }
        (Question::Noul { .. }, Field::Value) => probability(op),
        (Question::Noul { .. }, Field::P | Field::Margin | Field::Mean) => Err(not_allowed()),
    }
}

// ---------------------------------------------------------------------------
// 判断モデルの口（コアは trait だけを知る — D11）
// ---------------------------------------------------------------------------

/// 判断モデルへの 1 回の問い合わせの結果。
#[derive(Debug, Clone, PartialEq)]
pub struct JudgeReport {
    /// サーバーが名乗ったモデル版（例 `jev-1.13.0`）。
    pub model: String,
    /// 問いの名前 → 答え。**未回答の問いは載せない**（0 に潰さない）。
    pub answers: BTreeMap<String, Answer>,
    /// 入力トークン（計器へ出す。予算には入れない）。
    pub input_tokens: u64,
    /// 出力トークン（Jev は出力が無料でも数を返す — P0 実測）。
    pub output_tokens: u64,
}

/// 判断モデルに判定させられなかった理由。**文面は計器に出さない**（#71）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JudgeError {
    /// 入力が上限を超えた（Jev の 400 `max_tokens_exceeded` — P0 実測）。
    TooLarge,
    /// それ以外の失敗（接続・ステータス・解釈）。中身は診断用。
    Failed(String),
}

/// 判断モデルの口。コアはこの trait だけを知り、Jev の実装は GUI 側が差し込む
/// （Spec 59 の `ParagraphScorer` と同じ形。鍵も共有する）。
///
/// **締め切りは呼び出し側が持つ**（`tokio::time::timeout`）— 口の中で待ちを抱えない。
#[async_trait::async_trait]
pub trait Judge: Send + Sync {
    /// 全部の問いを 1 回で判定させる（`state = { "message": message }`）。
    async fn judge(
        &self,
        message: &str,
        questions: &BTreeMap<String, Question>,
    ) -> Result<JudgeReport, JudgeError>;
}

// ---------------------------------------------------------------------------
// 答えと評価
// ---------------------------------------------------------------------------

/// Jev の答え 1 つ（コアが受けた形のまま）。
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// `choice` と選択肢ごとの確率。
    Choice {
        /// Jev が選んだ鍵。
        choice: String,
        /// 鍵ごとの確率。
        probabilities: BTreeMap<String, f64>,
    },
    /// **0 始まりの**期待値と、段階ごとの確率（index = 0 始まりの段階）。
    Score {
        /// Jev の `score`（0 始まりの期待値）。
        mean0: f64,
        /// 段階ごとの確率（index = 0 始まりの段階）。
        probabilities: Vec<f64>,
    },
    /// 確率。
    Noul(f64),
}

/// 規則が読む値（答えから求めた形）。
#[derive(Debug, Clone, PartialEq)]
pub enum Resolved {
    /// Choice。
    Choice {
        /// 選ばれた鍵。
        key: String,
        /// その確率。
        p: f64,
        /// 1 位と 2 位の確率の差。
        margin: f64,
    },
    /// `level` は 1 始まり。`mean` も 1 始まり。
    Score {
        /// 確率が最も高い段階（1 始まり）。
        level: u32,
        /// その確率。
        p: f64,
        /// 1 位と 2 位の確率の差。
        margin: f64,
        /// 期待値（1 始まり）。
        mean: f64,
    },
    /// Noul。
    Noul {
        /// 確率。
        p: f64,
    },
}

/// 1 位と 2 位の差。1 つしか無ければ 1 位そのもの。
fn margin_of(mut probs: Vec<f64>) -> f64 {
    probs.sort_by(|a, b| b.total_cmp(a));
    match probs.as_slice() {
        [first, second, ..] => first - second,
        [first] => *first,
        [] => 0.0,
    }
}

/// 答えを規則が読む値へ直す。問いと答えの型が食い違うか、Score の段階の数が合わなければ `None`
/// （＝未回答として扱う。0 に潰さない）。
fn resolve(question: &Question, answer: &Answer) -> Option<Resolved> {
    match (question, answer) {
        (Question::Choice { options, .. }, Answer::Choice { choice, probabilities }) => {
            if !options.iter().any(|(k, _)| k == choice) {
                return None;
            }
            let p = probabilities.get(choice).copied().unwrap_or(0.0);
            Some(Resolved::Choice {
                key: choice.clone(),
                p,
                margin: margin_of(probabilities.values().copied().collect()),
            })
        }
        (Question::Score { levels, .. }, Answer::Score { mean0, probabilities }) => {
            if probabilities.len() != levels.len() {
                return None;
            }
            // 確率が最も高い段階。**同じ確率なら低い段階**（最初に見つかったもの）。
            let mut best = 0usize;
            for (i, p) in probabilities.iter().enumerate() {
                if *p > probabilities[best] {
                    best = i;
                }
            }
            #[allow(clippy::cast_possible_truncation, reason = "段階は 10 まで")]
            let level = best as u32 + 1;
            Some(Resolved::Score {
                level,
                p: probabilities[best],
                margin: margin_of(probabilities.clone()),
                mean: mean0 + 1.0,
            })
        }
        (Question::Noul { .. }, Answer::Noul(p)) => Some(Resolved::Noul { p: *p }),
        _ => None,
    }
}

/// どの規則に当たったか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Picked {
    /// 規則の番号（**1 始まり** — ファイルの上からの順。ログの `rule=` と画面がこれで数える）。
    Rule(usize),
    /// `[otherwise]`。
    Otherwise,
}

/// 評価の結果。
#[derive(Debug, Clone, PartialEq)]
pub struct Decision<'a> {
    /// 当たった規則。
    pub picked: Picked,
    /// すること。
    pub action: &'a Action,
    /// 規則が読んだ値（問いの名前の順）。判定の行と計器に使う。
    pub values: BTreeMap<String, Resolved>,
}

impl JudgeFile {
    /// 規則を上から評価し、最初に当たった 1 つ（無ければ `otherwise`）を返す。
    ///
    /// # Errors
    ///
    /// **どれか 1 つの問いが未回答**（または答えの型が問いと食い違う）なら、規則を評価せずに
    /// その問いの名前を返す — 規則が参照していない問いでも同じ（未回答を 0 に潰さない）。
    pub fn evaluate(&self, answers: &BTreeMap<String, Answer>) -> Result<Decision<'_>, String> {
        let mut values = BTreeMap::new();
        for (name, question) in &self.questions {
            let resolved = answers.get(name).and_then(|a| resolve(question, a)).ok_or_else(|| name.clone())?;
            values.insert(name.clone(), resolved);
        }
        for (index, rule) in self.rules.iter().enumerate() {
            if eval(&rule.when, &values) {
                return Ok(Decision { picked: Picked::Rule(index + 1), action: &rule.action, values });
            }
        }
        Ok(Decision { picked: Picked::Otherwise, action: &self.otherwise, values })
    }
}

fn number(literal: &Literal) -> f64 {
    match literal {
        Literal::Number { value, .. } => *value,
        // 型検査を通った式では起きない。起きても「当たらない」側へ倒す値を返す。
        Literal::Name(_) => f64::NAN,
    }
}

fn compare(left: f64, op: CmpOp, right: f64) -> bool {
    match op {
        CmpOp::Eq => left == right,
        CmpOp::Ne => left != right,
        CmpOp::Ge => left >= right,
        CmpOp::Le => left <= right,
        CmpOp::Gt => left > right,
        CmpOp::Lt => left < right,
    }
}

/// 値を数で取る（Choice の鍵は数ではないので `None`）。
fn numeric(value: &ValueRef, resolved: &Resolved) -> Option<f64> {
    match (resolved, value.field) {
        (Resolved::Choice { p, .. }, Field::P) | (Resolved::Score { p, .. }, Field::P) => Some(*p),
        (Resolved::Choice { margin, .. }, Field::Margin) | (Resolved::Score { margin, .. }, Field::Margin) => {
            Some(*margin)
        }
        (Resolved::Score { level, .. }, Field::Value) => Some(f64::from(*level)),
        (Resolved::Score { mean, .. }, Field::Mean) => Some(*mean),
        (Resolved::Noul { p }, Field::Value) => Some(*p),
        _ => None,
    }
}

fn eval(expr: &Expr, values: &BTreeMap<String, Resolved>) -> bool {
    match expr {
        Expr::Or(items) => items.iter().any(|e| eval(e, values)),
        Expr::And(items) => items.iter().all(|e| eval(e, values)),
        Expr::Not(inner) => !eval(inner, values),
        Expr::Cmp { value, op, literal } => {
            let Some(resolved) = values.get(&value.question) else {
                return false;
            };
            if let (Resolved::Choice { key, .. }, Field::Value, Literal::Name(name)) = (resolved, value.field, literal) {
                return match op {
                    CmpOp::Eq => key == name,
                    CmpOp::Ne => key != name,
                    _ => false,
                };
            }
            numeric(value, resolved).is_some_and(|left| compare(left, *op, number(literal)))
        }
        Expr::In { value, literals } => {
            let Some(resolved) = values.get(&value.question) else {
                return false;
            };
            if let (Resolved::Choice { key, .. }, Field::Value) = (resolved, value.field) {
                return literals.iter().any(|l| matches!(l, Literal::Name(n) if n == key));
            }
            numeric(value, resolved)
                .is_some_and(|left| literals.iter().any(|l| compare(left, CmpOp::Eq, number(l))))
        }
    }
}

// ---------------------------------------------------------------------------
// 有効の述語・表示・計器（純関数）
// ---------------------------------------------------------------------------

/// 判断役が有効か（`judge_contract` の**有効の述語は 1 つ**）。ツールを生やすか・左ペインの表示・
/// 地図の線はこの 1 つを読む — 別々に判定すると「ツールは生えないのに線は描かれる」が生まれる。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum JudgeStatus {
    /// 使える。
    Active,
    /// 利用者が無効にしている（左ペインのトグル。`JudgeSpec::enabled`）。
    Disabled,
    /// `judge.toml` が無い。
    NoFile,
    /// 検査に落ちている（手で編集したファイル）。
    #[serde(rename_all = "camelCase")]
    Invalid {
        /// どこで。
        location: String,
        /// 何が。
        message: String,
    },
    /// `to` にサーヴァントでない ID がある（消したサーヴァント・判断役の ID）。
    #[serde(rename_all = "camelCase")]
    MissingTargets {
        /// 名指し。
        targets: Vec<AgentId>,
    },
    /// 判断モデル（Jev）の設定が無い。
    NoJudgeModel,
}

/// 有効の述語（純関数）。判定の順は「利用者の無効 → ファイル → 検査 → 行き先 → 判断モデル」。
///
/// **利用者の無効を最初に見る** — 人が止めたものは、ほかの理由より先にそう読めるべき
/// （戻したあとに残りの理由が出る）。`file` は読み込みと検査の結果（`None` = ファイルが無い）。
pub fn status_of(
    enabled: bool,
    file: Option<&Result<JudgeFile, JudgeFileError>>,
    is_servant: impl Fn(&AgentId) -> bool,
    has_judge_model: bool,
) -> JudgeStatus {
    if !enabled {
        return JudgeStatus::Disabled;
    }
    match file {
        None => JudgeStatus::NoFile,
        Some(Err(e)) => JudgeStatus::Invalid { location: e.location.clone(), message: e.message.clone() },
        Some(Ok(parsed)) => {
            let missing = parsed.missing_targets(is_servant);
            if !missing.is_empty() {
                JudgeStatus::MissingTargets { targets: missing }
            } else if !has_judge_model {
                JudgeStatus::NoJudgeModel
            } else {
                JudgeStatus::Active
            }
        }
    }
}

impl JudgeStatus {
    /// 計器の `reason=` の語。
    pub fn label(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
            Self::NoFile => "no_file",
            Self::Invalid { .. } => "invalid",
            Self::MissingTargets { .. } => "missing_targets",
            Self::NoJudgeModel => "no_judge_model",
        }
    }
}

/// 問いの型（`judge.toml` の `type`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum QuestionKind {
    /// 順序の無い選択肢から 1 つ。
    Choice,
    /// 順序つきの段階。
    Score,
    /// 命題が成り立つ確率。
    Noul,
}

impl Question {
    /// 型。
    pub fn question_kind(&self) -> QuestionKind {
        match self {
            Self::Choice { .. } => QuestionKind::Choice,
            Self::Score { .. } => QuestionKind::Score,
            Self::Noul { .. } => QuestionKind::Noul,
        }
    }
}

/// 問い 1 つの概形。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionOutline {
    /// 問いの名前（規則の式で使う識別子）。
    pub name: String,
    /// 型。
    pub kind: QuestionKind,
}

/// `judge.toml` の概形（Spec 62 D9 — 地図のホバーに出す「問いの名前と型・規則の数」）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JudgeOutline {
    /// 問い。名前の順。
    pub questions: Vec<QuestionOutline>,
    /// `[[rules]]` の本数（`[otherwise]` は数えない — 必ず 1 つある）。
    pub rules: usize,
}

/// 判断役 1 つの一覧用の姿（IPC へ出す形）。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JudgeView {
    /// ID（サーヴァントと同じ名前空間）。
    pub id: AgentId,
    /// 表示名。
    pub name: String,
    /// 並び。
    pub order: u32,
    /// 利用者の有効/無効（`JudgeSpec::enabled` の写し）。画面が更新を組み直すときに要る —
    /// 無いと名前を直しただけで既定（有効）へ戻る（Spec 14 P1 の「投影から組み直して欄が消える」）。
    pub enabled: bool,
    /// 有効かどうか。
    pub status: JudgeStatus,
    /// 行き先になりうる相手（ファイルが読めたときだけ。地図の破線はここから描く）。
    pub targets: Vec<AgentId>,
    /// ファイルの概形。**`None` = ファイルが無いか検査に落ちている**（「読めない」と「0 本」を
    /// 同じ値に畳まない）。
    pub outline: Option<JudgeOutline>,
}

/// 規則が読んだ値を 1 つの文字列にする（計器の `answers=` と「試す」の表示）。
///
/// Choice は `鍵/p/margin`、Score は `段階/p/margin`、Noul は `確率`。**数値と鍵だけ**で、
/// 問いの文面は出さない（#71）。鍵は人が設定に書いた識別子。
pub fn answers_line(values: &BTreeMap<String, Resolved>) -> String {
    values
        .iter()
        .map(|(name, v)| match v {
            Resolved::Choice { key, p, margin } => format!("{name}:{key}/{p:.2}/{margin:.2}"),
            Resolved::Score { level, p, margin, .. } => format!("{name}:{level}/{p:.2}/{margin:.2}"),
            Resolved::Noul { p } => format!("{name}:{p:.2}"),
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// 中継する本文の末尾に添える判定 1 行（**封筒の寄せは呼び手が掛ける**）。
///
/// 例: `［判断: 振り分け役 → kind=research(0.82) size=3(0.60) risky=0.12］`
pub fn judgment_line(judge_name: &str, values: &BTreeMap<String, Resolved>, language: crate::world::Language) -> String {
    let parts: Vec<String> = values
        .iter()
        .map(|(name, v)| match v {
            Resolved::Choice { key, p, .. } => format!("{name}={key}({p:.2})"),
            Resolved::Score { level, p, .. } => format!("{name}={level}({p:.2})"),
            Resolved::Noul { p } => format!("{name}={p:.2}"),
        })
        .collect();
    match language {
        crate::world::Language::Ja => format!("［判断: {judge_name} → {}］", parts.join(" ")),
        crate::world::Language::En => format!("[Judgment: {judge_name} → {}]", parts.join(" ")),
    }
}

/// 新しい判断役の雛形（D10）。**`to` を書かない** — 雛形の時点ではどのサーヴァントが居るか
/// 分からず、存在しない ID を書くと保存の検査に落ちる。行き先はコメントで案内する。
pub fn starter_template(language: crate::world::Language) -> &'static str {
    match language {
        crate::world::Language::Ja => STARTER_JA,
        crate::world::Language::En => STARTER_EN,
    }
}

const STARTER_JA: &str = r#"# 判断役の問いと規則（Spec 62）。規則は上から評価し、最初に当たった 1 つだけを実行する。
# 行き先は to = ["サーヴァントの ID", …]（2 体以上なら撒いて束ねる）か do = "return"（渡さない）。

[questions.kind]
type = "choice"
ask = "依頼 `message` の主な作業の種類を選んでください。"
options = { research = "外部情報の調査・比較", implement = "コードの変更・実装", other = "上記のいずれにも当てはまらない" }

[[rules]]
when = "kind == other"
do = "return"
note = "どの作業にも当てはまらないので振り分けませんでした"

[[rules]]
# 例: do = "return" を to = ["agent_1"] に書き換えると、その相手へ渡る
when = "kind == research and kind.margin >= 0.2"
do = "return"

[otherwise]
do = "return"
"#;

const STARTER_EN: &str = r#"# Questions and rules of a judge (Spec 62). Rules are evaluated top-down; only the first match runs.
# A destination is to = ["servant id", ...] (2 or more fans out and bundles) or do = "return" (deliver nothing).

[questions.kind]
type = "choice"
ask = "Pick the main kind of work the request `message` asks for."
options = { research = "Research or comparison of outside information", implement = "Changing or implementing code", other = "None of the above" }

[[rules]]
when = "kind == other"
do = "return"
note = "Not routed: it matched none of the kinds"

[[rules]]
# Example: replace do = "return" with to = ["agent_1"] to deliver to that servant
when = "kind == research and kind.margin >= 0.2"
do = "return"

[otherwise]
do = "return"
"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec 62 D3 の例（そのまま）。
    const EXAMPLE: &str = r#"
[questions.kind]
type = "choice"
ask  = "依頼 `message` の主な作業の種類を選んでください。"
options = { research = "外部情報の調査・比較", implement = "コードの変更・実装", other = "上記のいずれにも当てはまらない" }

[questions.risky]
type = "noul"
ask  = "`message` は取り消せない操作を依頼していますか。"
true_if  = "削除・公開・送信など取り消せない操作を依頼している"
false_if = "読み取り・調査・下書きだけを依頼している"

[questions.size]
type = "score"
ask  = "`message` の作業量を評価してください。"
levels = ["1 回の検索で済む", "数件の資料を比べる", "複数の工程に分かれる"]

[[rules]]
when = "risky >= 0.7"
do   = "return"
note = "取り消せない操作を含むので振り分けませんでした"

[[rules]]
when = "kind == other"
do   = "return"

[[rules]]
when = "kind == research and size >= 3"
to   = ["agent_3", "agent_10"]

[[rules]]
when = "kind == research and kind.margin >= 0.2"
to   = ["agent_3"]

[[rules]]
when = "kind == implement"
to   = ["agent_10"]

[otherwise]
do = "return"
"#;

    fn parse_err(text: &str) -> JudgeFileError {
        JudgeFile::parse(text).expect_err("拒否されるはず")
    }

    /// 問い 1 つ（choice / score / noul を 1 つずつ）と `otherwise` だけの土台に規則を 1 本足す。
    fn with_rule(when: &str) -> String {
        format!(
            "{}\n[[rules]]\nwhen = {when:?}\ndo = \"return\"\n\n[otherwise]\ndo = \"return\"\n",
            EXAMPLE.split("[[rules]]").next().expect("土台")
        )
    }

    fn when_err(when: &str) -> String {
        let err = parse_err(&with_rule(when));
        assert_eq!(err.location, "rules[1].when", "{err}");
        err.message
    }

    fn answers(kind: (&str, &[(&str, f64)]), size: (f64, &[f64]), risky: f64) -> BTreeMap<String, Answer> {
        BTreeMap::from([
            (
                "kind".to_owned(),
                Answer::Choice {
                    choice: kind.0.to_owned(),
                    probabilities: kind.1.iter().map(|(k, p)| ((*k).to_owned(), *p)).collect(),
                },
            ),
            ("size".to_owned(), Answer::Score { mean0: size.0, probabilities: size.1.to_vec() }),
            ("risky".to_owned(), Answer::Noul(risky)),
        ])
    }

    #[test]
    fn the_spec_example_parses() {
        let file = JudgeFile::parse(EXAMPLE).expect("例は通る");
        assert_eq!(file.questions.len(), 3);
        assert_eq!(file.rules.len(), 5);
        assert_eq!(file.otherwise.target, Target::Return);
        // 選択肢はファイルの順（鍵の文字順ではない）。
        let Question::Choice { options, .. } = &file.questions["kind"] else { panic!("choice") };
        let keys: Vec<&str> = options.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["research", "implement", "other"]);
        assert_eq!(
            file.targets(),
            [&AgentId::new("agent_3"), &AgentId::new("agent_10")],
            "行き先は重複なし・出現順"
        );
    }

    #[test]
    fn the_outline_carries_names_kinds_and_the_rule_count_only() {
        let outline = JudgeFile::parse(EXAMPLE).unwrap().outline();
        let got: Vec<(&str, QuestionKind)> =
            outline.questions.iter().map(|q| (q.name.as_str(), q.kind)).collect();
        assert_eq!(
            got,
            [("kind", QuestionKind::Choice), ("risky", QuestionKind::Noul), ("size", QuestionKind::Score)],
            "名前の順"
        );
        assert_eq!(outline.rules, 5, "otherwise は数えない");
        // 画面へ出る形。問いの文面は載らない（#71 — 人が書いた文を必要のない面へ運ばない）。
        let wire = serde_json::to_string(&outline).unwrap();
        assert!(!wire.contains("取り消せない"), "{wire}");
        assert!(wire.contains(r#""kind":"noul""#), "{wire}");
    }

    #[test]
    fn rules_are_evaluated_top_down_and_the_first_match_wins() {
        let file = JudgeFile::parse(EXAMPLE).unwrap();
        // 調査で大きい → 規則 3（規則 4 にも当たるが 3 が先）。
        let a = answers(("research", &[("research", 0.9), ("implement", 0.1), ("other", 0.0)]), (1.9, &[0.01, 0.09, 0.9]), 0.1);
        let d = file.evaluate(&a).unwrap();
        assert_eq!(d.picked, Picked::Rule(3));
        assert_eq!(d.action.target, Target::To(vec![AgentId::new("agent_3"), AgentId::new("agent_10")]));
        // 取り消せない操作 → 規則 1 が先に当たる。
        let a = answers(("research", &[("research", 0.9), ("implement", 0.1), ("other", 0.0)]), (1.9, &[0.01, 0.09, 0.9]), 0.8);
        assert_eq!(file.evaluate(&a).unwrap().picked, Picked::Rule(1));
    }

    #[test]
    fn nothing_matches_goes_to_otherwise() {
        let file = JudgeFile::parse(EXAMPLE).unwrap();
        // 調査だが小さく、margin も低い → 規則 3・4 に当たらず otherwise。
        let a = answers(("research", &[("research", 0.5), ("implement", 0.4), ("other", 0.1)]), (0.2, &[0.8, 0.2, 0.0]), 0.1);
        let d = file.evaluate(&a).unwrap();
        assert_eq!(d.picked, Picked::Otherwise);
        assert_eq!(d.action.target, Target::Return);
    }

    #[test]
    fn an_unanswered_question_blocks_the_rules_even_when_no_rule_reads_it() {
        let file = JudgeFile::parse(EXAMPLE).unwrap();
        let mut a = answers(("implement", &[("implement", 1.0), ("research", 0.0), ("other", 0.0)]), (1.0, &[0.0, 1.0, 0.0]), 0.0);
        a.remove("size");
        assert_eq!(file.evaluate(&a).unwrap_err(), "size");
        // 答えの型が問いと食い違うのも未回答（0 に潰さない）。
        let mut a = answers(("implement", &[("implement", 1.0), ("research", 0.0), ("other", 0.0)]), (1.0, &[0.0, 1.0, 0.0]), 0.0);
        a.insert("risky".to_owned(), Answer::Score { mean0: 0.0, probabilities: vec![1.0, 0.0] });
        assert_eq!(file.evaluate(&a).unwrap_err(), "risky");
        // Score の段階の数が合わないのも未回答。
        let mut a = answers(("implement", &[("implement", 1.0), ("research", 0.0), ("other", 0.0)]), (1.0, &[0.0, 1.0, 0.0]), 0.0);
        a.insert("size".to_owned(), Answer::Score { mean0: 0.5, probabilities: vec![0.5, 0.5] });
        assert_eq!(file.evaluate(&a).unwrap_err(), "size");
    }

    #[test]
    fn a_score_resolves_to_the_most_probable_level_counting_from_one() {
        let file = JudgeFile::parse(EXAMPLE).unwrap();
        // P0 の実測値そのもの: 期待値 1.89（0 始まり）でも確率の 90% が最上段 → size = 3。
        let a = answers(("implement", &[("implement", 1.0), ("research", 0.0), ("other", 0.0)]), (1.89, &[0.01, 0.09, 0.9]), 0.08);
        let d = file.evaluate(&a).unwrap();
        let Resolved::Score { level, p, margin, mean } = d.values["size"] else { panic!("score") };
        assert_eq!(level, 3);
        assert!((p - 0.9).abs() < 1e-9);
        assert!((margin - 0.81).abs() < 1e-9);
        assert!((mean - 2.89).abs() < 1e-9, "mean は 1 始まり");
    }

    #[test]
    fn a_tie_picks_the_lower_level_and_margin_zero() {
        let q = Question::Score { ask: "x".into(), levels: vec!["a".into(), "b".into(), "c".into()] };
        let r = resolve(&q, &Answer::Score { mean0: 1.0, probabilities: vec![0.0, 0.5, 0.5] }).unwrap();
        assert_eq!(r, Resolved::Score { level: 2, p: 0.5, margin: 0.0, mean: 2.0 });
    }

    #[test]
    fn in_works_for_choice_keys_and_score_levels() {
        let text = with_rule("kind in [research, implement] and size in [2, 3]");
        let file = JudgeFile::parse(&text).unwrap();
        let hit = answers(("implement", &[("implement", 0.8), ("research", 0.1), ("other", 0.1)]), (1.0, &[0.1, 0.7, 0.2]), 0.0);
        assert_eq!(file.evaluate(&hit).unwrap().picked, Picked::Rule(1));
        let miss = answers(("implement", &[("implement", 0.8), ("research", 0.1), ("other", 0.1)]), (0.2, &[0.8, 0.1, 0.1]), 0.0);
        assert_eq!(file.evaluate(&miss).unwrap().picked, Picked::Otherwise);
    }

    #[test]
    fn not_or_and_parentheses_follow_precedence() {
        // not > and > or。`a or b and c` は `a or (b and c)`。
        let text = with_rule("risky > 0.9 or not (kind == other) and size.mean >= 2.5");
        let file = JudgeFile::parse(&text).unwrap();
        let a = answers(("research", &[("research", 1.0), ("implement", 0.0), ("other", 0.0)]), (1.6, &[0.0, 0.4, 0.6]), 0.0);
        assert_eq!(file.evaluate(&a).unwrap().picked, Picked::Rule(1), "mean 2.6 >= 2.5");
        let a = answers(("research", &[("research", 1.0), ("implement", 0.0), ("other", 0.0)]), (1.2, &[0.0, 0.8, 0.2]), 0.0);
        assert_eq!(file.evaluate(&a).unwrap().picked, Picked::Otherwise, "mean 2.2 < 2.5");
    }

    // ---- 形の拒否 ----

    #[test]
    fn otherwise_is_required() {
        let text = EXAMPLE.replace("[otherwise]\ndo = \"return\"\n", "");
        assert_eq!(parse_err(&text).location, "otherwise");
    }

    #[test]
    fn otherwise_may_route_but_may_not_have_when() {
        let text = EXAMPLE.replace("[otherwise]\ndo = \"return\"", "[otherwise]\nto = [\"agent_3\"]");
        assert_eq!(JudgeFile::parse(&text).unwrap().otherwise.target, Target::To(vec![AgentId::new("agent_3")]));
        let text = EXAMPLE.replace("[otherwise]\ndo = \"return\"", "[otherwise]\nwhen = \"risky > 0.5\"\ndo = \"return\"");
        assert_eq!(parse_err(&text).location, "TOML", "when は otherwise に書けない（未知の欄）");
    }

    #[test]
    fn names_and_choice_keys_reject_reserved_words_and_bad_characters() {
        let text = EXAMPLE.replace("[questions.risky]", "[questions.and]");
        assert!(parse_err(&text).message.contains("予約語"));
        let text = EXAMPLE.replace("other = \"上記", "in = \"上記");
        assert!(parse_err(&text).message.contains("予約語"));
        let text = EXAMPLE.replace("implement = ", "code-review = ");
        assert!(parse_err(&text).message.contains("英小文字"), "ハイフンは不可");
        let text = EXAMPLE.replace("[questions.size]", "[questions.Size]");
        assert!(parse_err(&text).message.contains("英小文字"));
    }

    #[test]
    fn choice_needs_two_options_and_score_two_to_ten_levels() {
        let text = EXAMPLE.replace(
            "options = { research = \"外部情報の調査・比較\", implement = \"コードの変更・実装\", other = \"上記のいずれにも当てはまらない\" }",
            "options = { research = \"調査\" }",
        );
        assert!(parse_err(&text).message.contains("2〜255"));
        let one = EXAMPLE.replace("levels = [\"1 回の検索で済む\", \"数件の資料を比べる\", \"複数の工程に分かれる\"]", "levels = [\"1 つ\"]");
        assert!(parse_err(&one).message.contains("2〜10"));
        let eleven: Vec<String> = (0..11).map(|i| format!("\"{i}\"")).collect();
        let eleven = EXAMPLE.replace(
            "levels = [\"1 回の検索で済む\", \"数件の資料を比べる\", \"複数の工程に分かれる\"]",
            &format!("levels = [{}]", eleven.join(", ")),
        );
        // 規則 3 の `size >= 3` はまだ範囲内なので、落ちるのは段階の数。
        assert!(parse_err(&eleven).message.contains("2〜10"));
    }

    #[test]
    fn fields_of_another_type_are_named() {
        let text = EXAMPLE.replace("type = \"noul\"", "type = \"noul\"\nlevels = [\"a\", \"b\"]");
        let err = parse_err(&text);
        assert_eq!(err.location, "questions.risky");
        assert!(err.message.contains("levels"), "{err}");
    }

    #[test]
    fn a_rule_needs_exactly_one_of_to_and_do() {
        let both = EXAMPLE.replacen("do   = \"return\"\nnote", "do   = \"return\"\nto = [\"agent_3\"]\nnote", 1);
        assert!(parse_err(&both).message.contains("同時"));
        let neither = EXAMPLE.replacen("do   = \"return\"\nnote", "note", 1);
        assert!(parse_err(&neither).message.contains("どちらか"));
        let other_do = EXAMPLE.replacen("do   = \"return\"\nnote", "do   = \"stop\"\nnote", 1);
        assert!(parse_err(&other_do).message.contains("return"));
    }

    #[test]
    fn to_must_be_nonempty_and_unique() {
        let empty = EXAMPLE.replace("to   = [\"agent_10\"]", "to   = []");
        assert!(parse_err(&empty).message.contains("空"));
        let dup = EXAMPLE.replace("to   = [\"agent_10\"]", "to   = [\"agent_10\", \"agent_10\"]");
        assert!(parse_err(&dup).message.contains("2 回"));
        let unsafe_id = EXAMPLE.replace("to   = [\"agent_10\"]", "to   = [\"../x\"]");
        assert!(parse_err(&unsafe_id).message.contains("使えない文字"));
    }

    #[test]
    fn note_counts_code_points_not_bytes() {
        let ok = "あ".repeat(MAX_NOTE_CHARS);
        let text = EXAMPLE.replace("取り消せない操作を含むので振り分けませんでした", &ok);
        assert!(JudgeFile::parse(&text).is_ok(), "200 字（600 バイト）は通る");
        let over = "あ".repeat(MAX_NOTE_CHARS + 1);
        let text = EXAMPLE.replace("取り消せない操作を含むので振り分けませんでした", &over);
        assert!(parse_err(&text).message.contains("200"));
    }

    #[test]
    fn missing_targets_names_ids_that_are_not_servants() {
        let file = JudgeFile::parse(EXAMPLE).unwrap();
        let missing = file.missing_targets(|id| id.as_str() == "agent_3");
        assert_eq!(missing, [AgentId::new("agent_10")]);
    }

    #[test]
    fn a_toml_syntax_error_is_reported_as_toml() {
        assert_eq!(parse_err("[questions.kind\n").location, "TOML");
    }

    // ---- 式の拒否（型と定義域） ----

    #[test]
    fn type_mismatches_are_rejected_at_parse_time() {
        assert!(when_err("kind >= 3").contains("== / != / in"));
        assert!(when_err("kind == 3").contains("数は書けません"));
        assert!(when_err("risky == research").contains("確率"));
        assert!(when_err("risky.margin > 0.1").contains("書けません"));
        assert!(when_err("risky in [0.1]").contains("in"));
        assert!(when_err("kind.mean > 1").contains("書けません"));
        assert!(when_err("kind.p in [0.5]").contains("in"), "in の左辺は q だけ");
        assert!(when_err("size.mean in [2]").contains("in"));
    }

    #[test]
    fn out_of_range_values_are_rejected_at_parse_time() {
        assert!(when_err("size >= 4").contains("1〜3"));
        assert!(when_err("size == 0").contains("1〜3"));
        assert!(when_err("size == 2.5").contains("整数"));
        assert!(when_err("risky >= 1.5").contains("0〜1"));
        assert!(when_err("kind.margin > 2").contains("0〜1"));
        assert!(when_err("size.mean > 3.5").contains("1〜3"));
        assert!(when_err("kind == missing").contains("選択肢「missing」"));
        assert!(when_err("nope > 0.1").contains("questions にありません"));
    }

    #[test]
    fn syntax_errors_are_rejected() {
        assert!(when_err("").contains("空"));
        assert!(when_err("kind == research size >= 2").contains("余計"));
        assert!(when_err("(kind == research").contains(")"));
        assert!(when_err("kind in []").contains("右辺"));
        assert!(when_err("kind in [research").contains("閉じていません"));
        assert!(when_err("risky >= .7").contains("右辺"), "先頭の . は書けない");
        assert!(when_err("risky >= -0.1").contains("「-」"), "負の数は書けない");
        assert!(when_err("risky = 0.5").contains("「=」"));
        assert!(when_err("kind.foo == research").contains(".p / .margin / .mean"));
        assert!(when_err("size + 1 > 2").contains("「+」"), "四則演算は持たない");
    }

    // ---- 有効の述語・表示 ----

    #[test]
    fn status_checks_disabled_then_file_then_parse_then_targets_then_model() {
        let ok = JudgeFile::parse(EXAMPLE);
        let bad: Result<JudgeFile, JudgeFileError> = Err(JudgeFileError::new("rules[1].when", "x"));
        let all = |_: &AgentId| true;
        // 利用者の無効は、ほかのどの理由より先に出る（ファイルが無くても・壊れていても）。
        assert_eq!(status_of(false, Some(&ok), all, true), JudgeStatus::Disabled);
        assert_eq!(status_of(false, None, all, true), JudgeStatus::Disabled);
        assert_eq!(status_of(false, Some(&bad), all, false), JudgeStatus::Disabled);
        assert_eq!(status_of(true, None, all, true), JudgeStatus::NoFile);
        assert!(matches!(status_of(true, Some(&bad), all, true), JudgeStatus::Invalid { .. }));
        assert_eq!(
            status_of(true, Some(&ok), |id: &AgentId| id.as_str() == "agent_3", true),
            JudgeStatus::MissingTargets { targets: vec![AgentId::new("agent_10")] }
        );
        assert_eq!(status_of(true, Some(&ok), all, false), JudgeStatus::NoJudgeModel);
        assert_eq!(status_of(true, Some(&ok), all, true), JudgeStatus::Active);
    }

    #[test]
    fn the_starter_templates_parse_in_both_languages() {
        for language in [crate::world::Language::Ja, crate::world::Language::En] {
            let file = JudgeFile::parse(starter_template(language)).expect("雛形は検査に通る");
            assert!(file.targets().is_empty(), "雛形は to を書かない");
        }
    }

    #[test]
    fn the_judgment_line_and_the_log_line_carry_values_but_no_question_text() {
        let file = JudgeFile::parse(EXAMPLE).unwrap();
        let a = answers(("research", &[("research", 0.82), ("implement", 0.18), ("other", 0.0)]), (1.2, &[0.1, 0.6, 0.3]), 0.12);
        let d = file.evaluate(&a).unwrap();
        let line = judgment_line("振り分け役", &d.values, crate::world::Language::Ja);
        assert_eq!(line, "［判断: 振り分け役 → kind=research(0.82) risky=0.12 size=2(0.60)］");
        let log = answers_line(&d.values);
        assert_eq!(log, "kind:research/0.82/0.64,risky:0.12,size:2/0.60/0.30");
        assert!(!log.contains("依頼") && !line.contains("依頼"), "問いの文面は出さない");
    }
}
