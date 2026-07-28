//! OpenAI 互換 adapter（OpenAI / Grok / ローカル互換サーバ）。
//!
//! canonical ⇄ wire の **encode / decode 純関数**のみを持つ。
//! HTTP・認証・再試行は [`super::client`] が担当し、ここは形の翻訳だけを持つ。
//! 壊れるのは常に ser/de なので、この層をテストで固めることに意味がある。
//!
//! 由来: Kataribe `crates/llm_client/src/openai_compat.rs`。

use serde_json::Value;

use super::canonical::{
    ChatMessage, ChatRequest, ChatResponse, Effort, Finish, Role, ToolCall, ToolChoice, Usage,
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
            ToolChoice::None => None,
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

    wire::OaiRequest {
        model: req.model.clone(),
        messages,
        temperature: req.temperature,
        max_tokens: req.max_tokens,
        tools,
        tool_choice,
        reasoning_effort: reasoning_effort(&req.model, req.effort),
    }
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
    let is_oai_reasoning = model.starts_with('o') && model.chars().nth(1).is_some_and(|c| c.is_ascii_digit());

    if !is_grok_reasoning && !is_oai_reasoning {
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
        });
    }

    Ok(ChatResponse {
        text: choice.message.content,
        tool_calls,
        finish,
        usage,
    })
}

/// 空応答の防御（純関数）。
///
/// **本文が空 かつ tool_calls が空 かつ `finish == Length`** のときだけ
/// [`LlmError::EmptyResponse`] にする。これは推論モデルが出力予算を全部思考に使い切った
/// 状態であり、再抽選で回復しうるため一過性として再試行に乗せる。
///
/// `Length` 以外の空応答はここでは弾かない。通常の空応答は再送しても同じ結果になるため、
/// 呼び出し側へそのまま返して原因を見せるほうが正しい。
pub fn reject_empty_reasoning(resp: ChatResponse) -> Result<ChatResponse, LlmError> {
    let text_empty = resp.text.as_deref().is_none_or(|t| t.trim().is_empty());
    if text_empty && resp.tool_calls.is_empty() && resp.finish == Finish::Length {
        return Err(LlmError::EmptyResponse);
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
        };
        assert!(reject_empty_reasoning(base.clone()).is_err());

        let normal_empty = ChatResponse {
            finish: Finish::Stop,
            ..base
        };
        assert!(reject_empty_reasoning(normal_empty).is_ok());
    }
}
