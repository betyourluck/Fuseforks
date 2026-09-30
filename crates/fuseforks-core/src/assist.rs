//! AI による下書き補助（Spec 63）— **純機構**。I/O も時計も持たない。
//!
//! SKILL.md / Construct.md / `judge.toml` の下書きを、利用者が選んだテンプレート（生成役）との
//! ヒアリングで作る。ここに置くのは:
//!
//! - 指針（生成役のシステムプロンプト）— 種類 × 言語
//! - 文脈の組み立て（「村の事実」と「利用者が書いたもの」の 2 区画。D7）
//! - ツール `submit_draft` の定義と、応答の振り分け（質問 / 下書き / 空。D5）
//! - フロントから戻ってきた履歴の**形だけ**の検査（D3）
//! - コアが足す定型文（ツール結果・「下書きを出して」の発話）
//!
//! 呼び出し・検証の輪・記録は `orchestrator/assist.rs`。
//!
//! **この村の SKILL.md は毎ターン全文がプロンプトに入る**（`compose_system_prompt`）。
//! 発火条件で読み込まれる一般のスキルとは前提が違うので、skill-creator の手順は借りても
//! 書き方の指南は写さない（Spec 63 の「前提の実測」）。

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::llm::{ChatMessage, ChatResponse, Provider, Role, ToolSpec};
use crate::model::{AgentId, ModelTemplateId};
use crate::world::Language;

/// 生成役に提示する唯一のツール。
pub const SUBMIT_DRAFT: &str = "submit_draft";

/// 判断役の検証の輪の上限（D8。コードの定数）。
pub const MAX_JUDGE_ATTEMPTS: u32 = 3;

/// 何の下書きか。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistKind {
    /// `agents/<id>/SKILL.md`。
    Skill,
    /// `agents/<id>/Construct.md`。
    Construct,
    /// `judges/<id>/judge.toml`。
    Judge,
}

impl AssistKind {
    /// ログの `kind=` に出す名前。
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Skill => "skill",
            Self::Construct => "construct",
            Self::Judge => "judge",
        }
    }
}

/// 下書きの対象（D4）。**欄名は種類に依らず `id`**。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistTarget {
    /// 種類。
    pub kind: AssistKind,
    /// サーヴァントか判断役の ID。
    pub id: AgentId,
}

/// IPC `assist_draft` の引数（D4）。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistRequest {
    /// 対象。
    pub target: AssistTarget,
    /// 生成役のテンプレート。
    pub template_id: ModelTemplateId,
    /// これまでの会話（利用者の発話 + 前回までの `appended`）。**コアが作ったメッセージを
    /// 逐語で**送り返してもらう（D3）。コアは形だけを [`check_history`] で検査する。
    #[serde(default)]
    pub history: Vec<ChatMessage>,
    /// 今回の利用者の発話。`force_draft` のときは `None` でよい。
    #[serde(default)]
    pub input: Option<String>,
    /// 「下書きを出して」ボタン（D5）。
    #[serde(default)]
    pub force_draft: bool,
    /// 編集中の本文（未保存の変更を含む）。**フロントから受け取る唯一の本文**（D7）。
    #[serde(default)]
    pub current: String,
}

/// 判断役の下書きの検査結果（D4 / D8）。**判別共用体 1 つ** — `valid` と理由を別々の欄に
/// すると「無効なのに理由が無い」形が表せてしまう。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssistValidation {
    /// 保存と同じ検査に通った。
    Valid,
    /// 落ちた。
    Invalid {
        /// どこで落ちたか。
        location: String,
        /// 何が悪いか。
        message: String,
    },
}

impl Serialize for AssistValidation {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Valid => json!({ "valid": true }),
            Self::Invalid { location, message } => {
                json!({ "valid": false, "location": location, "message": message })
            }
        }
        .serialize(serializer)
    }
}

/// IPC `assist_draft` の返り値（D4）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AssistReply {
    /// 生成役が質問を返した。
    #[serde(rename_all = "camelCase")]
    Question {
        /// 今回の利用者の発話 + コアが作ったメッセージ。フロントは解釈せず `history` の末尾へ足す。
        appended: Vec<ChatMessage>,
        /// 質問の本文。
        text: String,
    },
    /// 生成役が下書きを出した。
    #[serde(rename_all = "camelCase")]
    Draft {
        /// 同上。
        appended: Vec<ChatMessage>,
        /// 生成役が下書きに添えた本文。
        text: Option<String>,
        /// 下書き。
        content: String,
        /// `submit_draft` の `notes`（仮定・未確認点）。
        notes: Option<String>,
        /// `content` のコードポイント数。
        draft_chars: u32,
        /// 判断役だけ。SKILL / Construct は `None`（検査しない）。
        validation: Option<AssistValidation>,
        /// 検証の輪の回数（SKILL / Construct は 1）。
        attempts: u32,
    },
}

/// LLM 呼び出し 1 回の結末（`assist:` 行と `Record::Assist` の `outcome`。D10）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistOutcome {
    /// 質問を返した。
    Question,
    /// 下書きを出した（検査なし、または通った）。
    Draft,
    /// 判断役の下書きが検査に落ち、作り直させる。
    Invalid,
    /// 上限に達した、または輪の途中で失敗して、不正な下書きを返した。
    DraftInvalid,
    /// 本文も下書きも無かった。
    Empty,
    /// 呼び出しが失敗した。
    Failed,
}

impl AssistOutcome {
    /// ログに出す名前。
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Question => "question",
            Self::Draft => "draft",
            Self::Invalid => "invalid",
            Self::DraftInvalid => "draft_invalid",
            Self::Empty => "empty",
            Self::Failed => "failed",
        }
    }
}

/// 生成役に渡す「村の事実」（D7。コアが対象の id から集める）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssistContext {
    /// SKILL / Construct。
    Servant {
        /// 個体の表示名。
        name: String,
        /// 有効な同梱ツールの名前。
        bundled_tools: Vec<String>,
        /// MCP サーバー。接続中はツール名まで（`Some`）、未接続は名前だけ（`None`）。
        mcp: Vec<(String, Option<Vec<String>>)>,
        /// 接続先の表示名。
        connected: Vec<String>,
        /// 対になるファイルの保存済み本文（SKILL なら Construct、逆も）。空なら `None`。
        pair: Option<String>,
    },
    /// 判断役。
    Judge {
        /// 判断役の名前。
        name: String,
        /// 接続先に選べるサーヴァント（ID・表示名・役職名）。
        servants: Vec<(AgentId, String, Option<String>)>,
    },
}

/// 質問か下書きか（D5。**型で分ける** — 本文の目印では分けない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Classified {
    /// 質問（呼び出しが無く、本文がある）。
    Question(String),
    /// 下書き（`submit_draft` の呼び出しがある）。
    Draft {
        /// 添えた本文（あれば）。
        text: Option<String>,
        /// 下書き。
        content: String,
        /// 仮定・未確認点。
        notes: Option<String>,
    },
    /// どちらも無い。
    Empty,
}

/// 応答を振り分ける。
///
/// - `submit_draft` の呼び出しがあり `content` が空でなければ**下書き**（本文があれば添える）
/// - 無く、本文があれば**質問**
/// - どちらも無ければ `Empty`
#[must_use]
pub fn classify(response: &ChatResponse) -> Classified {
    let text = response
        .text
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    // **最初の `submit_draft` だけを見る**（2 本目以降は使わない — ツール結果の対を付ける側と
    // 同じ規則。空の 1 本目の後ろの 2 本目を拾うと、どの呼び出しに結果を返したかがずれる）。
    let draft = response.tool_calls.iter().find(|call| call.name == SUBMIT_DRAFT).and_then(|call| {
        let content = call.args.get("content")?.as_str()?;
        if content.trim().is_empty() {
            return None;
        }
        let notes = call
            .args
            .get("notes")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        Some((content.to_owned(), notes))
    });
    match (draft, text) {
        (Some((content, notes)), text) => Classified::Draft { text, content, notes },
        (None, Some(text)) => Classified::Question(text),
        (None, None) => Classified::Empty,
    }
}

/// 強制（`Specific("submit_draft")`）を送れるワイヤか（D5）。**ワイヤで決める**
/// （モデル名では決めない）。
///
/// P0 実測: Anthropic は `tool_choice: type "tool" and "any" are not supported for this model.`
/// の 400 / Meta のアダプタは `tool_choice` を送らない（Spec 37: `auto` のみ受理）。
#[must_use]
pub fn force_supported(provider: Provider) -> bool {
    !matches!(provider, Provider::Anthropic | Provider::MetaResponses)
}

/// フロントから戻ってきた履歴の**形だけ**を検査する（D3）。
///
/// - 役割は `user` / `assistant` / `tool` だけ（`system` はコアが組む）
/// - 呼び出しは `submit_draft` だけ（それしか提示していない）
/// - ツール結果は直前の呼び出しと対（対が無いとプロバイダが拒否する）
/// - 次の `user` / `assistant` の前に、呼び出しはすべて答えられている
/// - 添付は持たない
///
/// # Errors
/// 崩れている場所と理由（画面に出す文ではなく、`CoreError` の理由）。
pub fn check_history(history: &[ChatMessage]) -> Result<(), String> {
    let mut pending: Vec<String> = Vec::new();
    for (i, message) in history.iter().enumerate() {
        if !message.attachments.is_empty() {
            return Err(format!("history[{i}]: 添付は送れません"));
        }
        match message.role {
            Role::System => return Err(format!("history[{i}]: system は送れません")),
            Role::User => {
                if !pending.is_empty() {
                    return Err(format!("history[{i}]: 答えられていない呼び出しの後に user が来ました"));
                }
                if !message.tool_calls.is_empty() || message.tool_call_id.is_some() {
                    return Err(format!("history[{i}]: user が呼び出しを持っています"));
                }
            }
            Role::Assistant => {
                if !pending.is_empty() {
                    return Err(format!("history[{i}]: 答えられていない呼び出しの後に assistant が来ました"));
                }
                if message.tool_call_id.is_some() {
                    return Err(format!("history[{i}]: assistant がツール結果の欄を持っています"));
                }
                for call in &message.tool_calls {
                    if call.name != SUBMIT_DRAFT {
                        return Err(format!("history[{i}]: 提示していないツール `{}` の呼び出しです", call.name));
                    }
                    pending.push(call.id.clone());
                }
            }
            Role::Tool => {
                let Some(id) = message.tool_call_id.as_deref() else {
                    return Err(format!("history[{i}]: ツール結果に呼び出しの ID がありません"));
                };
                let Some(at) = pending.iter().position(|p| p == id) else {
                    return Err(format!("history[{i}]: 対になる呼び出しの無いツール結果です"));
                };
                pending.remove(at);
            }
        }
    }
    if pending.is_empty() {
        Ok(())
    } else {
        Err("最後の呼び出しが答えられていません".to_owned())
    }
}

/// ツール `submit_draft` の定義。
#[must_use]
pub fn submit_draft_spec(language: Language) -> ToolSpec {
    let (description, content, notes) = match language {
        Language::Ja => (
            "下書きを利用者へ見せる。下書きは必ずこのツールで出し、本文には書かない。",
            "下書きの全文（ファイルにそのまま入る文字列）",
            "仮定したこと・利用者に確かめたいこと（下書きには入らない）",
        ),
        Language::En => (
            "Show a draft to the user. Always deliver drafts through this tool, never in your message text.",
            "The full draft (the exact text that goes into the file)",
            "Assumptions you made and points to confirm with the user (not part of the draft)",
        ),
    };
    ToolSpec {
        name: SUBMIT_DRAFT.to_owned(),
        description: description.to_owned(),
        parameters: json!({
            "type": "object",
            "properties": {
                "content": { "type": "string", "description": content },
                "notes": { "type": "string", "description": notes }
            },
            "required": ["content"]
        }),
    }
}

/// 「下書きを出して」ボタンでコアが足す利用者の発話（D5。村の言語）。
#[must_use]
pub fn force_utterance(language: Language) -> &'static str {
    match language {
        Language::Ja => "ここまでの情報で下書きを出してください。分からない点は仮定して、notes に書いてください。",
        Language::En => {
            "Please produce a draft with the information so far. Make assumptions where unsure and list them in notes."
        }
    }
}

/// 検査に通った（または検査しない）下書きへのツール結果（D3）。
#[must_use]
pub fn draft_shown(language: Language) -> &'static str {
    match language {
        Language::Ja => "下書きを利用者に見せました。以後の依頼に従って直してください。",
        Language::En => "The draft was shown to the user. Revise it according to their next request.",
    }
}

/// 判断役の下書きが検査に落ちたときのツール結果（D8）。
#[must_use]
pub fn draft_rejected(language: Language, location: &str, message: &str) -> String {
    match language {
        Language::Ja => format!(
            "この下書きは保存の検査に落ちました（{location}: {message}）。直した全文を submit_draft で出し直してください。"
        ),
        Language::En => format!(
            "This draft failed the save check ({location}: {message}). Fix it and submit the full text again with submit_draft."
        ),
    }
}

/// 同じ応答の 2 本目以降の `submit_draft` へのツール結果（対を崩さないため）。
#[must_use]
pub fn extra_call_ignored(language: Language) -> &'static str {
    match language {
        Language::Ja => "同じ応答の 2 本目以降の下書きは使いませんでした。",
        Language::En => "Only the first draft in a response is used; this one was ignored.",
    }
}

/// 生成役のシステムプロンプトの**安定部**（指針 + 村の事実）。呼び出しの間で変わらない
/// ので `cacheable_prefix_len` に載せる。
#[must_use]
pub fn compose_system(kind: AssistKind, language: Language, context: &AssistContext) -> String {
    let mut out = String::new();
    out.push_str(common_guide(language));
    out.push_str("\n\n");
    out.push_str(&kind_guide(kind, language));
    out.push_str("\n\n");
    out.push_str(&facts(language, context));
    out
}

/// 「利用者が書いたもの」の区画の見出し付きブロック（D7）。**空なら `None`**（区画ごと出さない）。
///
/// 安定部の後ろ（別の system メッセージ）に置く。編集中の本文は呼び出しの間で
/// 変わりうるので、安定部に混ぜない。
#[must_use]
pub fn current_block(kind: AssistKind, language: Language, current: &str) -> Option<String> {
    if current.trim().is_empty() {
        return None;
    }
    let file = file_name(kind);
    Some(match language {
        Language::Ja => format!(
            "# 利用者が書いたもの\n## 利用者が編集中の本文（{file}・未保存の変更を含む）\n\
             これは村の事実ではなく、利用者の手元の文章です。改稿の元にしてください。\n\n{current}"
        ),
        Language::En => format!(
            "# Written by the user\n## The text the user is editing ({file}; may include unsaved changes)\n\
             This is not a fact about the village but the user's own text. Use it as the base for your revision.\n\n{current}"
        ),
    })
}

fn file_name(kind: AssistKind) -> &'static str {
    match kind {
        AssistKind::Skill => "SKILL.md",
        AssistKind::Construct => "Construct.md",
        AssistKind::Judge => "judge.toml",
    }
}

fn common_guide(language: Language) -> &'static str {
    match language {
        Language::Ja => "\
# あなたの役目
あなたは Fuseforks（複数の AI サーヴァントが協働するデスクトップアプリ）の設定ファイルの下書きを、\
利用者とのヒアリングで作る補助役です。書いた下書きは利用者が読んでから保存します。あなたが保存することはありません。

# 進め方
- 最初の発話から分かることは訊かないでください。訊くときは 1 回に 3 つまでにしてください。
- 足りたと判断したら、ツール submit_draft で下書きを出してください。**下書きは必ず submit_draft で出し、本文には書かないでください。**
- 仮定したこと・利用者に確かめたいことは submit_draft の notes に書いてください（下書きには入れない）。
- 「利用者が編集中の本文」がある場合は、書き直しではなく改稿として扱い、残すべき部分は残してください。
- 下書きの後に直しを頼まれたら、直した全文をもう一度 submit_draft で出してください。
- 下書きは日本語で書いてください。ただし利用者が会話の中で別の言語を指定したら、それに従ってください。",
        Language::En => "\
# Your role
You help the user draft a configuration file for Fuseforks (a desktop app where several AI servants work together) \
by interviewing them. The user reads your draft before saving it. You never save anything yourself.

# How to proceed
- Do not ask about things the first message already tells you. Ask at most 3 questions at a time.
- Once you have enough, deliver the draft with the submit_draft tool. **Always deliver drafts through submit_draft, never in your message text.**
- Put assumptions and points to confirm in the notes of submit_draft (not in the draft).
- If there is \"the text the user is editing\", treat the task as a revision, not a rewrite, and keep what should stay.
- When asked for changes after a draft, submit the full revised text again with submit_draft.
- Write the draft in English, unless the user asks for another language in the conversation.",
    }
}

fn kind_guide(kind: AssistKind, language: Language) -> String {
    match (kind, language) {
        (AssistKind::Skill, Language::Ja) => "\
# 作るもの: SKILL.md（サーヴァントの手順）
- **この村の SKILL.md は、毎ターン全文がサーヴァントのシステムプロンプトに入ります。** 必要なときだけ読み込まれる仕組みではありません。
  - 「いつ使うか」の説明・description・前付け（frontmatter）は書かないでください。
  - 長さはそのまま毎ターンの費用になります。必要なことだけを短く書いてください。
- 手順は命令形で書いてください。
- **名前が渡されたツールだけを名指しし、持っていないツールを前提にしないでください。** MCP サーバーの名前だけが渡されている（未接続の）ときは、関数名を推測せず、そのサーバーの用途として書いてください。
- 人格や口調は書かないでください（Construct.md の役割）。「対になるファイル」と重複させないでください。".to_owned(),
        (AssistKind::Skill, Language::En) => "\
# What to write: SKILL.md (the servant's procedures)
- **In this village, the whole SKILL.md goes into the servant's system prompt on every turn.** It is not loaded on demand.
  - Do not write \"when to use\" sections, a description, or frontmatter.
  - Its length is paid on every turn. Keep it to what is needed.
- Write procedures in the imperative.
- **Name only the tools you were given, and never assume tools the servant does not have.** When only an MCP server name is given (not connected), describe what the server is for instead of guessing function names.
- Do not write personality or tone (that belongs to Construct.md). Do not duplicate \"the paired file\".".to_owned(),
        (AssistKind::Construct, Language::Ja) => "\
# 作るもの: Construct.md（サーヴァントの人格と役割）
- 人格・口調・役割の範囲・してはいけないことを書いてください。
- **役職のラベル（「あなたは調査役です」など）に寄せず、振る舞いで書いてください。** ラベルの含意に人格が引きずられます。
- 「他のエージェントの発言を代筆しない」はアプリが既に伝えているので繰り返さないでください。
- 手順は書かないでください（SKILL.md の役割）。「対になるファイル」と重複させないでください。
- 毎ターン全文がシステムプロンプトに入るので、短く書いてください。".to_owned(),
        (AssistKind::Construct, Language::En) => "\
# What to write: Construct.md (the servant's personality and role)
- Write personality, tone, the scope of the role, and what the servant must not do.
- **Describe behavior instead of a role label (\"You are the researcher\").** A label drags the personality toward its connotations.
- The app already tells servants not to write other agents' lines; do not repeat it.
- Do not write procedures (that belongs to SKILL.md). Do not duplicate \"the paired file\".
- The whole file goes into the system prompt on every turn, so keep it short.".to_owned(),
        (AssistKind::Judge, language) => judge_guide(language),
    }
}

fn judge_guide(language: Language) -> String {
    let starter = crate::judge::starter_template(language);
    match language {
        Language::Ja => format!("\
# 作るもの: judge.toml（判断役の問いと規則）
判断役は、判断専用モデルに問いを投げ、その型付きの答えを人が書いた規則に当てて、依頼を渡す相手を決める部品です。\
**文法の正はアプリのパーサーです。** 検査に落ちた下書きは理由つきで返るので、直して出し直してください。

## ファイルの形
- `[questions.<名前>]` — 名前は `[a-z][a-z0-9_]*`。`and` / `or` / `not` / `in` は使えない。
  - `type = \"choice\"`: `ask`（問い）と `options = {{ 鍵 = 説明文 }}`（2 択以上。鍵は名前と同じ規則）
  - `type = \"score\"`: `ask` と `levels = [説明文, …]`（低い順。2〜10 段階）
  - `type = \"noul\"`: `ask` と `true_if` / `false_if`（真と偽それぞれ一文）
- `[[rules]]` — `when`（条件式）と、`to = [\"サーヴァントの ID\", …]`（1 体以上・重複なし）か `do = \"return\"` のどちらか 1 つ。任意で `note`（200 字まで）
- `[otherwise]` — どの規則にも当たらなかったとき。`to` か `do = \"return\"`（無いと保存できない）
- 規則は上から評価し、最初に当たった 1 つだけを実行する。

## 条件式
- 比較: `==` `!=` `>=` `<=` `>` `<`、`in [..]`、`and` / `or` / `not`、括弧。四則演算・関数は無い。
- choice: `q`（選ばれた鍵。`==` / `!=` / `in` のみ）/ `q.p`（その確率）/ `q.margin`（1 位と 2 位の確率の差）
- score: `q`（最も確率の高い段階。1 始まりの整数）/ `q.p` / `q.margin` / `q.mean`（期待値。1〜段階数の小数）
- noul: `q`（真である確率 0〜1。比較のみ）
- 数は 0 以上。確率は 0〜1。score の段階の外の番号・choice に無い鍵は書けない。

## 良い問いと規則
- **1 問 1 論点**にしてください。2 つの論点を 1 つの問いに入れると、2 つ目が沈みます。
- choice には「その他」を置き、`otherwise` より前に `kind == other` のような規則を置いてください。
- `to` には下の「接続先に選べるサーヴァント」の ID だけを書いてください。

## 雛形（形の例）
```toml
{starter}```"),
        Language::En => format!("\
# What to write: judge.toml (a judge's questions and rules)
A judge asks a judgment-only model some questions and applies the user's rules to its typed answers to pick who receives the request. \
**The app's parser is the source of truth for the grammar.** A draft that fails the check comes back with the reason; fix it and submit again.

## File shape
- `[questions.<name>]` — the name is `[a-z][a-z0-9_]*`; `and` / `or` / `not` / `in` are reserved.
  - `type = \"choice\"`: `ask` (the question) and `options = {{ key = description }}` (2 or more; keys follow the name rule)
  - `type = \"score\"`: `ask` and `levels = [description, ...]` (lowest first; 2 to 10 levels)
  - `type = \"noul\"`: `ask` and `true_if` / `false_if` (one sentence each)
- `[[rules]]` — `when` (a condition) and exactly one of `to = [\"servant id\", ...]` (1 or more, no duplicates) or `do = \"return\"`. Optional `note` (up to 200 characters)
- `[otherwise]` — when no rule matches: `to` or `do = \"return\"` (required)
- Rules are evaluated top-down; only the first match runs.

## Conditions
- Comparisons `==` `!=` `>=` `<=` `>` `<`, `in [..]`, `and` / `or` / `not`, parentheses. No arithmetic or functions.
- choice: `q` (the chosen key; `==` / `!=` / `in` only) / `q.p` (its probability) / `q.margin` (gap between 1st and 2nd)
- score: `q` (the most probable level; 1-based integer) / `q.p` / `q.margin` / `q.mean` (expected level, 1 to the number of levels)
- noul: `q` (probability of true, 0 to 1; comparisons only)
- Numbers are 0 or more; probabilities are 0 to 1. Levels outside the score and keys not in the choice are rejected.

## Good questions and rules
- **One point per question.** Two points in one question make the second one sink.
- Give a choice an \"other\" option and put a rule like `kind == other` before `otherwise`.
- Use only the ids from \"servants you can route to\" below in `to`.

## Starter (an example of the shape)
```toml
{starter}```"),
    }
}

fn facts(language: Language, context: &AssistContext) -> String {
    let none = match language {
        Language::Ja => "（なし）",
        Language::En => "(none)",
    };
    let list = |items: &[String]| {
        if items.is_empty() {
            none.to_owned()
        } else {
            items.join(", ")
        }
    };
    match (context, language) {
        (
            AssistContext::Servant { name, bundled_tools, mcp, connected, pair },
            _,
        ) => {
            let mcp_lines: Vec<String> = mcp
                .iter()
                .map(|(server, tools)| match (tools, language) {
                    (Some(tools), _) => format!("- {server}: {}", list(tools)),
                    (None, Language::Ja) => format!("- {server}（未接続のためツール名は不明）"),
                    (None, Language::En) => format!("- {server} (not connected; tool names unknown)"),
                })
                .collect();
            let mcp_text = if mcp_lines.is_empty() {
                none.to_owned()
            } else {
                mcp_lines.join("\n")
            };
            let pair_text = pair.as_deref().unwrap_or(none);
            match language {
                Language::Ja => format!(
                    "# 村の事実（アプリが集めたもの）\n\
                     - 対象のサーヴァント: {name}\n\
                     - 使える同梱ツール: {}\n\
                     - 接続している相手（頼める相手）: {}\n\
                     ## MCP のツール\n{mcp_text}\n\
                     ## 対になるファイル（保存済み。重複させない）\n{pair_text}",
                    list(bundled_tools),
                    list(connected),
                ),
                Language::En => format!(
                    "# Facts about the village (collected by the app)\n\
                     - Target servant: {name}\n\
                     - Bundled tools it can use: {}\n\
                     - Servants it is connected to (can ask): {}\n\
                     ## MCP tools\n{mcp_text}\n\
                     ## The paired file (saved; do not duplicate)\n{pair_text}",
                    list(bundled_tools),
                    list(connected),
                ),
            }
        }
        (AssistContext::Judge { name, servants }, _) => {
            let rows: Vec<String> = servants
                .iter()
                .map(|(id, display, role)| match role {
                    Some(role) => format!("- {id}: {display}（{role}）"),
                    None => format!("- {id}: {display}"),
                })
                .collect();
            let rows = if rows.is_empty() { none.to_owned() } else { rows.join("\n") };
            match language {
                Language::Ja => format!(
                    "# 村の事実（アプリが集めたもの）\n- 判断役の名前: {name}\n## 接続先に選べるサーヴァント（ID: 表示名（役職））\n{rows}"
                ),
                Language::En => format!(
                    "# Facts about the village (collected by the app)\n- Judge name: {name}\n## Servants you can route to (id: display name (role))\n{rows}"
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{Finish, ToolCall, Usage};

    fn response(text: Option<&str>, calls: Vec<ToolCall>) -> ChatResponse {
        ChatResponse {
            text: text.map(str::to_owned),
            tool_calls: calls,
            finish: Finish::Stop,
            usage: Usage::default(),
            grounding: Default::default(),
            reasoning_summary: Vec::new(),
        }
    }

    fn submit(id: &str, content: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: SUBMIT_DRAFT.into(),
            args: json!({ "content": content, "notes": "仮定 A" }),
            extra: None,
        }
    }

    #[test]
    fn a_call_is_a_draft_and_text_alone_is_a_question() {
        assert_eq!(
            classify(&response(None, vec![submit("c1", "# 手順")])),
            Classified::Draft { text: None, content: "# 手順".into(), notes: Some("仮定 A".into()) }
        );
        assert_eq!(classify(&response(Some(" 何を？ "), vec![])), Classified::Question("何を？".into()));
        assert_eq!(
            classify(&response(Some("どうぞ"), vec![submit("c1", "本文")])),
            Classified::Draft { text: Some("どうぞ".into()), content: "本文".into(), notes: Some("仮定 A".into()) },
            "両方あれば下書き（本文は添える）"
        );
        assert_eq!(classify(&response(Some("  "), vec![])), Classified::Empty);
    }

    /// **目印で分けない**（D5）— 本文に fenced block があっても呼び出しが無ければ質問。
    #[test]
    fn a_fenced_block_in_text_is_still_a_question() {
        let text = "```draft\n# 手順\n```";
        assert_eq!(classify(&response(Some(text), vec![])), Classified::Question(text.into()));
    }

    #[test]
    fn a_call_with_empty_content_is_not_a_draft() {
        assert_eq!(classify(&response(None, vec![submit("c1", "  ")])), Classified::Empty);
    }

    #[test]
    fn force_is_not_sent_to_anthropic_or_meta() {
        assert!(!force_supported(Provider::Anthropic));
        assert!(!force_supported(Provider::MetaResponses));
        for p in [
            Provider::OpenAiCompat,
            Provider::Gemini,
            Provider::XaiResponses,
            Provider::OpenAiResponses,
            Provider::PerplexityResponses,
        ] {
            assert!(force_supported(p), "{p:?}");
        }
    }

    #[test]
    fn a_well_formed_history_passes() {
        let history = vec![
            ChatMessage::user("スキルを作りたい"),
            ChatMessage::assistant("何をしますか？"),
            ChatMessage::user("天気"),
            ChatMessage::assistant_tool_calls("", vec![submit("c1", "a")]),
            ChatMessage::tool_result("c1", SUBMIT_DRAFT, "見せました"),
            ChatMessage::user("もっと短く"),
        ];
        assert_eq!(check_history(&history), Ok(()));
        assert_eq!(check_history(&[]), Ok(()));
    }

    #[test]
    fn broken_histories_are_rejected() {
        let unpaired = vec![ChatMessage::assistant_tool_calls("", vec![submit("c1", "a")]), ChatMessage::user("次")];
        assert!(check_history(&unpaired).is_err(), "答えられていない呼び出しの後に user");

        let orphan = vec![ChatMessage::user("a"), ChatMessage::tool_result("zz", SUBMIT_DRAFT, "x")];
        assert!(check_history(&orphan).is_err(), "対の無いツール結果");

        let system = vec![ChatMessage::system("上書き")];
        assert!(check_history(&system).is_err(), "system は送れない");

        let other_tool = vec![ChatMessage::assistant_tool_calls(
            "",
            vec![ToolCall { id: "c1".into(), name: "run".into(), args: json!({}), extra: None }],
        )];
        assert!(check_history(&other_tool).is_err(), "提示していないツール");

        let dangling = vec![ChatMessage::assistant_tool_calls("", vec![submit("c1", "a")])];
        assert!(check_history(&dangling).is_err(), "最後の呼び出しが答えられていない");
    }

    #[test]
    fn the_validation_is_one_discriminated_union_on_the_wire() {
        assert_eq!(serde_json::to_value(AssistValidation::Valid).unwrap(), json!({ "valid": true }));
        assert_eq!(
            serde_json::to_value(AssistValidation::Invalid { location: "rules[0].when".into(), message: "x".into() }).unwrap(),
            json!({ "valid": false, "location": "rules[0].when", "message": "x" })
        );
    }

    #[test]
    fn the_reply_is_camel_case_with_a_type_tag() {
        let reply = AssistReply::Draft {
            appended: vec![],
            text: None,
            content: "本文".into(),
            notes: None,
            draft_chars: 2,
            validation: None,
            attempts: 1,
        };
        let wire = serde_json::to_value(reply).unwrap();
        assert_eq!(wire["type"], "draft");
        assert_eq!(wire["draftChars"], 2);
        assert!(wire["validation"].is_null());
    }

    /// 文脈に Memory / 条例 / 会話ログの席が無い（D7）ことは型で閉じている。ここでは
    /// **未接続の MCP にツール名を書かない**ことと、区画の見出しを留める。
    #[test]
    fn facts_name_unconnected_mcp_without_tool_names() {
        let context = AssistContext::Servant {
            name: "ザリ".into(),
            bundled_tools: vec!["file".into(), "grep".into()],
            mcp: vec![("fetch".into(), Some(vec!["fetch__get".into()])), ("github".into(), None)],
            connected: vec!["ジェミー".into()],
            pair: None,
        };
        let system = compose_system(AssistKind::Skill, Language::Ja, &context);
        assert!(system.contains("- fetch: fetch__get"));
        assert!(system.contains("- github（未接続のためツール名は不明）"));
        assert!(system.contains("# 村の事実"));
        assert!(!system.contains("# 利用者が書いたもの"), "利用者の区画は安定部に入れない");
    }

    #[test]
    fn the_current_block_is_the_user_section_and_absent_when_empty() {
        assert_eq!(current_block(AssistKind::Skill, Language::Ja, "  \n"), None);
        let block = current_block(AssistKind::Construct, Language::En, "be kind").unwrap();
        assert!(block.starts_with("# Written by the user"));
        assert!(block.contains("Construct.md"));
    }

    /// 判断役の指針は雛形を載せる。**雛形はパーサーを通る**（`judge.rs` のテストが留めている）
    /// ので、ここでは載っていることだけを見る。
    #[test]
    fn the_judge_guide_carries_the_starter() {
        let context = AssistContext::Judge { name: "振り分け役".into(), servants: vec![] };
        for language in [Language::Ja, Language::En] {
            let system = compose_system(AssistKind::Judge, language, &context);
            assert!(system.contains(crate::judge::starter_template(language)));
        }
    }
}
