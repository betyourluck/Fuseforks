//! OpenAI Responses（`/v1/responses`）の encode / decode（Spec 34）。
//!
//! `/v1/chat/completions` では原理的に取れないものが 3 つある:
//! 思考の要約**本文**（数は来るが本文が来ない）/ web 検索（一般の gpt-5 系の
//! `web_search` は Responses 専用）/ `failures.md` #77 の代償
//! （ツールを持つ限り `reasoning_effort: "none"` を強制していた = 推論モデルの
//! 推論を殺したまま使っていた。400 の本文自身が逃げ道にこの口を挙げる）。
//!
//! 凍結は `data_contract.yaml` の `openai_responses` が正。要点:
//! - `store: false` を常送（2 つの Responses ワイヤで揃える）
//! - `reasoning` は 4 欄。`summary: "detailed"` 常送・`context: "current_turn"` 明示
//! - **`openai_compat::reasoning_effort` を呼ばない** — あの `"none"` 強制は
//!   chat/completions 固有の制約への対処で、**この口はその制約が無いことが
//!   移る理由そのもの**
//! - **`temperature` の欄が型に無い**（400 実測）
//! - `web_search_call.action` は 3 種。**`queries < calls` は正常**

use super::canonical::{
    ChatRequest, ChatResponse, Finish, Grounding, GroundingEngine, GroundingSource, ToolCall,
    ToolChoice, Usage,
};
use super::error::LlmError;
use super::{responses_input, wire};
use serde_json::Value;

/// 送る検索ツールの `type`。
///
/// `web_search_preview` は**別名**で、1 本でも 2 本同時でも `input_tokens` が
/// 4,454 で完全に一致する（実測）= 同じ宣言ブロックが注入される。
/// **旧名を選ぶ理由が 1 つも無い**ので現行名だけを送る。
const WEB_SEARCH_TOOL: &str = "web_search";

/// 検索呼び出しとして受ける output 種別。
const SEARCH_CALL_KIND: &str = "web_search_call";

/// canonical → OpenAI Responses wire。
///
/// `use_tools` が偽、または `tool_choice` が [`ToolChoice::None`] のときは
/// **関数ツールも検索ツールも送らない**（xAI と同じ規律 — 「ツールを使わせない」は
/// server-side tool にも及ぶ。要約の呼び出しで検索が走ると、押した人が
/// 検索の注入 input（実測 4,434 トークン）まで払う）。
pub fn encode(
    req: &ChatRequest,
    use_tools: bool,
    web_search: bool,
    reasoning_pro: bool,
) -> wire::OpenAiResponsesRequest {
    let offer_tools = use_tools && req.tool_choice != ToolChoice::None;
    let mut tools = Vec::new();
    if offer_tools {
        if web_search {
            tools.push(wire::ResponsesTool::Server {
                kind: WEB_SEARCH_TOOL,
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
        input: responses_input::encode(&req.messages, responses_input::image_and_pdf_part),
        tools,
        // **常送する**（Spec 34 D12）。annotations は**モデルが引用した分だけ**で、
        // 引用しなければ 0 件になる（実測 — 金融の問いで annotations=0）。
        // 触れた全ソースはこの鍵でしか取れない。
        include: vec!["web_search_call.action.sources"],
        reasoning: wire::OpenAiReasoning {
            // **丸めない。** canonical の 5 値はすべて受理される（実測 —
            // 誤った値 `minimal` を 1 つ送ったら、400 の本文が受理集合
            // 'none', 'low', 'medium', 'high', 'xhigh', 'max' を列挙した）。
            // openai_compat が high へ丸めるのは、受け付けないプロバイダ向けの縮退。
            //
            // **`openai_compat::reasoning_effort` を呼ばない** — あの関数の先頭の
            // gpt-5 → "none" は chat/completions 固有の制約への対処で、
            // 写すとこのワイヤへ移る理由（#77 の代償の解消）を自分で潰す。
            effort: req.effort.map(|e| e.as_str()),
            summary: "detailed",
            context: "current_turn",
            mode: reasoning_pro.then_some("pro"),
        },
        store: false,
        max_output_tokens: req.max_tokens,
        tool_choice,
    }
}

/// OpenAI Responses wire → canonical。
pub fn decode(resp: wire::ResponsesResponse) -> Result<ChatResponse, LlmError> {
    let mut texts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    let mut grounding = Grounding {
        engine: GroundingEngine::OpenAi,
        ..Grounding::default()
    };
    // action の内訳。`search` 以外は検索語を持たないので、queries の分母にしない。
    let mut actions: Vec<(String, u32)> = Vec::new();
    let mut search_calls = 0u32;
    // 触れた全ソース。**annotations を入れ終えてから足す**（表題を落とさないため）。
    let mut touched: Vec<String> = Vec::new();
    // URL を持たない外部フィードの名前（計器専用）。
    let mut api_sources: Vec<String> = Vec::new();
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
            // **本文へ混ぜず、要約だけを取る。** xAI と違って
            // `dropped content blocks:` は出さない — あちらは reasoning を
            // 捨てているが、こちらは読んでいるので「捨てた」ではない。
            // **要約が 0 件の回は普通にある**（ツール併用回の大半。Spec 34 D4）ので、
            // そこへ計器を置くと常時鳴って診断の役に立たない。
            "reasoning" => {
                for part in item.summary.iter().flatten() {
                    if part.kind != "summary_text" {
                        continue;
                    }
                    let Some(text) = part.text.as_ref() else {
                        continue;
                    };
                    // 空文字は落とす（0 字は表示するものが無い）。長さでは切らない。
                    if !text.is_empty() {
                        reasoning_summary.push(text.clone());
                    }
                }
            }
            SEARCH_CALL_KIND => {
                search_calls += 1;
                // `action` は search / open_page / find_in_page の 3 種。
                // 後 2 者は検索語を持たないので、**取れないことは失敗ではない**。
                let kind = item
                    .action
                    .as_ref()
                    .and_then(|a| a.kind.clone())
                    .unwrap_or_else(|| "-".to_owned());
                match actions.iter_mut().find(|(name, _)| name == &kind) {
                    Some((_, count)) => *count += 1,
                    None => actions.push((kind, 1)),
                }
                if let Some(query) = item.action.as_ref().and_then(|a| a.query.clone())
                    && !grounding.queries.contains(&query)
                {
                    grounding.queries.push(query);
                }
                // 触れた全ソース（Spec 34 D12）。**ここでは溜めるだけで、
                // `Grounding` へは annotations を入れた後で足す** — 注釈の側だけが
                // 表題を持つので、先に URL だけを入れると重複排除で表題が落ちる。
                for source in item.action.iter().flat_map(|a| a.sources.iter()) {
                    match source.kind.as_deref() {
                        Some("url") => {
                            if let Some(url) = source.url.clone() {
                                touched.push(url);
                            }
                        }
                        // **URL を持たない外部フィード**（`oai-finance` など）。
                        // **`Grounding.sources` へは入れない** — URI ではないものを
                        // URI の欄へ入れると、画面が「出典」として嘘を出す。
                        // 計器にだけ残すと「0 件だったのは内部 API へ回ったから」が読める。
                        _ => {
                            if let Some(name) = source.name.clone()
                                && !api_sources.contains(&name)
                            {
                                api_sources.push(name);
                            }
                        }
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
            other => dropped.push(other.to_owned()),
        }
    }

    // **annotations の後に足す。** 引用された出典は表題を持つので先に入っており、
    // ここで同じ URL が来ても重複排除で落ちる = 表題つきの側が残る。
    for url in touched {
        if !grounding.sources.iter().any(|s| s.uri == url) {
            grounding.sources.push(GroundingSource {
                uri: url,
                title: String::new(),
            });
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
            // **`ResponsesInputTokensDetails` は 3 本の Responses ワイヤで共有**なので、
            // 欄を 1 つ足すと 3 本に効く。ただし decode の式は 3 ファイルに別々に
            // 書かれているので、テストも 3 本要る（Spec 40 P1）。
            cache_write: u
                .input_tokens_details
                .as_ref()
                .map(|d| d.cache_write_tokens)
                .unwrap_or_default(),
            cache_write_1h: 0,
            // `output_tokens` の内数。足さない（Spec 32 D2）。
            reasoning: u
                .output_tokens_details
                .as_ref()
                .map(|d| d.reasoning_tokens)
                .unwrap_or_default(),
        })
        .unwrap_or_default();

    // 未知種別は数えてから捨てる（#72）。
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

    // 検索の計器。**行名は engine ごとに分ける** — `xai search:` は Spec 31 P5 の
    // 観測記録がその文字列を指しており、統一すると過去の観測が引けなくなる。
    //
    // **`actions=` の内訳が要る理由**: Spec 31 は `queries == calls` を合格条件に
    // したが、この口は `open_page` / `find_in_page` が検索語を持たないので
    // **`queries < calls` が正常**。等式を写すと常に不合格に見える。
    // **ticks は書かない** — `cost_in_usd_ticks` は xAI の欄で、OpenAI にあるかを
    // 測っていない。無い欄を `-` で埋めると「測ったが無かった」と「見ていない」を畳む。
    if search_calls > 0 {
        crate::note!(
            "openai search: calls={} actions={} sources={} queries={} api_sources={} api_names={}",
            search_calls,
            actions
                .iter()
                .map(|(name, count)| format!("{name}:{count}"))
                .collect::<Vec<_>>()
                .join(","),
            grounding.sources.len(),
            grounding.queries.len(),
            api_sources.len(),
            if api_sources.is_empty() {
                "-".to_owned()
            } else {
                api_sources.join(",")
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
    use crate::llm::canonical::{ChatMessage, Effort, ToolSpec};

    fn spec(name: &str) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: "テスト用".into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }
    }

    fn base_request() -> ChatRequest {
        ChatRequest {
            model: "gpt-5.6-terra".into(),
            messages: vec![
                ChatMessage::system("あなたは検証用の応答者です"),
                ChatMessage::user("AAPL の現在価格を教えて"),
            ],
            tools: vec![spec("get_price")],
            tool_choice: ToolChoice::Auto,
            temperature: Some(0.7),
            max_tokens: 1024,
            effort: None,
            cacheable_prefix_len: 0,
        }
    }

    /// golden: リクエスト全体の形。**`temperature` が出ないこと**が
    /// 1 本で読める（canonical には 0.7 が入っているが、型に欄が無い）。
    /// `store` / `summary` / `context` の常送も同じ 1 本で凍る。
    #[test]
    fn encode_golden_sends_reasoning_and_store_but_never_temperature() {
        let body = encode(&base_request(), true, true, false);
        let json = serde_json::to_string(&body).unwrap();
        assert_eq!(
            json,
            r#"{"model":"gpt-5.6-terra","input":[{"role":"system","content":"あなたは検証用の応答者です"},{"role":"user","content":"AAPL の現在価格を教えて"}],"tools":[{"type":"web_search"},{"type":"function","name":"get_price","description":"テスト用","parameters":{"type":"object","properties":{}}}],"include":["web_search_call.action.sources"],"reasoning":{"summary":"detailed","context":"current_turn"},"store":false,"max_output_tokens":1024}"#
        );
    }

    /// **負の対照 1**: `temperature` はどんな入力でもワイヤへ出ない（D11）。
    /// gpt-5.6 系は 400 `Unsupported parameter: 'temperature'` を返す。
    /// **モデル名の送り分けではなく、欄を持たないことで閉じている。**
    #[test]
    fn temperature_never_reaches_the_wire() {
        let mut req = base_request();
        req.temperature = Some(1.5);
        let json = serde_json::to_string(&encode(&req, true, false, false)).unwrap();
        assert!(!json.contains("temperature"), "温度が漏れている: {json}");
    }

    /// **負の対照 2**: 上限欄は `max_output_tokens` の 1 名だけ。
    /// `max_completion_tokens` を送ると `failures.md` #76 の逆向きを踏む。
    #[test]
    fn max_completion_tokens_is_never_sent() {
        let json = serde_json::to_string(&encode(&base_request(), true, false, false)).unwrap();
        assert!(json.contains(r#""max_output_tokens":1024"#));
        assert!(!json.contains("max_completion_tokens"));
    }

    /// **負の対照 3**: gpt-5 系 + ツールありでも `"none"` を出さない（D4）。
    /// あの強制は chat/completions 固有の制約への対処で、**このワイヤは
    /// その制約が無いことが移る理由そのもの**。写すと Goal を自分で潰す。
    #[test]
    fn gpt5_with_tools_does_not_force_effort_none() {
        let body = encode(&base_request(), true, false, false);
        assert_eq!(body.reasoning.effort, None, "未指定なら欄ごと省く");
        assert!(!body.tools.is_empty(), "ツールは送っている（前提の確認）");

        let mut req = base_request();
        req.effort = Some(Effort::Low);
        assert_eq!(encode(&req, true, false, false).reasoning.effort, Some("low"));
    }

    /// `effort` は**丸めない**。canonical の 5 値はすべて受理される
    /// （実測 — 誤った値 `minimal` の 400 が受理集合を列挙した:
    /// 'none', 'low', 'medium', 'high', 'xhigh', 'max'）。
    /// `openai_compat` が high へ丸めるのは、受け付けないプロバイダ向けの縮退。
    #[test]
    fn effort_is_sent_verbatim_without_rounding() {
        for (effort, wire_value) in [
            (Effort::Low, "low"),
            (Effort::Medium, "medium"),
            (Effort::High, "high"),
            (Effort::XHigh, "xhigh"),
            (Effort::Max, "max"),
        ] {
            let mut req = base_request();
            req.effort = Some(effort);
            assert_eq!(
                encode(&req, true, false, false).reasoning.effort,
                Some(wire_value),
                "{effort:?} が丸められている"
            );
        }
    }

    /// Pro モードはトグル。**OFF では欄ごと省く** —
    /// 送らないのと `"standard"` は `input_tokens` が完全に一致する（実測 20 / 20）。
    #[test]
    fn reasoning_pro_is_a_toggle_that_omits_the_field_when_off() {
        assert_eq!(encode(&base_request(), true, false, false).reasoning.mode, None);
        assert_eq!(
            encode(&base_request(), true, false, true).reasoning.mode,
            Some("pro")
        );
        let json = serde_json::to_string(&encode(&base_request(), true, false, false)).unwrap();
        // **引用符ごと突き合わせる。** 素の `mode` で検査すると `"model"` に
        // 含まれて必ず真になり、**実装が正しくてもテストが赤くなる**
        // （実際に踏んだ）。部分文字列で「欄が無いこと」を検査するときは、
        // その文字列が他の欄名の一部でないかを先に数える。
        assert!(!json.contains(r#""mode""#), "OFF で mode が出ている: {json}");
        assert!(json.contains(r#""model""#), "model は出ている（前提の確認）");
    }

    /// `ToolChoice::None` では検索ツールも関数ツールも送らない。
    /// 要約の呼び出しで検索が走ると、押した人が注入 input（実測 4,434）まで払う。
    #[test]
    fn tool_choice_none_sends_no_tools_at_all() {
        let mut req = base_request();
        req.tool_choice = ToolChoice::None;
        let body = encode(&req, true, true, false);
        assert!(body.tools.is_empty());
        let json = serde_json::to_string(&body).unwrap();
        // **ツールの形ごと突き合わせる。** 素の `web_search` で検査すると
        // `include` の値 `"web_search_call.action.sources"` に含まれて必ず真になる。
        // **この Spec で 2 度目**（1 度目は `mode` が `"model"` に含まれた）。
        // **一般化: 部分文字列で「無いこと」を検査するときは、欄名だけでなく
        // 値の側にもその文字列が現れないかを数える。**
        assert!(
            !json.contains(r#"{"type":"web_search"}"#),
            "検索ツールが漏れている: {json}"
        );
    }

    /// `web_search_preview` は送らない（別名で、input_tokens が完全に一致する）。
    #[test]
    fn only_the_current_search_tool_name_is_sent() {
        let json = serde_json::to_string(&encode(&base_request(), true, true, false)).unwrap();
        assert!(json.contains(r#"{"type":"web_search"}"#));
        assert!(!json.contains("web_search_preview"));
    }

    /// 関数往復の再送形（xAI と同じ item 型を共有している）。
    #[test]
    fn tool_roundtrip_history_encodes_as_call_and_output_items() {
        let mut req = base_request();
        let mut assistant = ChatMessage::assistant("");
        assistant.tool_calls.push(ToolCall {
            id: "call-1".into(),
            name: "get_price".into(),
            args: serde_json::json!({"symbol": "AAPL"}),
            extra: None,
        });
        req.messages.push(assistant);
        req.messages.push(ChatMessage::tool_result(
            "call-1",
            "get_price",
            r#"{"price": 231.5}"#,
        ));

        let json = serde_json::to_string(&encode(&req, true, false, false)).unwrap();
        assert!(json.contains(
            r#"{"type":"function_call","call_id":"call-1","name":"get_price","arguments":"{\"symbol\":\"AAPL\"}"}"#
        ));
        assert!(json.contains(
            r#"{"type":"function_call_output","call_id":"call-1","output":"{\"price\": 231.5}"}"#
        ));
    }

    /// P0a の実応答から要点を写した decode golden。
    /// **`encrypted_content` は型に欄が無いので落ちる** — この村は思考を
    /// 往復させないので読む必要が無い（契約 `intentionally_unread`）。
    #[test]
    fn decode_reads_summary_sources_and_usage() {
        let raw = r#"{
            "status": "completed",
            "output": [
                {"type": "reasoning", "id": "rs_1",
                 "encrypted_content": "gAAAAA...",
                 "summary": [{"type": "summary_text", "text": "The user asks a puzzle…"}]},
                {"type": "web_search_call", "action": {"type": "search", "query": "AAPL price"}},
                {"type": "message", "role": "assistant", "content": [
                    {"type": "output_text", "text": "231.5 ドルです。",
                     "annotations": [
                        {"type": "url_citation", "url": "https://example.test/a", "title": "A"},
                        {"type": "url_citation", "url": "https://example.test/b", "title": "https://example.test/b"}
                     ]}
                ]}
            ],
            "usage": {
                "input_tokens": 4454,
                "input_tokens_details": {"cached_tokens": 4411, "cache_write_tokens": 0},
                "output_tokens": 307,
                "output_tokens_details": {"reasoning_tokens": 176}
            }
        }"#;
        let decoded = decode(serde_json::from_str::<wire::ResponsesResponse>(raw).unwrap()).unwrap();

        assert_eq!(decoded.text.as_deref(), Some("231.5 ドルです。"));
        assert_eq!(decoded.finish, Finish::Stop);
        assert_eq!(decoded.grounding.engine, GroundingEngine::OpenAi);
        assert_eq!(decoded.grounding.queries, vec!["AAPL price"]);
        assert_eq!(decoded.grounding.sources.len(), 2);
        // title が URL の複製なら空へ落ちる（表示側が URL を二重に出さない）。
        assert_eq!(decoded.grounding.sources[1].title, "");
        assert_eq!(decoded.reasoning_summary, vec!["The user asks a puzzle…"]);
        assert_eq!(decoded.usage.prompt, 4454);
        // **受け皿の配線を実機の前に固定する**（Spec 40 P1）。この欄は
        // **2026-08-11 の probe で 4,411 を実測**していた（Spec 34 P0a の S4）。
        // **fixture にも当時から書かれていたが、ワイヤ型に欄が無く 7 日間
        // 黙って落ちていた** — このテストはその再発を留める。
        assert_eq!(decoded.usage.cache_write, 0, "この fixture の値は 0");
        assert_eq!(decoded.usage.cache_write_1h, 0, "TTL 別の内訳は無い");
        assert_eq!(decoded.usage.cache_read, 4411);
        assert_eq!(decoded.usage.reasoning, 176);
        // reasoning は output_tokens の内数。外数で実装すると 4,937 になる。
        assert_eq!(decoded.usage.total(), 4_761);
    }

    /// **`queries < calls` は正常**（Spec 34 D9）。`open_page` /
    /// `find_in_page` は検索語を持たないので、Spec 31 の `queries == calls` を
    /// 写すとページを開いただけの周がある応答が常に不合格に見える。
    #[test]
    fn page_actions_count_as_calls_but_carry_no_query() {
        let raw = r#"{
            "status": "completed",
            "output": [
                {"type": "web_search_call", "action": {"type": "search", "query": "a"}},
                {"type": "web_search_call", "action": {"type": "open_page", "url": "https://x.test"}},
                {"type": "web_search_call", "action": {"type": "find_in_page", "pattern": "b"}},
                {"type": "message", "content": [{"type": "output_text", "text": "ok", "annotations": []}]}
            ]
        }"#;
        let decoded = decode(serde_json::from_str::<wire::ResponsesResponse>(raw).unwrap()).unwrap();
        assert_eq!(decoded.grounding.queries, vec!["a"], "検索語は search の 1 件だけ");
        assert!(decoded.tool_calls.is_empty(), "検索呼び出しはツール呼び出しではない");
    }

    /// **触れた全ソースを拾う**（Spec 34 D12）。annotations は**引用した分だけ**なので、
    /// モデルが引用しなければ 0 件になる（実機で金融の問いが annotations=0 だった）。
    ///
    /// **表題は注釈の側にしか無い。** だから `action.sources` は annotations の
    /// 後で足す — 先に URL だけ入れると、重複排除で表題つきの側が落ちる。
    #[test]
    fn touched_sources_are_added_without_losing_annotation_titles() {
        let raw = r#"{
            "status": "completed",
            "output": [
                {"type": "web_search_call", "action": {"type": "search", "query": "rust 1.90",
                 "sources": [
                    {"type": "url", "url": "https://blog.rust-lang.org/a"},
                    {"type": "url", "url": "https://example.test/never-cited"}
                 ]}},
                {"type": "message", "content": [
                    {"type": "output_text", "text": "本文", "annotations": [
                        {"type": "url_citation", "url": "https://blog.rust-lang.org/a", "title": "Rust 1.90"}
                    ]}
                ]}
            ]
        }"#;
        let decoded = decode(serde_json::from_str::<wire::ResponsesResponse>(raw).unwrap()).unwrap();

        assert_eq!(decoded.grounding.sources.len(), 2, "引用されていない URL も拾う");
        // 引用された側は表題を保つ（順序も引用が先）。
        assert_eq!(decoded.grounding.sources[0].uri, "https://blog.rust-lang.org/a");
        assert_eq!(decoded.grounding.sources[0].title, "Rust 1.90");
        assert_eq!(decoded.grounding.sources[1].uri, "https://example.test/never-cited");
        assert_eq!(decoded.grounding.sources[1].title, "");
    }

    /// **URL を持たない外部フィードは `Grounding.sources` へ入れない**（Spec 34 D12）。
    ///
    /// `{"type":"api","name":"oai-finance"}` は URI ではないので、URI の欄へ入れると
    /// 画面が「出典」として嘘を出す。**計器にだけ残す** — そうすれば
    /// 「出典 0 件だったのは内部 API へ回ったから」がログから読める。
    /// 実機でダウ平均を訊いた回がこの形だった（annotations=0 / results=[]）。
    #[test]
    fn api_feeds_never_become_sources() {
        let raw = r#"{
            "status": "completed",
            "output": [
                {"type": "web_search_call", "action": {"type": "search", "query": "finance: DJI",
                 "sources": [{"type": "api", "name": "oai-finance"}]}},
                {"type": "message", "content": [{"type": "output_text", "text": "53,975.98 ドル", "annotations": []}]}
            ]
        }"#;
        let decoded = decode(serde_json::from_str::<wire::ResponsesResponse>(raw).unwrap()).unwrap();

        assert!(
            decoded.grounding.sources.is_empty(),
            "URI を持たないソースが出典として出ている: {:?}",
            decoded.grounding.sources
        );
        // 検索した事実（検索語）は残る — Spec 05 の「検索した事実と、出典が
        // 返らない事実を区別して見せる」がここでも成立する。
        assert_eq!(decoded.grounding.queries, vec!["finance: DJI"]);
    }

    /// 壊れた arguments は raw を保持した Parse エラー（境界規律は他 adapter と同じ）。
    #[test]
    fn broken_function_arguments_fail_loudly_with_raw() {
        let raw = r#"{"output": [
            {"type": "function_call", "call_id": "c", "name": "f", "arguments": "{oops"}
        ]}"#;
        let err = decode(serde_json::from_str::<wire::ResponsesResponse>(raw).unwrap()).unwrap_err();
        assert!(matches!(err, LlmError::Parse { raw, .. } if raw == "{oops"));
    }

    /// 未知種別は応答全体を落とさず、数えられて捨てられる（#72）。
    #[test]
    fn unknown_items_are_counted_not_fatal() {
        let raw = r#"{
            "status": "completed",
            "output": [
                {"type": "image_generation_call"},
                {"type": "message", "content": [{"type": "output_text", "text": "本文", "annotations": []}]}
            ]
        }"#;
        let decoded = decode(serde_json::from_str::<wire::ResponsesResponse>(raw).unwrap()).unwrap();
        assert_eq!(decoded.text.as_deref(), Some("本文"));
        assert_eq!(decoded.finish, Finish::Stop);
    }

    /// 打ち切りは incomplete_details.reason で判定し、status 不明は Stop に倒さない。
    #[test]
    fn incomplete_maps_to_length_and_missing_status_is_other() {
        let cut = r#"{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":[]}"#;
        assert_eq!(
            decode(serde_json::from_str::<wire::ResponsesResponse>(cut).unwrap())
                .unwrap()
                .finish,
            Finish::Length
        );
        let unknown = r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"x","annotations":[]}]}]}"#;
        assert_eq!(
            decode(serde_json::from_str::<wire::ResponsesResponse>(unknown).unwrap())
                .unwrap()
                .finish,
            Finish::Other
        );
    }
}
