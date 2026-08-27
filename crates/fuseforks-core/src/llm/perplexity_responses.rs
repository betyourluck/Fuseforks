//! Perplexity Responses（`api.perplexity.ai/v1/responses`）の encode / decode
//! （Spec 45）。
//!
//! Perplexity Agent API（`/v1/agent`）の OpenAI 互換エイリアスを使う。
//! **Chat Completions の口を持たない**（`/v1/chat/completions` は 404 —
//! 実測 2026-08-19。互換の口も持つ xAI / Meta / Gemini と逆）。
//!
//! 凍結は `data_contract.yaml` の `perplexity_responses` が正。要点:
//!
//! - **要求のトップレベル型は [`wire::OpenAiResponsesRequest`] を共有**し、
//!   `max_steps: Option<u32>` を加算した（Spec 45 D2）。アプリの
//!   OpenAI Responses ワイヤが送る形そのままで 200 が実測済み（2026-08-19）
//! - **`max_steps` は `finance_search` が ON のときだけ `Some(5)`**（D4）。
//!   送らないと skill 族の finance は `skill_loaded` item だけ返して
//!   **200 のまま黙って空振りする**（実測 2026-08-27）
//! - **出典は annotations ではなく `*_results` 系 output item**（D5）。
//!   annotations は全 probe で 0 件だった
//! - **誤ったツール型は 400 named**（`unknown discriminator value`）で、
//!   受理集合は列挙しない — OpenAI（全列挙）とも xAI（無言の 422）とも違う
//!   第 3 の様式

use super::canonical::{
    ChatRequest, ChatResponse, Finish, Grounding, GroundingEngine, GroundingSource,
    PromptAttachment, ToolCall, ToolChoice, Usage,
};
use super::error::LlmError;
use super::{responses_input, wire};
use crate::attachment::AttachmentKind;
use serde_json::Value;

/// 送るサーバー側ツールの `type`（文書と probe の両方で確認済み）。
const WEB_SEARCH_TOOL: &str = "web_search";
/// 金融検索。1 回 $0.005（実測 — `tool_calls_cost_details` と文書が一致）。
const FINANCE_SEARCH_TOOL: &str = "finance_search";
/// 人物検索。1 回 $0.005。**課金・回数の detail 鍵は `search_people`**
/// （tool 型と揃っていない — 実測 2026-08-27）。
const PEOPLE_SEARCH_TOOL: &str = "people_search";
/// URL 取得。1 回 $0.0005。
const FETCH_URL_TOOL: &str = "fetch_url";

/// `finance_search` に対で送る step 予算（Spec 45 D4）。
///
/// 文書の推奨帯 5〜10 の下限。step は課金と遅延を持つので天井側を選ばない。
/// 設定にしない（Spec 11 の重みと同じ扱い — 利用者に見せない定数）。
const FINANCE_MAX_STEPS: u32 = 5;

/// 固有スキル 4 本の判定済みフラグ（Spec 45 D3）。
///
/// 値は `ModelTemplate::perplexity_*_active()` を通した後のもの
/// （AND 述語の 1 実装 — フラグ単独を判定に使わない規律は呼び出し側が担う）。
/// **bool 4 連の引数にしない**のは、同型の並びは呼び出し側で取り違えても
/// コンパイラが指さないため。
#[derive(Debug, Clone, Copy, Default)]
pub struct Tools {
    /// web 検索。
    pub web_search: bool,
    /// 金融検索（ON なら `max_steps: 5` が対で出る）。
    pub finance_search: bool,
    /// 人物検索。
    pub people_search: bool,
    /// URL 取得。
    pub fetch_url: bool,
}

/// 添付 1 件を part へ写す。**このワイヤは画像しか運べない**（carries 表。
/// 音声・動画・PDF は `invalid type` の名指し 400 — 実測 2026-08-27）。
///
/// [`super::responses_input::image_and_pdf_part`] を使わないのは PDF のため —
/// あちらは `input_file` を組み立てるが、この接続先はその型を拒否する。
/// **`Provider::carries` をここから読まない**（読むと
/// `adapters_match_the_carries_table` が同語反復になる — Spec 37 D2 の網）。
fn attachment_part(attachment: &PromptAttachment) -> Option<wire::ResponsesInputPart> {
    match attachment.kind() {
        AttachmentKind::Image => Some(wire::ResponsesInputPart::InputImage {
            image_url: attachment.data_url(),
        }),
        AttachmentKind::Audio | AttachmentKind::Video | AttachmentKind::Pdf => None,
    }
}

/// canonical → Perplexity Responses wire。
///
/// `use_tools` が偽、または `tool_choice` が [`ToolChoice::None`] のときは
/// **関数ツールもサーバー側ツールも送らない**（他の Responses と同じ規律 —
/// 「ツールを使わせない」は server-side tool にも及ぶ）。**`max_steps` も
/// そのとき `None` に落ちる** — finance を送らない要求に step 予算だけ
/// ぶら下げない。
pub fn encode(
    req: &ChatRequest,
    use_tools: bool,
    tools_on: Tools,
) -> wire::OpenAiResponsesRequest {
    let offer_tools = use_tools && req.tool_choice != ToolChoice::None;
    let mut tools = Vec::new();
    let mut finance = false;
    if offer_tools {
        if tools_on.web_search {
            tools.push(wire::ResponsesTool::Server {
                kind: WEB_SEARCH_TOOL,
            });
        }
        if tools_on.finance_search {
            tools.push(wire::ResponsesTool::Server {
                kind: FINANCE_SEARCH_TOOL,
            });
            finance = true;
        }
        if tools_on.people_search {
            tools.push(wire::ResponsesTool::Server {
                kind: PEOPLE_SEARCH_TOOL,
            });
        }
        if tools_on.fetch_url {
            tools.push(wire::ResponsesTool::Server {
                kind: FETCH_URL_TOOL,
            });
        }
        for spec in &req.tools {
            tools.push(wire::ResponsesTool::Function {
                kind: "function",
                name: spec.name.clone(),
                description: spec.description.clone(),
                parameters: spec.parameters.clone(),
            });
        }
    }

    let tool_choice = match &req.tool_choice {
        ToolChoice::None | ToolChoice::Auto => None,
        ToolChoice::Required => Some(Value::String("required".into())),
        ToolChoice::Specific(name) => Some(serde_json::json!({ "type": "function", "name": name })),
    };

    wire::OpenAiResponsesRequest {
        model: req.model.clone(),
        input: responses_input::encode(&req.messages, attachment_part),
        tools,
        // **空 = 欄ごと省く**（Spec 45）。この接続先に `action.sources` は無く
        // （出典は `*_results` item 側）、読む先の無い欄を送らない。
        include: vec![],
        // probe と 2026-08-19 の実測がこの形（summary/context 込み）で 200。
        // `reasoning_tokens` は 0 で要約も返らないが、**送って無害と実測済みの
        // 形をそのまま使う**ほうが、欄を削って未測定の形を作るより安全。
        reasoning: wire::OpenAiReasoning {
            effort: req.effort.map(|e| e.as_str()),
            summary: "detailed",
            context: "current_turn",
            mode: None,
        },
        store: false,
        max_output_tokens: req.max_tokens,
        tool_choice,
        // finance が実際にツール列へ載ったときだけ対で送る（Spec 45 D4）。
        max_steps: finance.then_some(FINANCE_MAX_STEPS),
    }
}

/// `*_results` のオブジェクト配列（search / people / fetch_url）から出典を写す。
///
/// URL の無い要素は落とす（出典は URI が本体）。表題が URL の複製なら空へ
/// 落とす（annotations と同じ規律）。
fn push_object_sources(
    grounding: &mut Grounding,
    entries: impl Iterator<Item = (Option<String>, Option<String>)>,
) {
    for (url, title) in entries {
        let Some(url) = url else { continue };
        if grounding.sources.iter().any(|s| s.uri == url) {
            continue;
        }
        let title = title.filter(|t| t != &url).unwrap_or_default();
        grounding.sources.push(GroundingSource { uri: url, title });
    }
}

/// Perplexity Responses wire → canonical。
///
/// 応答の型は他の Responses 3 本と共有（probe 11 発が全部 serde で通った）。
/// **このワイヤ固有なのは `*_results` 系 output item の読みと
/// `usage.cost` / `usage.tool_calls_details`**。
pub fn decode(resp: wire::ResponsesResponse) -> Result<ChatResponse, LlmError> {
    let mut texts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    let mut grounding = Grounding {
        engine: GroundingEngine::Perplexity,
        ..Grounding::default()
    };
    // 計器の分子（Spec 45 D6）。**output item の数で数える** — usage の鍵名は
    // tool 型と揃わない例があり（`search_people`）、web の detail は未実測。
    let mut web_n = 0u32;
    let mut finance_n = 0u32;
    let mut people_n = 0u32;
    let mut fetch_n = 0u32;
    let mut reasoning_summary: Vec<String> = Vec::new();
    let mut dropped: Vec<String> = Vec::new();

    for item in resp.output {
        match item.kind.as_str() {
            "message" => {
                for part in item.content.unwrap_or_default() {
                    if let Some(text) = part.text
                        && !text.is_empty()
                    {
                        texts.push(text);
                    }
                    // 実測は全 probe で 0 件だが、**返るなら拾う**（message の
                    // annotations を読む共通形。表題つきの側が重複排除で残る）。
                    for ann in part.annotations {
                        if ann.kind != "url_citation" {
                            continue;
                        }
                        let Some(url) = ann.url else { continue };
                        if grounding.sources.iter().any(|s| s.uri == url) {
                            continue;
                        }
                        let title = ann.title.filter(|t| t != &url).unwrap_or_default();
                        grounding.sources.push(GroundingSource { uri: url, title });
                    }
                }
            }
            // 思考の要約。実測では返らない（`reasoning_tokens` 0）が、
            // 共通形を保つ — 返るようになったとき黙って捨てない。
            "reasoning" => {
                for part in item.summary.iter().flatten() {
                    if part.kind != "summary_text" {
                        continue;
                    }
                    if let Some(text) = part.text.as_ref()
                        && !text.is_empty()
                    {
                        reasoning_summary.push(text.clone());
                    }
                }
            }
            // web 検索の結果（Spec 45 D5）。検索語と出典の両方を運ぶ。
            "search_results" => {
                web_n += 1;
                for query in item.queries.iter().flatten() {
                    if !grounding.queries.contains(query) {
                        grounding.queries.push(query.clone());
                    }
                }
                push_object_sources(
                    &mut grounding,
                    item.results
                        .into_iter()
                        .flatten()
                        .map(|r| (r.url, r.title)),
                );
            }
            "people_search_results" => {
                people_n += 1;
                for query in item.queries.iter().flatten() {
                    if !grounding.queries.contains(query) {
                        grounding.queries.push(query.clone());
                    }
                }
                push_object_sources(
                    &mut grounding,
                    item.results
                        .into_iter()
                        .flatten()
                        .map(|r| (r.url, r.title)),
                );
            }
            "fetch_url_results" => {
                fetch_n += 1;
                push_object_sources(
                    &mut grounding,
                    item.contents
                        .into_iter()
                        .flatten()
                        .map(|c| (c.url, c.title)),
                );
            }
            // 金融検索の結果。**URL は `results[].sources`（裸の文字列の
            // 二重配列）にだけ入り、表題を持たない**（実測 2026-08-27）。
            // 平坦化して写す。**表題は空のまま** — 空文字で捏造せず、
            // 表示層が URI をそのまま出す（Spec 45 D5）。
            "finance_results" => {
                finance_n += 1;
                for url in item
                    .results
                    .into_iter()
                    .flatten()
                    .flat_map(|r| r.sources.into_iter().flatten())
                {
                    if !grounding.sources.iter().any(|s| s.uri == url) {
                        grounding.sources.push(GroundingSource {
                            uri: url,
                            title: String::new(),
                        });
                    }
                }
            }
            "function_call" => {
                let raw = item.arguments.unwrap_or_default();
                let args: Value = serde_json::from_str(&raw).map_err(|source| LlmError::Parse {
                    source,
                    raw: raw.clone(),
                })?;
                tool_calls.push(ToolCall {
                    id: item.call_id.unwrap_or_default(),
                    name: item.name.unwrap_or_default(),
                    args,
                    extra: None,
                });
            }
            // `skill_loaded` は既知だが出典を持たない（`{name, type}` だけ）。
            // **捨てるが数える**（Spec 45 D5 / #72）— `dropped` の計器行に載る。
            other => dropped.push(other.to_owned()),
        }
    }

    let usage = resp
        .usage
        .as_ref()
        .map(|u| Usage {
            prompt: u.input_tokens,
            completion: u.output_tokens,
            cache_read: u
                .input_tokens_details
                .as_ref()
                .map(|d| d.cached_tokens)
                .unwrap_or_default(),
            cache_write: u
                .input_tokens_details
                .as_ref()
                .map(|d| d.cache_write_tokens)
                .unwrap_or_default(),
            cache_write_1h: 0,
            reasoning: u
                .output_tokens_details
                .as_ref()
                .map(|d| d.reasoning_tokens)
                .unwrap_or_default(),
        })
        .unwrap_or_default();

    // 未知種別は数えてから捨てる（#72）。`skill_loaded` もここに出る。
    if !dropped.is_empty() {
        crate::note!(
            "dropped content blocks: kinds={} count={} output_tokens={} text_chars={} tool_calls={}",
            dropped.join("+"),
            dropped.len(),
            usage.completion,
            texts.iter().map(|t| t.chars().count()).sum::<usize>(),
            tool_calls.len(),
        );
    }

    // ツールの計器（Spec 45 D6）。行名は engine ごとに分ける前例
    // （`xai search:` / `openai search:` / `meta search:`）のまま。
    //
    // - `finance=` 等の N は **output item の数**（decode が確実に持っている側）
    // - `invocations=` は `usage.tool_calls_details` の**生の列挙** — 鍵名を
    //   enum で固定しないので、`search_people` の綴りが直っても未知のツールが
    //   増えてもサイレント欠損にならない。item 数との食い違いもこの 1 行で読める
    // - `tool_cost_usd=` は `usage.cost.tool_calls_cost`（**ツール課金だけ**。
    //   `total_cost` は入出力込みで、この行名の下に置くと誤読する）。
    //   欄が来なければ `-` — 「来るはずの欄が来ていない」がそのまま読める
    let invocations: Vec<String> = resp
        .usage
        .as_ref()
        .and_then(|u| u.tool_calls_details.as_ref())
        .map(|details| {
            details
                .iter()
                .map(|(name, v)| format!("{name}:{}", v.invocation.unwrap_or_default()))
                .collect()
        })
        .unwrap_or_default();
    if web_n + finance_n + people_n + fetch_n > 0 || !invocations.is_empty() {
        let tool_cost = resp
            .usage
            .as_ref()
            .and_then(|u| u.cost.as_ref())
            .and_then(|c| c.tool_calls_cost)
            .map(|c| format!("{c:.6}"))
            .unwrap_or_else(|| "-".to_owned());
        crate::note!(
            "pplx tools: finance={finance_n} people={people_n} fetch={fetch_n} web={web_n} sources={} invocations={} tool_cost_usd={tool_cost}",
            grounding.sources.len(),
            if invocations.is_empty() {
                "-".to_owned()
            } else {
                invocations.join(",")
            },
        );
    }

    let text = if texts.is_empty() {
        None
    } else {
        Some(texts.join("\n\n"))
    };

    let finish = if !tool_calls.is_empty() {
        Finish::ToolUse
    } else {
        match resp.status.as_deref() {
            Some("completed") => Finish::Stop,
            Some("incomplete")
                if resp
                    .incomplete_details
                    .as_ref()
                    .and_then(|d| d.reason.as_deref())
                    == Some("max_output_tokens") =>
            {
                Finish::Length
            }
            _ => Finish::Other,
        }
    };

    Ok(ChatResponse {
        text,
        tool_calls,
        finish,
        usage,
        grounding,
        reasoning_summary,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ChatMessage;

    fn base_request() -> ChatRequest {
        // `plain` の既定は `ToolChoice::None`（ツールを一切送らない要求）なので、
        // golden はツールを提示できる `Auto` へ上げる（openai_responses の
        // golden と同じ形）。
        let mut req = ChatRequest::plain(
            "perplexity/deepseek-v4-flash-0731",
            vec![
                ChatMessage::system("あなたは検証用の応答者です"),
                ChatMessage::user("AAPL の現在価格を教えて"),
            ],
            1024,
        );
        req.tool_choice = ToolChoice::Auto;
        req
    }

    /// golden: finance ON のリクエスト全体。**`max_steps:5` が対で出る**こと、
    /// `include` が欄ごと消えることが 1 本で読める（Spec 45 D2 / D4）。
    #[test]
    fn encode_golden_finance_on_sends_max_steps() {
        let tools_on = Tools {
            finance_search: true,
            ..Tools::default()
        };
        let json = serde_json::to_string(&encode(&base_request(), true, tools_on)).unwrap();
        assert_eq!(
            json,
            r#"{"model":"perplexity/deepseek-v4-flash-0731","input":[{"role":"system","content":"あなたは検証用の応答者です"},{"role":"user","content":"AAPL の現在価格を教えて"}],"tools":[{"type":"finance_search"}],"reasoning":{"summary":"detailed","context":"current_turn"},"store":false,"max_output_tokens":1024,"max_steps":5}"#
        );
    }

    /// golden: finance OFF（web だけ）なら **`max_steps` は欄ごと出ない**。
    ///
    /// 上の golden と対で読む — 「finance ON のときだけ」の両側を凍結する。
    #[test]
    fn encode_golden_without_finance_omits_max_steps() {
        let tools_on = Tools {
            web_search: true,
            ..Tools::default()
        };
        let json = serde_json::to_string(&encode(&base_request(), true, tools_on)).unwrap();
        assert_eq!(
            json,
            r#"{"model":"perplexity/deepseek-v4-flash-0731","input":[{"role":"system","content":"あなたは検証用の応答者です"},{"role":"user","content":"AAPL の現在価格を教えて"}],"tools":[{"type":"web_search"}],"reasoning":{"summary":"detailed","context":"current_turn"},"store":false,"max_output_tokens":1024}"#
        );
    }

    /// `use_tools` が偽なら **finance が ON でも `max_steps` を送らない** —
    /// ツールを送らない要求に step 予算だけぶら下げない。
    #[test]
    fn no_tools_means_no_max_steps_even_with_finance_on() {
        let tools_on = Tools {
            finance_search: true,
            ..Tools::default()
        };
        let body = encode(&base_request(), false, tools_on);
        assert!(body.tools.is_empty());
        assert_eq!(body.max_steps, None);
    }

    /// decode: probe の実物形（縮約）から 4 種の `*_results` と usage を読む。
    ///
    /// - `finance_results` の URL は `results[].sources` の**平坦化**で写り、
    ///   表題は空のまま（捏造しない — Spec 45 D5）
    /// - `search_results` は表題つきで写る
    /// - `skill_loaded` は捨てられる（本文には混ざらない）
    #[test]
    fn decode_reads_perplexity_result_items_and_cost() {
        let raw = r#"{
            "output": [
                {"name": "finance", "type": "skill_loaded"},
                {"type": "finance_results", "categories": ["quote"],
                 "results": [{"category": "quote", "content": "| AAPL | 313.45 |",
                              "sources": ["https://www.perplexity.ai/finance/AAPL",
                                          "https://www.perplexity.ai/finance/MSFT"],
                              "tickers": ["AAPL", "MSFT"]}],
                 "tickers": ["AAPL", "MSFT"]},
                {"type": "search_results",
                 "queries": ["Dario Amodei CEO"],
                 "results": [{"id": 1, "url": "https://darioamodei.com/",
                              "title": "Dario Amodei", "snippet": "…", "source": "web"}]},
                {"type": "fetch_url_results",
                 "contents": [{"url": "https://docs.perplexity.ai/llms.txt",
                               "title": "Perplexity docs", "snippet": "…"}]},
                {"type": "message", "role": "assistant", "status": "completed",
                 "content": [{"type": "output_text", "annotations": [],
                              "text": "AAPL は $313.45 です"}]}
            ],
            "status": "completed",
            "usage": {
                "input_tokens": 12148, "output_tokens": 350,
                "input_tokens_details": {"cached_tokens": 3968},
                "output_tokens_details": {"reasoning_tokens": 0},
                "cost": {"currency": "USD", "input_cost": 0.00106,
                         "output_cost": 0.00009, "tool_calls_cost": 0.005,
                         "tool_calls_cost_details": {"finance_search": 0.005},
                         "total_cost": 0.00626},
                "tool_calls_details": {"finance_search": {"invocation": 1}}
            }
        }"#;
        let parsed: wire::ResponsesResponse = serde_json::from_str(raw).unwrap();
        let decoded = decode(parsed).unwrap();

        assert_eq!(decoded.text.as_deref(), Some("AAPL は $313.45 です"));
        assert_eq!(decoded.grounding.engine, GroundingEngine::Perplexity);
        // finance の 2 本（表題なし）+ search の 1 本（表題あり）+ fetch の 1 本。
        let uris: Vec<&str> = decoded.grounding.sources.iter().map(|s| s.uri.as_str()).collect();
        assert_eq!(
            uris,
            [
                "https://www.perplexity.ai/finance/AAPL",
                "https://www.perplexity.ai/finance/MSFT",
                "https://darioamodei.com/",
                "https://docs.perplexity.ai/llms.txt",
            ]
        );
        let dario = &decoded.grounding.sources[2];
        assert_eq!(dario.title, "Dario Amodei", "search_results は表題つきで写る");
        assert_eq!(
            decoded.grounding.sources[0].title, "",
            "finance の表題は空のまま（捏造しない）"
        );
        assert_eq!(decoded.grounding.queries, ["Dario Amodei CEO"]);
        assert_eq!(decoded.usage.prompt, 12148);
        assert_eq!(decoded.usage.cache_read, 3968);
        assert_eq!(decoded.finish, Finish::Stop);
        // skill_loaded は本文に混ざらない（dropped 行の側に出る）。
        assert!(!decoded.text.unwrap().contains("skill_loaded"));
    }

    /// 画像しか part にならない（carries 表の adapter 側の主張）。
    ///
    /// PDF を組み立てると `invalid type "input_file"` の 400 になる
    /// （実測 2026-08-27 — 本家 open_ai_responses と割れているマス）。
    #[test]
    fn attachment_part_builds_image_only() {
        use crate::llm::PromptMediaType;
        let image = PromptAttachment::new(PromptMediaType::Webp, "QUJD");
        let pdf = PromptAttachment::new(PromptMediaType::Pdf, "QUJD");
        assert!(attachment_part(&image).is_some());
        assert!(attachment_part(&pdf).is_none(), "PDF は組み立てない（400 の実測）");
    }
}
