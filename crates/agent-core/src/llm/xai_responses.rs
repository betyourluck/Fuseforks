//! xAI Responses（`/v1/responses`）の encode / decode（Spec 31）。
//!
//! Grok の Live Search（Agent Tools の `web_search` / `x_search`）は
//! `/chat/completions` に存在しない。legacy の `search_parameters` は
//! HTTP 410 Gone で、サーバー自身が Agent Tools API への移行を名指しして断る
//! （実測 2026-08-09）。検索を使うときだけこの経路が要る。
//! 関数呼び出しだけなら互換層で足りる（既存エージェントはそのまま）—
//! `gemini` と同じ構図の 2 例目。
//!
//! 凍結は `data_contract.yaml` の `xai_responses` が正。ここに写している要点:
//! - `include: ["no_inline_citations"]` を常送（本文の無汚染と出典の網羅が同じ 1 手）
//! - 検索呼び出しは `web_search_call` / `x_search_call` / `custom_tool_call` の
//!   3 名を同じ腕で受け、どの名で来たかを計器に残す
//! - `reasoning` は本文へ混ぜず数えて捨てる（受け取りは thinking の Spec で）
//! - 出典は annotations の 1 系統。`action.sources` は読まない

use super::canonical::{
    ChatRequest, ChatResponse, Finish, Grounding, GroundingEngine, GroundingSource, Role,
    ToolCall, ToolChoice, Usage,
};
use super::error::LlmError;
use super::wire;
use serde_json::Value;

/// canonical → xAI Responses wire。
///
/// `use_tools` が偽、または `tool_choice` が [`ToolChoice::None`] のときは
/// **関数ツールも検索ツールも送らない**（契約 — 「ツールを使わせない」は
/// server-side tool にも及ぶ。summarize の呼び出しで検索が走ると、押した人が
/// 検索の注入 input まで払う）。
pub fn encode(
    req: &ChatRequest,
    use_tools: bool,
    web_search: bool,
    x_search: bool,
) -> wire::XaiRequest {
    let mut input = Vec::new();
    for message in &req.messages {
        match message.role {
            Role::System => input.push(wire::ResponsesInputItem::Message {
                role: "system",
                content: message.content.clone(),
            }),
            // 添付画像はこのワイヤでは送らない（Spec 23 D8 — 画像は互換経路のみ。
            // gemini ネイティブが素通しする挙動をテストで凍結したのと同じ棚）。
            Role::User => input.push(wire::ResponsesInputItem::Message {
                role: "user",
                content: message.content.clone(),
            }),
            Role::Assistant => {
                if !message.content.is_empty() {
                    input.push(wire::ResponsesInputItem::Message {
                        role: "assistant",
                        content: message.content.clone(),
                    });
                }
                for call in &message.tool_calls {
                    input.push(wire::ResponsesInputItem::FunctionCall {
                        kind: "function_call",
                        call_id: call.id.clone(),
                        name: call.name.clone(),
                        // canonical の args はオブジェクト。ワイヤの方言
                        // 「arguments は JSON 文字列」へは encode 境界で 1 回だけ戻す。
                        arguments: call.args.to_string(),
                    });
                }
            }
            Role::Tool => input.push(wire::ResponsesInputItem::FunctionCallOutput {
                kind: "function_call_output",
                call_id: message.tool_call_id.clone().unwrap_or_default(),
                output: message.content.clone(),
            }),
        }
    }

    let offer_tools = use_tools && req.tool_choice != ToolChoice::None;
    let mut tools = Vec::new();
    if offer_tools {
        if web_search {
            tools.push(wire::ResponsesTool::Server { kind: "web_search" });
        }
        if x_search {
            tools.push(wire::ResponsesTool::Server { kind: "x_search" });
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
        ToolChoice::Specific(name) => {
            Some(serde_json::json!({ "type": "function", "name": name }))
        }
    };

    wire::XaiRequest {
        model: req.model.clone(),
        input,
        tools,
        include: vec!["no_inline_citations"],
        max_output_tokens: req.max_tokens,
        temperature: req.temperature,
        tool_choice,
    }
}

/// 検索呼び出しとして受ける output 種別（契約 — 3 名を同じ腕で受ける）。
///
/// 公式文書は `x_search_call`、実ワイヤの観測は `custom_tool_call`
/// （移行途中の未文書挙動と読む）。どの名で来たかは計器に残す。
const SEARCH_CALL_KINDS: [&str; 3] = ["web_search_call", "x_search_call", "custom_tool_call"];

/// 検索呼び出しの item から検索語を取る。**2 つの形がある**（どちらも実測）:
///
/// - `web_search_call`: `action.query` に素の文字列
/// - `custom_tool_call`（x_search の実体）: **`input` に JSON 文字列**で
///   `{"query": "...", "limit": "5"}` が入る。`action` は無い
///
/// 片方だけを読むと、もう片方の経路で `queries` が**黙って空になる**
/// （実機の 2 走行がどちらも `queries=0` だった）。検索語は「何を調べたか」を
/// 人へ見せる唯一の材料なので、空は情報の欠落として現れる。
fn search_query(item: &wire::ResponsesOutputItem) -> Option<String> {
    if let Some(query) = item.action.as_ref().and_then(|a| a.query.clone()) {
        return Some(query);
    }
    let raw = item.input.as_ref()?;
    let parsed: Value = serde_json::from_str(raw).ok()?;
    // 形が違えば黙って諦める — 検索語が取れないことは失敗ではない
    // （応答そのものは正しく届いている）。
    Some(parsed.get("query")?.as_str()?.to_owned())
}

/// xAI Responses wire → canonical。
///
/// `arguments` は JSON 文字列なのでここで 1 回だけ parse して以後はオブジェクトとして
/// 運ぶ（`openai_compat::decode` と同じ境界規律）。壊れた arguments は raw を保持した
/// [`LlmError::Parse`] にする。
pub fn decode(resp: wire::ResponsesResponse) -> Result<ChatResponse, LlmError> {
    let mut texts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    let mut grounding = Grounding {
        engine: GroundingEngine::Xai,
        ..Grounding::default()
    };
    let mut search_counts: Vec<(String, u32)> = Vec::new();
    let mut reasoning_count = 0u32;
    let mut reasoning_summary: Vec<String> = Vec::new();
    let mut dropped: Vec<String> = Vec::new();

    for item in resp.output {
        match item.kind.as_str() {
            "message" => {
                for part in item.content.unwrap_or_default() {
                    if let Some(text) = part.text {
                        if !text.is_empty() {
                            texts.push(text);
                        }
                    }
                    for ann in part.annotations {
                        if ann.kind != "url_citation" {
                            continue;
                        }
                        let Some(url) = ann.url else { continue };
                        if grounding.sources.iter().any(|s| s.uri == url) {
                            continue;
                        }
                        // title が URL の複製で返ることがある（実測）。表題としては
                        // 空と同じなので空文字へ落とす（表示側が URL を二重に出さない）。
                        let title = ann.title.filter(|t| t != &url).unwrap_or_default();
                        grounding.sources.push(GroundingSource { uri: url, title });
                    }
                }
            }
            "reasoning" => {
                reasoning_count += 1;
                // 要約は既定で入る（Spec 33）。**空文字は落とす** — 0 字は
                // 表示するものが無いという意味で、列へ入れても枠が増えるだけ。
                // 短形（129 字の再掲）は落とさない：長さで切ると probe の
                // 産物が表示規則になる（`failures.md` #92 と同じ罠）。
                for part in item.summary.iter().flatten() {
                    if part.kind != "summary_text" {
                        continue;
                    }
                    let Some(text) = part.text.as_ref() else {
                        continue;
                    };
                    if !text.is_empty() {
                        reasoning_summary.push(text.clone());
                    }
                }
            }
            kind if SEARCH_CALL_KINDS.contains(&kind) => {
                if let Some(query) = search_query(&item)
                    && !grounding.queries.contains(&query)
                {
                    grounding.queries.push(query);
                }
                match search_counts.iter_mut().find(|(name, _)| name == kind) {
                    Some((_, count)) => *count += 1,
                    None => search_counts.push((kind.to_owned(), 1)),
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
            // `output_tokens` の内数。足さない（Spec 32 P0）。
            reasoning: u
                .output_tokens_details
                .as_ref()
                .map(|d| d.reasoning_tokens)
                .unwrap_or_default(),
        })
        .unwrap_or_default();

    // 未知種別と reasoning は数えてから捨てる（#72 — 受け皿が底なしだと、
    // そこへ落ちたものは存在しなかったことになる）。
    if reasoning_count > 0 || !dropped.is_empty() {
        crate::note!(
            "dropped content blocks: kinds={} count={} output_tokens={} text_chars={} tool_calls={}",
            {
                let mut kinds: Vec<String> = Vec::new();
                if reasoning_count > 0 {
                    kinds.push(format!("reasoning:{reasoning_count}"));
                }
                kinds.extend(dropped.iter().cloned());
                kinds.join("+")
            },
            reasoning_count as usize + dropped.len(),
            usage.completion,
            texts.iter().map(|t| t.chars().count()).sum::<usize>(),
            tool_calls.len(),
        );
    }

    // 検索の計器。検索を使った呼び出しのときだけ 1 行（`grep include:` と同じ形）。
    // 正の値は呼び出し回数、ticks は補助の生値のまま — 換算しない（契約）。
    if !search_counts.is_empty() {
        let names = search_counts
            .iter()
            .map(|(name, count)| format!("{name}:{count}"))
            .collect::<Vec<_>>()
            .join(",");
        let ticks = resp
            .usage
            .as_ref()
            .and_then(|u| u.cost_in_usd_ticks)
            .map(|t| t.to_string())
            .unwrap_or_else(|| "-".to_owned());
        crate::note!(
            "xai search: calls={} names={} sources={} queries={} ticks={}",
            search_counts.iter().map(|(_, c)| c).sum::<u32>(),
            names,
            grounding.sources.len(),
            grounding.queries.len(),
            ticks,
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

/// 履歴の assistant メッセージが空本文 + tool_calls のとき、encode が
/// input へ何を出すかの補助（テストの読みやすさのため公開はしない）。
#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::canonical::{ChatMessage, ToolSpec};

    fn spec(name: &str) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: "テスト用".into(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }
    }

    fn base_request() -> ChatRequest {
        ChatRequest {
            model: "grok-4.5".into(),
            messages: vec![
                ChatMessage::system("あなたは検証用の応答者です"),
                ChatMessage::user("AAPL の現在価格を教えて"),
            ],
            tools: vec![spec("get_price")],
            tool_choice: ToolChoice::Auto,
            temperature: None,
            max_tokens: 1024,
            effort: None,
            cacheable_prefix_len: 0,
        }
    }

    /// golden: リクエスト全体の形を文字列で固定する。
    /// include の常送・tools の同居・temperature 省略が 1 本で読める。
    #[test]
    fn encode_golden_includes_no_inline_citations_and_flat_tools() {
        let req = base_request();
        let body = encode(&req, true, true, false);
        let json = serde_json::to_string(&body).unwrap();
        assert_eq!(
            json,
            r#"{"model":"grok-4.5","input":[{"role":"system","content":"あなたは検証用の応答者です"},{"role":"user","content":"AAPL の現在価格を教えて"}],"tools":[{"type":"web_search"},{"type":"function","name":"get_price","description":"テスト用","parameters":{"type":"object","properties":{}}}],"include":["no_inline_citations"],"max_output_tokens":1024}"#
        );
    }

    /// ToolChoice::None では検索ツールも関数ツールも送らない（契約）。
    /// summarize の呼び出しで検索が走ると、押した人が注入 input まで払う。
    #[test]
    fn tool_choice_none_sends_no_tools_at_all() {
        let mut req = base_request();
        req.tool_choice = ToolChoice::None;
        let body = encode(&req, true, true, true);
        assert!(body.tools.is_empty());
        let json = serde_json::to_string(&body).unwrap();
        assert!(!json.contains("web_search"), "検索ツールが漏れている: {json}");
    }

    /// use_tools = false も同じ（互換サーバ向けフォールバックの規律を写す）。
    #[test]
    fn use_tools_false_sends_no_tools() {
        let body = encode(&base_request(), false, true, true);
        assert!(body.tools.is_empty());
    }

    /// トグルが偽なら対応するサーバー側ツールが tools に入らない。
    /// 両方偽 + 関数ツールありでは関数だけが並ぶ。
    #[test]
    fn server_tools_follow_their_toggles() {
        let body = encode(&base_request(), true, false, true);
        let json = serde_json::to_string(&body).unwrap();
        assert!(!json.contains("web_search"));
        assert!(json.contains("x_search"));

        let body = encode(&base_request(), true, false, false);
        let json = serde_json::to_string(&body).unwrap();
        assert!(!json.contains("_search"));
        assert!(json.contains(r#""type":"function""#));
    }

    /// 関数往復の再送形。assistant の tool_calls → function_call item、
    /// Role::Tool → function_call_output item。args はオブジェクト → JSON 文字列へ。
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

        let body = encode(&req, true, false, false);
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains(
            r#"{"type":"function_call","call_id":"call-1","name":"get_price","arguments":"{\"symbol\":\"AAPL\"}"}"#
        ));
        assert!(json.contains(
            r#"{"type":"function_call_output","call_id":"call-1","output":"{\"price\": 231.5}"}"#
        ));
    }

    /// probe の実応答から要点を写した decode golden。
    /// 複数 message の連結・annotations → sources・usage の対応が 1 本で読める。
    #[test]
    fn decode_collects_messages_annotations_and_usage() {
        let raw = r#"{
            "status": "completed",
            "output": [
                {"type": "reasoning"},
                {"type": "web_search_call", "status": "completed",
                 "action": {"type": "search", "query": "xAI latest news"}},
                {"type": "message", "role": "assistant", "content": [
                    {"type": "output_text", "text": "最新の記事を確認中です。", "annotations": []}
                ]},
                {"type": "message", "role": "assistant", "content": [
                    {"type": "output_text", "text": "xAI が Grok 4.5 を発表しました。",
                     "annotations": [
                        {"type": "url_citation", "url": "https://x.ai/news",
                         "title": "https://x.ai/news", "start_index": 0, "end_index": 0},
                        {"type": "url_citation", "url": "https://techcrunch.com/a",
                         "title": "TechCrunch", "start_index": 0, "end_index": 0}
                     ]}
                ]}
            ],
            "usage": {
                "input_tokens": 98213,
                "output_tokens": 1415,
                "input_tokens_details": {"cached_tokens": 62720},
                "cost_in_usd_ticks": 1582920000,
                "server_side_tool_usage_details": {"web_search_calls": 1}
            }
        }"#;
        let resp: wire::ResponsesResponse = serde_json::from_str(raw).unwrap();
        let decoded = decode(resp).unwrap();

        assert_eq!(
            decoded.text.as_deref(),
            Some("最新の記事を確認中です。\n\nxAI が Grok 4.5 を発表しました。")
        );
        assert_eq!(decoded.finish, Finish::Stop);
        assert_eq!(decoded.usage.prompt, 98213);
        assert_eq!(decoded.usage.completion, 1415);
        assert_eq!(decoded.usage.cache_read, 62720);
        assert_eq!(decoded.grounding.engine, GroundingEngine::Xai);
        assert_eq!(decoded.grounding.queries, vec!["xAI latest news"]);
        assert_eq!(decoded.grounding.sources.len(), 2);
        // title が URL の複製なら空へ落ちる。実表題はそのまま残る。
        assert_eq!(decoded.grounding.sources[0].title, "");
        assert_eq!(decoded.grounding.sources[1].title, "TechCrunch");
    }

    /// x_search の実体（`custom_tool_call`）は `action` を持たず、
    /// **`input` の JSON 文字列**に検索語が入る。片方だけを読むと、
    /// もう片方の経路で `queries` が黙って空になる（実機の 2 走行が
    /// どちらも `queries=0` だった）。
    #[test]
    fn custom_tool_call_carries_its_query_in_the_input_json() {
        let raw = r#"{
            "status": "completed",
            "output": [
                {"type": "custom_tool_call", "name": "x_semantic_search",
                 "call_id": "xs_call-1",
                 "input": "{\"query\":\"AI latest news\",\"limit\":\"5\"}"},
                {"type": "message", "content": [{"type": "output_text", "text": "ok", "annotations": []}]}
            ]
        }"#;
        let resp: wire::ResponsesResponse = serde_json::from_str(raw).unwrap();
        let decoded = decode(resp).unwrap();
        assert_eq!(decoded.grounding.queries, vec!["AI latest news"]);
        // 検索呼び出しとして数えられており、ツール呼び出しへは漏れていない。
        assert!(decoded.tool_calls.is_empty());
    }

    /// `input` が JSON でない・`query` を持たない形でも落ちない。
    /// 検索語が取れないことは失敗ではない（応答そのものは届いている）。
    #[test]
    fn unreadable_search_input_yields_no_query_but_still_decodes() {
        let raw = r#"{
            "status": "completed",
            "output": [
                {"type": "custom_tool_call", "input": "not json"},
                {"type": "custom_tool_call", "input": "{\"limit\":\"5\"}"},
                {"type": "message", "content": [{"type": "output_text", "text": "ok", "annotations": []}]}
            ]
        }"#;
        let resp: wire::ResponsesResponse = serde_json::from_str(raw).unwrap();
        let decoded = decode(resp).unwrap();
        assert!(decoded.grounding.queries.is_empty());
        assert_eq!(decoded.text.as_deref(), Some("ok"));
    }

    /// 検索呼び出し 3 名を同じ腕で受ける（契約）。custom_tool_call は実ワイヤの
    /// 観測名、x_search_call は公式文書名 — どちらが来ても検索として数える。
    #[test]
    fn all_three_search_call_kinds_are_accepted() {
        let raw = r#"{
            "status": "completed",
            "output": [
                {"type": "web_search_call", "action": {"type": "search", "query": "a"}},
                {"type": "x_search_call", "action": {"type": "search", "query": "b"}},
                {"type": "custom_tool_call", "action": {"type": "search", "query": "c"}},
                {"type": "message", "content": [{"type": "output_text", "text": "ok", "annotations": []}]}
            ]
        }"#;
        let resp: wire::ResponsesResponse = serde_json::from_str(raw).unwrap();
        let decoded = decode(resp).unwrap();
        assert_eq!(decoded.grounding.queries, vec!["a", "b", "c"]);
        assert_eq!(decoded.text.as_deref(), Some("ok"));
    }

    /// function_call は ToolCall へ。arguments は境界で 1 回だけ parse される。
    #[test]
    fn function_call_decodes_to_tool_call_with_parsed_args() {
        let raw = r#"{
            "status": "completed",
            "output": [
                {"type": "function_call", "call_id": "call-9", "name": "get_price",
                 "arguments": "{\"symbol\":\"AAPL\"}", "status": "completed"}
            ]
        }"#;
        let resp: wire::ResponsesResponse = serde_json::from_str(raw).unwrap();
        let decoded = decode(resp).unwrap();
        assert_eq!(decoded.finish, Finish::ToolUse);
        assert_eq!(decoded.tool_calls.len(), 1);
        assert_eq!(decoded.tool_calls[0].id, "call-9");
        assert_eq!(
            decoded.tool_calls[0].args,
            serde_json::json!({"symbol": "AAPL"})
        );
    }

    /// 壊れた arguments は raw を保持した Parse エラー（openai_compat と同じ境界規律）。
    #[test]
    fn broken_function_arguments_fail_loudly_with_raw() {
        let raw = r#"{
            "output": [
                {"type": "function_call", "call_id": "c", "name": "f", "arguments": "{oops"}
            ]
        }"#;
        let resp: wire::ResponsesResponse = serde_json::from_str(raw).unwrap();
        let err = decode(resp).unwrap_err();
        assert!(matches!(err, LlmError::Parse { raw, .. } if raw == "{oops"));
    }

    /// 未知の output 種別は応答全体を落とさず、数えられて捨てられる（#72）。
    /// reasoning も同じ経路（本文へ混ぜない）。
    #[test]
    fn unknown_and_reasoning_items_are_counted_not_fatal() {
        let raw = r#"{
            "status": "completed",
            "output": [
                {"type": "reasoning"},
                {"type": "some_future_item", "x": 1},
                {"type": "message", "content": [{"type": "output_text", "text": "本文", "annotations": []}]}
            ]
        }"#;
        let resp: wire::ResponsesResponse = serde_json::from_str(raw).unwrap();
        let decoded = decode(resp).unwrap();
        assert_eq!(decoded.text.as_deref(), Some("本文"));
        assert_eq!(decoded.finish, Finish::Stop);
    }

    /// 思考トークンは `output_tokens_details.reasoning_tokens` にしか出ない。
    /// **`output_tokens` の内数**なので足さない（Spec 32 D2）。
    ///
    /// 数字は 2026-08-10 の probe の実測形（output 1,497 / reasoning 1,494 /
    /// 差 3 = 本文のトークン数）。
    #[test]
    fn reasoning_tokens_are_read_as_a_share_of_output() {
        let raw = r#"{
            "status": "completed",
            "output": [
                {"type": "reasoning"},
                {"type": "message", "content": [{"type": "output_text", "text": "Bです", "annotations": []}]}
            ],
            "usage": {
                "input_tokens": 801,
                "input_tokens_details": {"cached_tokens": 768},
                "output_tokens": 1497,
                "output_tokens_details": {"reasoning_tokens": 1494}
            }
        }"#;
        let decoded = decode(serde_json::from_str::<wire::ResponsesResponse>(raw).unwrap()).unwrap();

        assert_eq!(decoded.usage.completion, 1_497);
        assert_eq!(decoded.usage.reasoning, 1_494);
        // 内数なので合計は prompt + completion のまま。外数で実装すると 3,792 になる。
        assert_eq!(decoded.usage.total(), 2_298);
    }

    /// `output_tokens_details` を持たない応答（検索なしの短いターン・互換の
    /// 中継サーバ）でも落ちない。**欄が無いことは 0 であって失敗ではない。**
    #[test]
    fn missing_output_token_details_reads_as_zero_reasoning() {
        let raw = r#"{
            "status": "completed",
            "output": [{"type": "message", "content": [{"type": "output_text", "text": "はい", "annotations": []}]}],
            "usage": {"input_tokens": 10, "output_tokens": 2}
        }"#;
        let decoded = decode(serde_json::from_str::<wire::ResponsesResponse>(raw).unwrap()).unwrap();

        assert_eq!(decoded.usage.reasoning, 0);
        assert_eq!(decoded.usage.completion, 2);
    }

    /// 思考の要約は `reasoning` item の `summary[].text` から取る（Spec 33）。
    /// **既定で入る**ので送信側の変更は要らない。
    #[test]
    fn reasoning_summaries_are_captured_from_the_item() {
        let raw = r#"{
            "status": "completed",
            "output": [
                {"type": "reasoning", "summary": [
                    {"type": "summary_text", "text": "The problem is a logic puzzle…"}
                ]},
                {"type": "message", "content": [{"type": "output_text", "text": "Bです", "annotations": []}]}
            ]
        }"#;
        let decoded = decode(serde_json::from_str::<wire::ResponsesResponse>(raw).unwrap()).unwrap();

        assert_eq!(
            decoded.reasoning_summary,
            vec!["The problem is a logic puzzle…"]
        );
        assert_eq!(decoded.text.as_deref(), Some("Bです"));
    }

    /// **空文字は入れない**（0 字は表示するものが無い）。一方
    /// **短形は落とさない** — 129 字と 919 字の間に線を引くと、probe の産物が
    /// 表示規則になる（`failures.md` #92 と同じ罠。`reasoning_summary` 契約の凍結 2）。
    ///
    /// 短形の実物は「問いの再掲だけ」で内容は薄いが、**薄いことは利用者が読めば
    /// 分かる**。ハーネスが中身の質を判定しない。
    #[test]
    fn empty_summaries_are_dropped_but_short_ones_are_kept() {
        let raw = r#"{
            "status": "completed",
            "output": [
                {"type": "reasoning", "summary": [
                    {"type": "summary_text", "text": ""},
                    {"type": "summary_text", "text": "The problem is in Japanese: 3 人の…"},
                    {"type": "some_future_part", "text": "読まない種別"}
                ]},
                {"type": "reasoning"},
                {"type": "message", "content": [{"type": "output_text", "text": "答え", "annotations": []}]}
            ]
        }"#;
        let decoded = decode(serde_json::from_str::<wire::ResponsesResponse>(raw).unwrap()).unwrap();

        assert_eq!(
            decoded.reasoning_summary,
            vec!["The problem is in Japanese: 3 人の…"],
            "空は落ち、短形は残り、未知の片は読まない。summary を持たない item も落ちない"
        );
    }

    /// 打ち切りは incomplete_details.reason で判定する。
    #[test]
    fn incomplete_with_max_output_tokens_maps_to_length() {
        let raw = r#"{
            "status": "incomplete",
            "incomplete_details": {"reason": "max_output_tokens"},
            "output": []
        }"#;
        let resp: wire::ResponsesResponse = serde_json::from_str(raw).unwrap();
        let decoded = decode(resp).unwrap();
        assert_eq!(decoded.finish, Finish::Length);
    }

    /// status が読めない応答を「正常終了」に倒さない（Finish::Other の既定と同じ向き）。
    #[test]
    fn missing_status_is_other_not_stop() {
        let raw = r#"{"output": [{"type": "message", "content": [{"type": "output_text", "text": "x", "annotations": []}]}]}"#;
        let resp: wire::ResponsesResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(decode(resp).unwrap().finish, Finish::Other);
    }
}
