//! OpenAI 互換 adapter（OpenAI / Grok / ローカル互換サーバ）。
//!
//! canonical ⇄ wire の **encode / decode 純関数**のみを持つ。
//! HTTP・認証・再試行は [`super::client`] が担当し、ここは形の翻訳だけを持つ。
//! 壊れるのは常に ser/de なので、この層をテストで固めることに意味がある。
//!
//! 由来: Kataribe `crates/llm_client/src/openai_compat.rs`。

use serde_json::Value;

use super::canonical::{
    ChatMessage, ChatRequest, ChatResponse, Effort, Finish, Grounding, Role, ToolCall, ToolChoice,
    Usage,
};
use super::error::LlmError;
use super::wire;

/// canonical → OpenAI 互換 wire。
///
/// `use_tools == false` のときは tools を送らず、スキーマを載せた指示文を
/// system メッセージとして末尾に積む。`tool_choice` を実装していない互換サーバ
/// （ローカル推論サーバなど）でも構造化出力を得るためのフォールバック経路。
pub fn encode(req: &ChatRequest, use_tools: bool) -> wire::OaiRequest {
    let mut messages: Vec<wire::OaiMessage> = req.messages.iter().map(encode_message).collect();

    let (tools, tool_choice) = if req.tools.is_empty() {
        (Vec::new(), None)
    } else if use_tools {
        let tools = req
            .tools
            .iter()
            .map(|t| wire::OaiTool {
                kind: wire::OaiToolKind::Function,
                function: wire::OaiFunctionDef {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.parameters.clone(),
                },
            })
            .collect();

        let choice = match &req.tool_choice {
            // 明示の "none"。欠落は既定 auto なので「定義は見せるが使わせない」に
            // ならない（Anthropic 側と同じ理由。まとめ呼び出しが使う形）。
            ToolChoice::None => Some(wire::OaiToolChoice::Mode("none")),
            ToolChoice::Auto => Some(wire::OaiToolChoice::Mode("auto")),
            ToolChoice::Required => Some(wire::OaiToolChoice::Mode("required")),
            ToolChoice::Specific(name) => Some(wire::OaiToolChoice::Function {
                kind: wire::OaiToolKind::Function,
                function: wire::OaiToolChoiceFunction { name: name.clone() },
            }),
        };
        (tools, choice)
    } else {
        messages.push(wire::OaiMessage::text(
            wire::OaiRole::System,
            json_instruction(&req.tools[0].parameters),
        ));
        (Vec::new(), None)
    };

    // 出力上限は常に片方の欄だけで送る（欄名の選択は uses_max_completion_tokens）。
    let (max_tokens, max_completion_tokens) = if uses_max_completion_tokens(&req.model) {
        (None, Some(req.max_tokens))
    } else {
        (Some(req.max_tokens), None)
    };

    wire::OaiRequest {
        model: req.model.clone(),
        messages,
        temperature: req.temperature,
        max_tokens,
        max_completion_tokens,
        tools,
        tool_choice,
        reasoning_effort: reasoning_effort(&req.model, req.effort),
    }
}

/// OpenAI の o 系推論モデルか（o1 / o3 / o4-mini ...）。
fn is_o_series(model: &str) -> bool {
    model.starts_with('o') && model.chars().nth(1).is_some_and(|c| c.is_ascii_digit())
}

/// 出力上限をどちらの欄名で送るかを決める（純関数）。
///
/// gpt-5 系と o 系は思考トークンを含めて上限を管理する方式へ移っており、
/// 旧欄 `max_tokens` を送ると 400 `unsupported_parameter` で拒否する
/// （2026-08-06 実機、gpt-5.6-luna）。一方で互換サーバ（llama.cpp / vLLM /
/// gpt-oss 系）には新欄 `max_completion_tokens` を知らないものがあるため、
/// 全面的な置き換えはできない。`reasoning_effort` と同じくモデル名で送り分ける。
pub fn uses_max_completion_tokens(model: &str) -> bool {
    model.starts_with("gpt-5") || is_o_series(model)
}

/// canonical の 1 発話を OpenAI 互換の形へ写す。
///
/// ツール往復では 3 通りに分かれる:
/// - ツール結果 → `role: "tool"` + `tool_call_id`
/// - ツールを呼んだ assistant → `tool_calls` を持ち、`content` は空なら省く
/// - それ以外 → 本文だけ
fn encode_message(message: &ChatMessage) -> wire::OaiMessage {
    let role = match message.role {
        Role::System => wire::OaiRole::System,
        Role::User => wire::OaiRole::User,
        Role::Assistant => wire::OaiRole::Assistant,
        Role::Tool => wire::OaiRole::Tool,
    };

    if message.role == Role::Tool {
        return wire::OaiMessage {
            role,
            content: Some(message.content.clone()),
            tool_calls: Vec::new(),
            tool_call_id: message.tool_call_id.clone(),
        };
    }

    wire::OaiMessage {
        role,
        // 本文が空のまま送ると `content` が必須のサーバで 400 になる。省くほうが安全。
        content: (!message.content.is_empty()).then(|| message.content.clone()),
        tool_calls: message
            .tool_calls
            .iter()
            .map(|call| wire::OaiRequestToolCall {
                id: call.id.clone(),
                kind: wire::OaiToolKind::Function,
                function: wire::OaiRequestFunctionCall {
                    name: call.name.clone(),
                    // 受け取ったときと同じ「JSON 文字列」へ戻す。
                    arguments: serde_json::to_string(&call.args).unwrap_or_else(|_| "{}".into()),
                },
                // Gemini の思考署名など。受けた値をそのまま返す（欠くと 400）。
                extra_content: call.extra.clone(),
            })
            .collect(),
        tool_call_id: None,
    }
}

/// `reasoning_effort` を送るかどうかと、その値を決める（純関数）。
///
/// 推論制御を持つモデルにだけ送る。他モデルへ送るとキーを解釈できず 400 になる。
/// canonical の 5 段階のうちプロバイダが受け付けない値は、ここで丸める。
///
/// 推論モデルに対して明示しない場合、プロバイダ側の既定（常時深い思考）が適用され、
/// 思考が出力予算を食い潰して空応答になることがある。既定を持つのは adapter の責務。
pub fn reasoning_effort(model: &str, effort: Option<Effort>) -> Option<&'static str> {
    let is_grok_reasoning = model.starts_with("grok-4.3") || model.starts_with("grok-4.5");

    if !is_grok_reasoning && !is_o_series(model) {
        return None;
    }

    Some(match effort {
        // 未指定時の既定。grok-4.3 のみ `none`（思考の完全停止）を許す。
        None => {
            if model.starts_with("grok-4.3") {
                "none"
            } else {
                "low"
            }
        }
        Some(Effort::Low) => "low",
        Some(Effort::Medium) => "medium",
        // xhigh / max を受け付けないプロバイダ向けに high へ丸める。
        Some(Effort::High) | Some(Effort::XHigh) | Some(Effort::Max) => "high",
    })
}

/// OpenAI 互換 wire → canonical。
///
/// `tool_calls[].function.arguments` は **JSON 文字列**なので、ここで 1 回だけ parse して
/// 以後はオブジェクトとして運ぶ。二重エンコードと未パースの取り違えを境界で断つ。
/// 壊れた arguments は **raw を保持した** [`LlmError::Parse`] にする（再生成の燃料）。
pub fn decode(resp: wire::OaiResponse) -> Result<ChatResponse, LlmError> {
    let usage = resp
        .usage
        .as_ref()
        .map(|u| Usage {
            prompt: u.prompt_tokens,
            completion: u.completion_tokens,
            cache_read: u
                .prompt_tokens_details
                .as_ref()
                .map_or(0, |d| d.cached_tokens),
        })
        .unwrap_or_default();

    let Some(choice) = resp.choices.into_iter().next() else {
        return Ok(ChatResponse {
            text: None,
            tool_calls: Vec::new(),
            finish: Finish::Other,
            usage,
            // このプロバイダは接地を代行しない。
            grounding: Grounding::default(),
        });
    };

    let finish = match choice.finish_reason.as_deref() {
        Some("stop") => Finish::Stop,
        Some("tool_calls") => Finish::ToolUse,
        Some("length") => Finish::Length,
        _ => Finish::Other,
    };

    let mut tool_calls = Vec::with_capacity(choice.message.tool_calls.len());
    for call in choice.message.tool_calls {
        let raw = call.function.arguments;
        let args: Value = serde_json::from_str(&raw).map_err(|source| LlmError::Parse {
            source,
            raw: raw.clone(),
        })?;
        tool_calls.push(ToolCall {
            id: call.id.unwrap_or_default(),
            name: call.function.name.unwrap_or_default(),
            args,
            extra: call.extra_content,
        });
    }

    Ok(ChatResponse {
        text: choice.message.content,
        tool_calls,
        finish,
        usage,
        // このプロバイダは接地を代行しない。
        grounding: Grounding::default(),
    })
}

/// 空応答の防御（純関数）。
///
/// **本文が空 かつ tool_calls が空 かつ `finish == Length`** のときだけ
/// [`LlmError::OutputTruncated`] にする。出力予算を使い切って何も成立しなかった
/// 状態で、原因は 2 通りある — 推論モデルが思考で使い切った場合と、生成物
/// （長い本文・大きなツール引数）が上限で切れた場合。**どちらも同じ入力では
/// 同じ所で切れる**ので、再試行には乗せず理由を返す（`is_transient` が偽）。
///
/// 以前は [`LlmError::EmptyResponse`] へ畳んで一過性としていたが、実機で
/// 2 回連続の同一失敗を観測して改めた（2026-07-31。failures.md #40）。
/// `limit` を載せるのは、利用者の次の一手が「いくつまで上げるか」だから —
/// 現在値が分からないと上げ幅を決められない。
///
/// `Length` 以外の空応答はここでは弾かない。通常の空応答は再送しても同じ結果になるため、
/// 呼び出し側へそのまま返して原因を見せるほうが正しい。
pub fn reject_empty_reasoning(resp: ChatResponse, limit: u32) -> Result<ChatResponse, LlmError> {
    let text_empty = resp.text.as_deref().is_none_or(|t| t.trim().is_empty());
    if text_empty && resp.tool_calls.is_empty() && resp.finish == Finish::Length {
        return Err(LlmError::OutputTruncated { limit });
    }
    Ok(resp)
}

/// tools を送れないサーバ向けに、「スキーマに従う JSON だけを出せ」と指示する system 本文。
fn json_instruction(schema: &Value) -> String {
    format!(
        "重要: このサーバはツール呼び出し (function calling) に対応していません。\
         応答は次の JSON Schema に厳密に従う JSON オブジェクトを **1つだけ** 出力し、\
         前置き・説明・コードフェンスのラベル等、余計なテキストを一切含めないでください。\n\
         JSON Schema:\n{}",
        serde_json::to_string(schema).unwrap_or_default()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::canonical::{ChatMessage, ToolSpec};
    use serde_json::json;

    fn tool() -> ToolSpec {
        ToolSpec {
            name: "emit_plan".into(),
            description: "計画を出力する".into(),
            parameters: json!({ "type": "object", "properties": { "steps": { "type": "array" } } }),
        }
    }

    fn req_with_tool(choice: ToolChoice) -> ChatRequest {
        ChatRequest {
            model: "gpt-4o".into(),
            messages: vec![ChatMessage::user("計画して")],
            tools: vec![tool()],
            tool_choice: choice,
            temperature: None,
            max_tokens: 512,
            effort: None,
            cacheable_prefix_len: 0,
        }
    }

    /// ツール定義を残したまま使用だけを禁じる形（まとめ呼び出しのワイヤ契約）。
    /// 欠落（= 既定 auto）に写すと「定義は見せるが使わせない」が表現できない。
    #[test]
    fn encode_sends_explicit_none_with_tools_kept() {
        let wire = encode(&req_with_tool(ToolChoice::None), true);
        let json = serde_json::to_value(&wire).unwrap();

        assert_eq!(json["tool_choice"], "none");
        assert_eq!(json["tools"][0]["function"]["name"], "emit_plan", "定義は残ること");
    }

    #[test]
    fn encode_forces_specific_tool() {
        let wire = encode(&req_with_tool(ToolChoice::Specific("emit_plan".into())), true);
        let json = serde_json::to_value(&wire).unwrap();

        assert_eq!(json["tool_choice"]["type"], "function");
        assert_eq!(json["tool_choice"]["function"]["name"], "emit_plan");
        assert_eq!(json["tools"][0]["function"]["name"], "emit_plan");
    }

    #[test]
    fn encode_falls_back_to_prompt_instruction_without_tool_support() {
        let wire = encode(&req_with_tool(ToolChoice::Specific("emit_plan".into())), false);

        assert!(wire.tools.is_empty(), "tools は送らない");
        assert!(wire.tool_choice.is_none());
        let last = wire.messages.last().expect("指示文が積まれること");
        assert_eq!(last.role, wire::OaiRole::System);
        assert!(last.content.as_deref().unwrap_or_default().contains("JSON Schema"));
    }

    /// ツール往復の形。`arguments` は**受け取ったときと同じ JSON 文字列**へ戻す。
    ///
    /// オブジェクトのまま送り返すと、サーバによっては黙って無視されるか 400 になる。
    #[test]
    fn tool_round_trip_restores_arguments_as_a_json_string() {
        let calls = vec![ToolCall {
            id: "call_1".into(),
            name: "remember".into(),
            args: serde_json::json!({ "note": "覚えること" }),
            extra: None,
        }];

        let req = ChatRequest {
            messages: vec![
                ChatMessage::user("覚えておいて"),
                ChatMessage::assistant_tool_calls("", calls),
                ChatMessage::tool_result("call_1", "remember", "書き留めました。"),
            ],
            ..req_with_tool(ToolChoice::Auto)
        };

        let json = serde_json::to_value(encode(&req, true)).unwrap();
        let messages = json["messages"].as_array().unwrap();

        // 呼び出しを積んだ assistant。本文が空なら content ごと省く。
        assert_eq!(messages[1]["role"], "assistant");
        assert!(messages[1].get("content").is_none());
        assert_eq!(messages[1]["tool_calls"][0]["id"], "call_1");
        assert_eq!(messages[1]["tool_calls"][0]["type"], "function");
        assert_eq!(
            messages[1]["tool_calls"][0]["function"]["arguments"],
            r#"{"note":"覚えること"}"#,
            "arguments は JSON 文字列へ戻す"
        );

        // 結果は role: "tool" + tool_call_id。
        assert_eq!(messages[2]["role"], "tool");
        assert_eq!(messages[2]["tool_call_id"], "call_1");
        assert_eq!(messages[2]["content"], "書き留めました。");
    }

    /// Gemini (OpenAI 互換) の思考署名がツール往復で保存されること。
    ///
    /// Gemini 3 系はツール呼び出しごとに `extra_content.google.thought_signature` を返し、
    /// 履歴として assistant の tool_calls を再送するとき、**同じ位置に同じ値**を
    /// 返すことを要求する。欠くと 400 INVALID_ARGUMENT
    /// (`Function call is missing a thought_signature`) で、ツールを 1 回でも
    /// 呼んだ会話の 2 周目が必ず落ちる。実機のジェミー (remember 呼び出し直後) で表面化。
    /// 一次資料: https://ai.google.dev/gemini-api/docs/thought-signatures
    #[test]
    fn gemini_thought_signature_round_trips_through_tool_calls() {
        let raw = r#"{
            "choices": [{
                "message": {
                    "tool_calls": [{
                        "id": "function-call-587",
                        "type": "function",
                        "extra_content": { "google": { "thought_signature": "CvcQAdHN2g==" } },
                        "function": { "name": "remember", "arguments": "{\"note\":\"呼称ルール\"}" }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }"#;
        let resp = decode(serde_json::from_str(raw).unwrap()).unwrap();

        // オーケストレーターと同じ経路: 受けた呼び出しをそのまま履歴へ積んで再送する。
        let req = ChatRequest {
            messages: vec![
                ChatMessage::user("覚えておいて"),
                ChatMessage::assistant_tool_calls("", resp.tool_calls.clone()),
                ChatMessage::tool_result("function-call-587", "remember", "書き留めました。"),
            ],
            ..req_with_tool(ToolChoice::Auto)
        };
        let json = serde_json::to_value(encode(&req, true)).unwrap();

        assert_eq!(
            json["messages"][1]["tool_calls"][0]["extra_content"]["google"]["thought_signature"],
            "CvcQAdHN2g==",
            "署名は受け取った位置と同じ位置へそのまま戻す"
        );
    }

    /// 署名を返さないサーバ (OpenAI / Grok / ローカル互換) では `extra_content` を送らないこと。
    ///
    /// 無い所へ null や空オブジェクトを送ると、未知キーに厳格なサーバで壊れる。
    #[test]
    fn extra_content_is_omitted_when_the_server_did_not_send_one() {
        let calls = vec![ToolCall {
            id: "call_1".into(),
            name: "remember".into(),
            args: serde_json::json!({ "note": "覚えること" }),
            extra: None,
        }];
        let req = ChatRequest {
            messages: vec![ChatMessage::assistant_tool_calls("", calls)],
            ..req_with_tool(ToolChoice::Auto)
        };
        let json = serde_json::to_value(encode(&req, true)).unwrap();

        assert!(
            json["messages"][0]["tool_calls"][0].get("extra_content").is_none(),
            "署名の無い呼び出しに extra_content キーを生やさない"
        );
    }

    /// gpt-5 系には `max_completion_tokens`、それ以外には `max_tokens` を送る。
    ///
    /// 旧欄は gpt-5 系 / o 系が 400 (`unsupported_parameter`) で拒否し、
    /// 新欄は知らない互換サーバがあるので、ワイヤには**常に片方だけ**現れる。
    #[test]
    fn token_limit_field_splits_by_model_family() {
        let mut req = req_with_tool(ToolChoice::Auto);
        req.model = "gpt-5.6-luna".into();
        let json = serde_json::to_value(encode(&req, true)).unwrap();
        assert_eq!(json["max_completion_tokens"], 512);
        assert!(json.get("max_tokens").is_none(), "旧欄を送ると 400 unsupported_parameter");

        req.model = "gpt-oss-120b".into();
        let json = serde_json::to_value(encode(&req, true)).unwrap();
        assert_eq!(json["max_tokens"], 512);
        assert!(json.get("max_completion_tokens").is_none(), "新欄を知らない互換サーバが落ちる");
    }

    #[test]
    fn uses_max_completion_tokens_targets_only_openai_new_families() {
        assert!(uses_max_completion_tokens("gpt-5.6-terra"));
        assert!(uses_max_completion_tokens("gpt-5.6-luna"));
        assert!(uses_max_completion_tokens("o3-mini"));
        assert!(!uses_max_completion_tokens("gpt-4o"));
        assert!(!uses_max_completion_tokens("grok-4.3"));
        assert!(!uses_max_completion_tokens("gpt-oss-120b"));
        assert!(!uses_max_completion_tokens("gemini-3.5-flash-lite"));
    }

    #[test]
    fn reasoning_effort_targets_only_reasoning_models() {
        assert_eq!(reasoning_effort("gpt-4o", Some(Effort::High)), None);
        assert_eq!(reasoning_effort("grok-4.3-mini", None), Some("none"));
        assert_eq!(reasoning_effort("grok-4.5", None), Some("low"));
        // 未対応の段階は high へ丸める。
        assert_eq!(reasoning_effort("grok-4.5", Some(Effort::Max)), Some("high"));
    }

    #[test]
    fn decode_parses_arguments_string_exactly_once() {
        let raw = r#"{
            "choices": [{
                "message": {
                    "tool_calls": [{
                        "id": "call_1",
                        "function": { "name": "emit_plan", "arguments": "{\"steps\":[\"a\"]}" }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": { "prompt_tokens": 10, "completion_tokens": 5,
                       "prompt_tokens_details": { "cached_tokens": 8 } }
        }"#;
        let resp = decode(serde_json::from_str(raw).unwrap()).unwrap();

        assert_eq!(resp.finish, Finish::ToolUse);
        assert_eq!(resp.tool_calls.len(), 1);
        // 文字列ではなくオブジェクトとして運ばれている。
        assert_eq!(resp.tool_calls[0].args, json!({ "steps": ["a"] }));
        assert_eq!(resp.usage.cache_read, 8);
        assert_eq!(resp.usage.total(), 15);
    }

    #[test]
    fn decode_preserves_raw_on_broken_arguments() {
        let raw = r#"{"choices":[{"message":{"tool_calls":[
            {"function":{"name":"emit_plan","arguments":"{\"steps\": "}}]}}]}"#;
        let err = decode(serde_json::from_str(raw).unwrap()).unwrap_err();

        assert_eq!(err.raw_output(), Some(r#"{"steps": "#));
    }

    #[test]
    fn empty_reasoning_response_is_rejected_only_on_length() {
        let base = ChatResponse {
            text: Some("  ".into()),
            tool_calls: Vec::new(),
            finish: Finish::Length,
            usage: Usage::default(),
            grounding: Grounding::default(),
        };
        let err = reject_empty_reasoning(base.clone(), 4_096).unwrap_err();
        // 一過性にしない — 上限は入力に対して決定的で、再送すれば同じ所で切れる。
        assert!(!err.is_transient(), "再試行に乗せないこと");
        assert_eq!(err.code(), "LLM_OUTPUT_TRUNCATED");
        // 次の一手が「いくつまで上げるか」なので、現在値を文言に載せる。
        assert!(err.to_string().contains("4096"), "{err}");

        let normal_empty = ChatResponse {
            finish: Finish::Stop,
            ..base
        };
        assert!(reject_empty_reasoning(normal_empty, 4_096).is_ok());
    }
}
