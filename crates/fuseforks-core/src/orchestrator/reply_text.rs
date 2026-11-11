//! 依頼主が読む結果の文（2026-11-12）。**村の言語で書く**。
//!
//! `ask_*` / `plan` / 判断役のツール結果と、依頼主へ返す [`Reply`](super::Reply) の本文、それに
//! モデルが本文を返さなかったときにコアが代わりに書く返事をここへ集めた。それまでは使う場所に
//! 日本語の文が直に書かれており、英語の村（中文の村も英語で話す — Spec 74）でもモデルは日本語の
//! 結果を読んでいた。同じ穴は止まったターンの注記にもあり（`failures.md` #156）、そちらを直した
//! ときに**同じ網の外**として残していたもの。
//!
//! **日本語の文は 2026-11-12 までの文と 1 バイトも変えていない**（下のテストが逐語で留める）。
//! 英語は語順を英語で組む — 名前や数を埋め込む文は 1 本の `format!` を言語ごとに持つ
//! （訳した語句を同じ枠へ差すと語順が言語をまたげない — Spec 13 P3b）。
//!
//! 埋め込む `{err}` はコアのエラー文（Spec 13 で日本語に固定）なので、英語の文にもエラーの
//! 種類名は日本語で残る（[`super::turn::failed_turn_note`] と同じ扱い）。

use crate::world::Language;

/// 止まった依頼の返信（[`Reply`](super::Reply) の本文。止まり方ごとに 1 つ）。
#[derive(Debug, Clone, Copy)]
pub(super) enum Stopped {
    /// 利用者の指示で打ち切った（Spec 10 — 出口 2a と、未着手の畳み 2b）。
    Interrupted,
    /// トークン予算の上限（Spec 11）。
    BudgetExhausted,
    /// `run` の承認待ちで止めた（Spec 72）。
    ApprovalPending,
}

/// 止まった依頼の返信の本文。
pub(super) fn stopped(why: Stopped, language: Language) -> &'static str {
    match (why, language) {
        (Stopped::Interrupted, Language::Ja) => {
            "この依頼はユーザーの指示で打ち切られました。答えはありません。"
        }
        (Stopped::Interrupted, Language::En) => {
            "This request was stopped by the user. There is no answer."
        }
        (Stopped::BudgetExhausted, Language::Ja) => {
            "この依頼はトークン予算の上限で打ち切られました。答えはありません。"
        }
        (Stopped::BudgetExhausted, Language::En) => {
            "This request was stopped at the token budget limit. There is no answer."
        }
        (Stopped::ApprovalPending, Language::Ja) => {
            "この依頼は承認待ちのコマンドで止まりました。答えはありません。"
        }
        (Stopped::ApprovalPending, Language::En) => {
            "This request stopped at a command awaiting approval. There is no answer."
        }
    }
}

/// 委譲された相手が答えずに転送したときの返信（`failures.md` #35 の文）。`names` は転送先の表示名。
pub(super) fn handed_off(names: &[String], language: Language) -> String {
    match language {
        Language::Ja => format!(
            "相手はこの依頼に自分で答えず、{} へ会話を渡しました。\
             答えはこちらへ戻りません。必要なら別の相手に頼むか、自分で進めてください。",
            names.join("、")
        ),
        Language::En => format!(
            "The other agent did not answer this request itself and passed the conversation to {}. \
             The answer will not come back here. If needed, ask someone else or proceed yourself.",
            names.join(", ")
        ),
    }
}

/// 転送の上限（`max_hops`）で `ask_*` と判断役が配送しなかった結果。
pub(super) fn hop_limit_ask(language: Language) -> &'static str {
    language.pick(
        "転送の上限に達したため、これ以上は尋ねられません。",
        "The forwarding limit has been reached, so you cannot ask anyone further.",
    )
}

/// 転送の上限で `plan` が配送しなかった結果。
pub(super) fn hop_limit_plan(language: Language) -> &'static str {
    language.pick(
        "転送の上限に達したため、これ以上は頼めません。何も配送していません。",
        "The forwarding limit has been reached, so you cannot ask anyone further. Nothing was delivered.",
    )
}

/// 予算が尽きていて配送を始めなかった結果（`deliver_and_wait`）。
pub(super) fn budget_not_delivered(language: Language) -> &'static str {
    language.pick(
        "トークン予算の上限に達したため、配送していません。",
        "The token budget limit has been reached, so nothing was delivered.",
    )
}

/// 相手が停止中・受信箱が飽和で配送できなかった結果。
pub(super) fn undeliverable(error: &dyn std::fmt::Display, language: Language) -> String {
    match language {
        Language::Ja => format!("相手に尋ねられませんでした: {error}"),
        Language::En => format!("Could not ask them: {error}"),
    }
}

/// 相手が答えずにタスクを終えた（停止・失敗）結果。
pub(super) fn no_answer(language: Language) -> &'static str {
    language.pick("相手から答えが返りませんでした。", "No answer came back from them.")
}

/// 待ちの上限（`ask_timeout`）を過ぎた結果。
pub(super) fn timed_out(language: Language) -> &'static str {
    language.pick(
        "相手からの答えが時間内に返りませんでした。",
        "Their answer did not come back in time.",
    )
}

/// 宛先がこの依頼の因果の中で答えを待っていた（輪 — Spec 44）。
pub(super) fn circular(to: &str, language: Language) -> String {
    match language {
        Language::Ja => format!(
            "宛先「{to}」はこの依頼の因果の中で答えを待っています（循環する委譲）。\
             配送していません。依頼主への返信で直接答えてください。"
        ),
        Language::En => format!(
            "Target \"{to}\" is waiting for an answer within this request's causality \
             (circular delegation). Nothing was delivered. Answer your requester directly instead."
        ),
    }
}

/// `plan` の静的な不正（何も配送せずに差し戻す）。
#[derive(Debug, Clone, Copy)]
pub(super) enum PlanDefect<'a> {
    /// `tasks` が無い。
    MissingTasks,
    /// `tasks` が空。
    EmptyTasks,
    /// `position` 件目に `to` か `message` が無い。
    MissingFields { position: usize },
    /// `position` 件目の宛先が接続先ではない。`roster` は頼める相手の名簿。
    NotConnected { position: usize, to: &'a str, roster: &'a [&'a str] },
    /// 同じ波に同じ宛先が 2 回ある。
    Duplicate { to: &'a str },
}

/// `plan` の静的な不正の結果。
pub(super) fn plan_defect(defect: PlanDefect<'_>, language: Language) -> String {
    match (defect, language) {
        (PlanDefect::MissingTasks, Language::Ja) => {
            "plan には tasks（依頼の配列）が必要です。何も配送していません。".to_owned()
        }
        (PlanDefect::MissingTasks, Language::En) => {
            "plan needs tasks (an array of requests). Nothing was delivered.".to_owned()
        }
        (PlanDefect::EmptyTasks, Language::Ja) => "plan の tasks が空です。誰にも頼まずに終わりました。\
             頼む相手が居ないなら、plan を呼ばずに自分で答えてください。"
            .to_owned(),
        (PlanDefect::EmptyTasks, Language::En) => "plan's tasks is empty, so no one was asked. \
             If there is no one to ask, answer yourself without calling plan."
            .to_owned(),
        (PlanDefect::MissingFields { position }, Language::Ja) => {
            format!("{position} 件目の依頼に to と message の両方が必要です。何も配送していません。")
        }
        (PlanDefect::MissingFields { position }, Language::En) => {
            format!("Request {position} needs both to and message. Nothing was delivered.")
        }
        (PlanDefect::NotConnected { position, to, roster }, Language::Ja) => format!(
            "{position} 件目の宛先「{to}」は、あなたの接続先ではありません。\
             頼めるのは {} です。何も配送していません。",
            roster.join("、")
        ),
        (PlanDefect::NotConnected { position, to, roster }, Language::En) => format!(
            "The target \"{to}\" of request {position} is not one of your connections. \
             You can ask {}. Nothing was delivered.",
            roster.join(", ")
        ),
        (PlanDefect::Duplicate { to }, Language::Ja) => format!(
            "宛先「{to}」が同じ波に 2 回あります。1 回の plan で同じ相手へ頼めるのは 1 件です。\
             2 件目は次の波で頼んでください。何も配送していません。"
        ),
        (PlanDefect::Duplicate { to }, Language::En) => format!(
            "The target \"{to}\" appears twice in the same wave. One plan can ask the same agent \
             only once; ask for the second one in the next wave. Nothing was delivered."
        ),
    }
}

/// 計画を提示した（Spec 43 — 編集窓）。このターンでは結果を受け取らない。
pub(super) fn plan_proposed(language: Language) -> &'static str {
    language.pick(
        "計画を提示しました。利用者が編集して実行または破棄します。\
         このターンでは結果を受け取りません。提示した事実を短く報告して、ターンを終えてください。",
        "The plan has been proposed. The user will edit it, then \
         dispatch or discard it. You will not receive the results in this turn. \
         Briefly report that the plan was proposed, then end your turn.",
    )
}

/// 進行役のターンが打ち切られて束ねを作らなかった（モデルは読まない — 直後の周回境界で本人も止まる）。
pub(super) fn plan_interrupted(language: Language) -> &'static str {
    language.pick("plan はユーザーの指示で打ち切られました。", "plan was stopped by the user.")
}

/// 束ねの 1 節（見出し `agent_id（表示名）` + 本文）。`answer` が無い（タスクが落ちた）宛先は定型文。
pub(super) fn bundle_section(
    target: &str,
    display: &str,
    answer: Option<String>,
    language: Language,
) -> String {
    let body = answer.unwrap_or_else(|| {
        language
            .pick("答えの取得中に問題が起きました。", "A problem occurred while getting the answer.")
            .to_owned()
    });
    match language {
        Language::Ja => format!("## {target}（{display}）\n{body}"),
        Language::En => format!("## {target} ({display})\n{body}"),
    }
}

/// モデルが本文を返さなかったターンで、コアが代わりに書く返事のきっかけ（`reply:` の `author=core`）。
#[derive(Debug, Clone, Copy)]
pub(super) enum EmptyBody<'a> {
    /// 同じ引数で同じ結果が返り続け、ツール実行を打ち切った（RepeatGuard）。
    Repeat { tool: &'a str },
    /// ツール実行の上限に達した。
    ToolLimit { max: u32 },
    /// 出力トークンはあるのに、本文もツール呼び出しも無い。
    Silent { completion: u64 },
    /// 本文が返らなかった（出力トークンも 0）。
    Nothing,
}

/// コアが代わりに書く返事。`failure` はまとめの呼び出しが失敗したときのエラー文
/// （打ち切り 2 つでだけ使う。無ければ「本文が空だった」と書く）。
pub(super) fn empty_body(cause: EmptyBody<'_>, failure: Option<&str>, language: Language) -> String {
    let reason = || match (failure, language) {
        (Some(err), Language::Ja) => format!("失敗の理由: {err}。"),
        (Some(err), Language::En) => format!("Failure reason: {err}."),
        (None, Language::Ja) => "モデルは応答しましたが本文が空でした。".to_owned(),
        (None, Language::En) => "The model responded, but the text was empty.".to_owned(),
    };
    match (cause, language) {
        // 打ち切りの理由は上限ではないので、上限の直し方を案内しない
        // （直しても直らないものを勧めると、次の依頼がそのぶん燃える）。
        (EmptyBody::Repeat { tool }, Language::Ja) => format!(
            "（`{tool}` を同じ引数で繰り返し呼び、同じ結果が返り続けたため\
             ツール実行を打ち切りました。まとめの生成にも失敗しています。{}\
             頼み方を変えるか、必要な情報を直接渡してください。）",
            reason()
        ),
        (EmptyBody::Repeat { tool }, Language::En) => format!(
            "(Stopped running tools because `{tool}` was called repeatedly with the same \
             arguments and kept returning the same result. Generating the summary also failed. \
             {} Change how you ask, or give the needed information directly.)",
            reason()
        ),
        (EmptyBody::ToolLimit { max }, Language::Ja) => format!(
            "（ツール実行の上限 {max} 回に達し、まとめの生成にも\
             失敗しました。{}\
             エージェント設定で上限を上げるか、依頼を小さく分けてください。）",
            reason()
        ),
        (EmptyBody::ToolLimit { max }, Language::En) => format!(
            "(Reached the tool-run limit of {max}, and generating the summary also failed. \
             {} Raise the limit in the agent settings, or split the request into smaller parts.)",
            reason()
        ),
        // **観測できた事実だけを書く。** 出力トークンがあるのに本文もツール呼び出しも無いなら、
        // モデルは本文以外のブロック（拡張思考など）だけを返している。種別は
        // `dropped content blocks:` の 1 行がログに残す — ここで「thinking です」と断定しない。
        (EmptyBody::Silent { completion }, Language::Ja) => format!(
            "（モデルは出力 {completion} トークンを使いましたが、\
             本文もツール呼び出しも返しませんでした。\
             もう一度頼んでみてください。繰り返すなら頼み方を変えてください。）"
        ),
        (EmptyBody::Silent { completion }, Language::En) => format!(
            "(The model used {completion} output tokens but returned neither text nor a tool call. \
             Please ask again. If it keeps happening, change how you ask.)"
        ),
        (EmptyBody::Nothing, Language::Ja) => {
            "（モデルから本文が返りませんでした。もう一度頼んでみてください。）".to_owned()
        }
        (EmptyBody::Nothing, Language::En) => {
            "(The model returned no text. Please ask again.)".to_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 言語ごとに全部の文を組む（新しい文を足したらここにも足す — 下の 2 本の網の分母）。
    fn every_text(language: Language) -> Vec<(&'static str, String)> {
        let names = ["ジェミー".to_owned(), "ルナ".to_owned()];
        let roster = ["ジェミー", "ルナ"];
        vec![
            ("stopped:interrupted", stopped(Stopped::Interrupted, language).to_owned()),
            ("stopped:budget", stopped(Stopped::BudgetExhausted, language).to_owned()),
            ("stopped:approval", stopped(Stopped::ApprovalPending, language).to_owned()),
            ("handed_off", handed_off(&names, language)),
            ("hop_limit_ask", hop_limit_ask(language).to_owned()),
            ("hop_limit_plan", hop_limit_plan(language).to_owned()),
            ("budget_not_delivered", budget_not_delivered(language).to_owned()),
            ("undeliverable", undeliverable(&"E", language)),
            ("no_answer", no_answer(language).to_owned()),
            ("timed_out", timed_out(language).to_owned()),
            ("circular", circular("agent_2", language)),
            ("plan:missing_tasks", plan_defect(PlanDefect::MissingTasks, language)),
            ("plan:empty_tasks", plan_defect(PlanDefect::EmptyTasks, language)),
            ("plan:missing_fields", plan_defect(PlanDefect::MissingFields { position: 2 }, language)),
            (
                "plan:not_connected",
                plan_defect(
                    PlanDefect::NotConnected { position: 1, to: "agent_9", roster: &roster },
                    language,
                ),
            ),
            ("plan:duplicate", plan_defect(PlanDefect::Duplicate { to: "agent_2" }, language)),
            ("plan_proposed", plan_proposed(language).to_owned()),
            ("plan_interrupted", plan_interrupted(language).to_owned()),
            ("bundle:missing", bundle_section("agent_2", "ジェミー", None, language)),
            ("empty:repeat", empty_body(EmptyBody::Repeat { tool: "grep" }, Some("E"), language)),
            ("empty:tool_limit", empty_body(EmptyBody::ToolLimit { max: 36 }, None, language)),
            ("empty:silent", empty_body(EmptyBody::Silent { completion: 2048 }, None, language)),
            ("empty:nothing", empty_body(EmptyBody::Nothing, None, language)),
        ]
    }

    /// 日本語は 2026-11-12 までの文と逐語で同じ（使う場所に直書きされていた文の写し）。
    #[test]
    fn japanese_texts_are_unchanged() {
        let expected = [
            "この依頼はユーザーの指示で打ち切られました。答えはありません。",
            "この依頼はトークン予算の上限で打ち切られました。答えはありません。",
            "この依頼は承認待ちのコマンドで止まりました。答えはありません。",
            "相手はこの依頼に自分で答えず、ジェミー、ルナ へ会話を渡しました。答えはこちらへ戻りません。必要なら別の相手に頼むか、自分で進めてください。",
            "転送の上限に達したため、これ以上は尋ねられません。",
            "転送の上限に達したため、これ以上は頼めません。何も配送していません。",
            "トークン予算の上限に達したため、配送していません。",
            "相手に尋ねられませんでした: E",
            "相手から答えが返りませんでした。",
            "相手からの答えが時間内に返りませんでした。",
            "宛先「agent_2」はこの依頼の因果の中で答えを待っています（循環する委譲）。配送していません。依頼主への返信で直接答えてください。",
            "plan には tasks（依頼の配列）が必要です。何も配送していません。",
            "plan の tasks が空です。誰にも頼まずに終わりました。頼む相手が居ないなら、plan を呼ばずに自分で答えてください。",
            "2 件目の依頼に to と message の両方が必要です。何も配送していません。",
            "1 件目の宛先「agent_9」は、あなたの接続先ではありません。頼めるのは ジェミー、ルナ です。何も配送していません。",
            "宛先「agent_2」が同じ波に 2 回あります。1 回の plan で同じ相手へ頼めるのは 1 件です。2 件目は次の波で頼んでください。何も配送していません。",
            "計画を提示しました。利用者が編集して実行または破棄します。このターンでは結果を受け取りません。提示した事実を短く報告して、ターンを終えてください。",
            "plan はユーザーの指示で打ち切られました。",
            "## agent_2（ジェミー）\n答えの取得中に問題が起きました。",
            "（`grep` を同じ引数で繰り返し呼び、同じ結果が返り続けたためツール実行を打ち切りました。まとめの生成にも失敗しています。失敗の理由: E。頼み方を変えるか、必要な情報を直接渡してください。）",
            "（ツール実行の上限 36 回に達し、まとめの生成にも失敗しました。モデルは応答しましたが本文が空でした。エージェント設定で上限を上げるか、依頼を小さく分けてください。）",
            "（モデルは出力 2048 トークンを使いましたが、本文もツール呼び出しも返しませんでした。もう一度頼んでみてください。繰り返すなら頼み方を変えてください。）",
            "（モデルから本文が返りませんでした。もう一度頼んでみてください。）",
        ];
        let texts = every_text(Language::Ja);
        assert_eq!(texts.len(), expected.len(), "文を足したら逐語の期待値にも足す");
        for ((name, text), want) in texts.iter().zip(expected) {
            assert_eq!(text, want, "{name}");
        }
    }

    /// 英語の文は日本語を 1 字も含まない（埋め込んだ名前を除く — 名簿と表示名は日本語で渡している）。
    #[test]
    fn english_texts_contain_no_japanese() {
        for (name, text) in every_text(Language::En) {
            let own = text.replace("ジェミー", "").replace("ルナ", "");
            assert!(
                own.chars().all(|c| !matches!(c, '\u{3000}'..='\u{30ff}' | '\u{4e00}'..='\u{9fff}' | '\u{ff00}'..='\u{ffef}')),
                "{name} に日本語が残っている: {text}"
            );
        }
    }

    /// 英語の束ねは全角の括弧を使わない（見出しの枠ごと言語で分ける）。
    #[test]
    fn english_bundle_heading_uses_ascii_parentheses() {
        let section = bundle_section("agent_2", "Gemmy", Some("ok".to_owned()), Language::En);
        assert_eq!(section, "## agent_2 (Gemmy)\nok");
    }
}
