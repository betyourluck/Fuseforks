//! Anthropic Messages API adapter。
//!
//! OpenAI 互換層を経由せずネイティブ経路を持つ理由は **プロンプトキャッシュ**にある。
//! 互換層はキャッシュ指示を通せないため、毎ターンのフルプロンプト再送が全額課金になる。
//! ネイティブなら安定プレフィックス（システム指示 + 役割定義）に `cache_control` を打てて、
//! 読み取り側が大幅に安くなる。マルチエージェントは同じ system を N 体分毎ターン送るので、
//! ここの差が運用コストに直結する。
//!
//! canonical 側との差分（adapter が吸収する方言）:
//! - `system` はメッセージ配列に混ぜず、独立したトップレベルフィールド
//! - ツールの引数スキーマのキーは `parameters` ではなく `input_schema`
//! - ツール引数 `input` は **JSON オブジェクト**（OpenAI のような JSON 文字列ではない）
//! - 使用量のキー名が `input_tokens` / `output_tokens` / `cache_read_input_tokens`
//!
//! 由来: Kataribe `crates/llm_client/src/anthropic.rs` の設計方針。

use super::canonical::{ChatMessage, ChatResponse, Finish, Role, ToolCall, ToolChoice, Usage};
use super::error::LlmError;
use super::wire;
use crate::llm::canonical::ChatRequest;

/// canonical → Anthropic wire。
///
/// `cacheable_prefix_len > 0` のとき、system プロンプトを
/// 「安定部分 + 可変部分」の 2 ブロックに割り、安定部分の末尾に `cache_control` を打つ。
/// 分割位置は呼び出し側が文字数で宣言する契約にしてある。
pub fn encode(req: &ChatRequest) -> wire::AnthropicRequest {
    // system ロールは Anthropic では messages に混ぜられないため、先に抜き出して連結する。
    let system_text: String = req
        .messages
        .iter()
        .filter(|m| m.role == Role::System)
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");

    let system = build_system_blocks(&system_text, req.cacheable_prefix_len);

    let messages = req
        .messages
        .iter()
        .filter(|m| m.role != Role::System)
        // 中身が完全に空の発話は送らない。空のテキストブロックも空の content 配列も
        // API に拒否される（400: text content blocks must be non-empty）。
        // 空の assistant 履歴が 1 件混ざるだけで**以後の全リクエストが失敗し続ける**
        // 毒になる（実機で発生。failures.md #29）。
        .filter(|m| {
            m.role == Role::Tool || !m.content.is_empty() || !m.tool_calls.is_empty()
        })
        .map(encode_message)
        .collect();

    let tools = req
        .tools
        .iter()
        .map(|t| wire::AnthropicTool {
            name: t.name.clone(),
            description: t.description.clone(),
            input_schema: t.parameters.clone(),
        })
        .collect::<Vec<_>>();

    let tool_choice = if tools.is_empty() {
        None
    } else {
        match &req.tool_choice {
            ToolChoice::None => None,
            ToolChoice::Auto => Some(wire::AnthropicToolChoice {
                kind: "auto",
                name: None,
            }),
            ToolChoice::Required => Some(wire::AnthropicToolChoice {
                kind: "any",
                name: None,
            }),
            ToolChoice::Specific(name) => Some(wire::AnthropicToolChoice {
                kind: "tool",
                name: Some(name.clone()),
            }),
        }
    };

    wire::AnthropicRequest {
        model: req.model.clone(),
        max_tokens: req.max_tokens,
        system,
        messages,
        temperature: req.temperature,
        tools,
        tool_choice,
    }
}

/// プロンプトキャッシュを要求する最小文字数。
///
/// キャッシュには最小トークン数があり、それを下回るプレフィックスは
/// 指示を出しても再利用されない。効かないと分かっている指示は送らない。
/// 送っても無害なはずのものが実際には拒否されうる以上、
/// **利得が無い経路でリスクだけ取らない**のが正しい。
/// 4000 文字は日本語で概ね 1500〜2500 トークンに相当し、最小要件を安全に超える。
const MIN_CACHEABLE_CHARS: usize = 4_000;

/// canonical の 1 発話を Anthropic の形へ写す。
///
/// **ツール結果は `user` ロールに載せる**のが Anthropic の形で、
/// OpenAI 互換の `role: "tool"` とは構造が違う。この差を吸収するのが adapter の仕事。
fn encode_message(message: &ChatMessage) -> wire::AnthropicMessage {
    // ツール結果。role は user、中身は tool_result ブロック。
    if message.role == Role::Tool {
        return wire::AnthropicMessage {
            role: "user",
            content: vec![wire::AnthropicRequestBlock::ToolResult {
                tool_use_id: message.tool_call_id.clone().unwrap_or_default(),
                content: message.content.clone(),
            }],
        };
    }

    let mut content = Vec::new();
    // 空のテキストブロックは拒否されるので、中身があるときだけ積む。
    if !message.content.is_empty() {
        content.push(wire::AnthropicRequestBlock::Text {
            text: message.content.clone(),
        });
    }
    for call in &message.tool_calls {
        content.push(wire::AnthropicRequestBlock::ToolUse {
            id: call.id.clone(),
            name: call.name.clone(),
            // Anthropic の input は最初からオブジェクト。文字列化しない。
            input: call.args.clone(),
        });
    }
    // ここへ来る発話は encode() で「完全に空」を落とし済みなので、通常この分岐は
    // 通らない。万一素通りしても**空のテキストブロックは送らない** — 空ブロックは
    // 空の content 配列と同じく API に拒否され、400 の毒として全ターンに波及する。
    if content.is_empty() {
        content.push(wire::AnthropicRequestBlock::Text {
            text: "（発言なし）".to_owned(),
        });
    }

    wire::AnthropicMessage {
        role: if message.role == Role::Assistant {
            "assistant"
        } else {
            "user"
        },
        content,
    }
}

/// system プロンプトをキャッシュ境界で分割する（純関数）。
///
/// 分割するのは、安定部分が [`MIN_CACHEABLE_CHARS`] 以上あるときだけ。
/// それ以外は単一ブロックにして `cache_control` を出さない。
/// 境界は文字数指定だが、マルチバイト文字の途中で切らないよう `char_indices` で丸める。
fn build_system_blocks(text: &str, prefix_len: usize) -> Vec<wire::AnthropicTextBlock> {
    if text.is_empty() {
        return Vec::new();
    }

    let cut = if prefix_len >= MIN_CACHEABLE_CHARS {
        text.char_indices()
            .nth(prefix_len)
            .map(|(byte_idx, _)| byte_idx)
    } else {
        None
    };

    match cut {
        // 境界が本文の内側にあるときだけ 2 ブロックへ割る。
        Some(idx) if idx > 0 => vec![
            wire::AnthropicTextBlock {
                kind: "text",
                text: text[..idx].to_owned(),
                cache_control: Some(wire::AnthropicCacheControl {
                    kind: "ephemeral",
                }),
            },
            wire::AnthropicTextBlock {
                kind: "text",
                text: text[idx..].to_owned(),
                cache_control: None,
            },
        ],
        _ => vec![wire::AnthropicTextBlock {
            kind: "text",
            text: text.to_owned(),
            cache_control: None,
        }],
    }
}

/// Anthropic wire → canonical。
///
/// コンテンツブロック列を走査し、テキストは連結、`tool_use` は [`ToolCall`] へ移す。
/// `input` は既にオブジェクトなので、OpenAI 経路のような文字列 parse は不要。
pub fn decode(resp: wire::AnthropicResponse) -> Result<ChatResponse, LlmError> {
    let usage = resp
        .usage
        .as_ref()
        .map(|u| Usage {
            // キャッシュ読み取り分を prompt に含めて、プロバイダ間で総量の意味を揃える。
            prompt: u.input_tokens + u.cache_read_input_tokens + u.cache_creation_input_tokens,
            completion: u.output_tokens,
            cache_read: u.cache_read_input_tokens,
        })
        .unwrap_or_default();

    let finish = match resp.stop_reason.as_deref() {
        Some("end_turn") | Some("stop_sequence") => Finish::Stop,
        Some("tool_use") => Finish::ToolUse,
        Some("max_tokens") => Finish::Length,
        _ => Finish::Other,
    };

    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for block in resp.content {
        match block {
            wire::AnthropicContentBlock::Text { text: chunk } => text.push_str(&chunk),
            wire::AnthropicContentBlock::ToolUse { id, name, input } => {
                tool_calls.push(ToolCall {
                    id,
                    name,
                    args: input,
                    // Anthropic の tool_use に随伴データは無い。署名相当（thinking ブロック）は
                    // ブロック単位で別枠であり、この adapter は現状それを運ばない。
                    extra: None,
                });
            }
            // thinking など未知のブロックは canonical に写す先がないので落とす。
            wire::AnthropicContentBlock::Other => {}
        }
    }

    Ok(ChatResponse {
        text: if text.is_empty() { None } else { Some(text) },
        tool_calls,
        finish,
        usage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::canonical::{ChatMessage, ToolSpec};
    use serde_json::json;

    fn request(cacheable_prefix_len: usize) -> ChatRequest {
        ChatRequest {
            model: "claude-opus-5".into(),
            messages: vec![
                ChatMessage::system("あなたは計画立案担当です。"),
                ChatMessage::user("進捗を教えて"),
            ],
            tools: vec![ToolSpec {
                name: "emit_plan".into(),
                description: "計画を出力する".into(),
                parameters: json!({ "type": "object" }),
            }],
            tool_choice: ToolChoice::Specific("emit_plan".into()),
            temperature: None,
            max_tokens: 1024,
            effort: None,
            cacheable_prefix_len,
        }
    }

    /// 中身が完全に空の発話はワイヤへ出さない。
    ///
    /// 空のテキストブロックは API に 400 で拒否される。空の assistant 履歴が
    /// 1 件混ざるだけで以後の全リクエストが失敗し続ける（実機で発生、
    /// failures.md #29）。落とすのは「テキストもツール呼び出しも無い」発話だけで、
    /// ツール呼び出しだけの発話は正当なので残る。
    #[test]
    fn completely_empty_messages_are_dropped_from_the_wire() {
        let mut req = request(0);
        req.messages = vec![
            ChatMessage::system("system"),
            ChatMessage::user("一回目"),
            ChatMessage::assistant(""), // 毒: 空の assistant 履歴
            ChatMessage::user("二回目"),
        ];

        let w = encode(&req);

        assert_eq!(w.messages.len(), 2, "空の発話は落ちる: {:?}", w.messages);
        let json = serde_json::to_value(&w.messages).unwrap();
        assert!(
            !json.to_string().contains(r#""text":"""#),
            "空のテキストブロックがワイヤに現れないこと: {json}"
        );
    }

    #[test]
    fn system_is_lifted_out_of_messages() {
        let w = encode(&request(0));

        assert_eq!(w.system.len(), 1);
        assert_eq!(w.system[0].text, "あなたは計画立案担当です。");
        assert_eq!(w.messages.len(), 1, "system は messages に残らない");
        assert_eq!(w.messages[0].role, "user");
    }

    #[test]
    fn a_short_prefix_is_not_marked_for_caching() {
        // 最小長を下回る安定部分にキャッシュ指示を出しても再利用されない。
        // 効かない指示は送らない。
        let w = encode(&request(6));

        assert_eq!(w.system.len(), 1);
        assert!(w.system[0].cache_control.is_none());
        assert_eq!(w.system[0].text, "あなたは計画立案担当です。");
    }

    #[test]
    fn cache_control_is_placed_at_the_declared_boundary_when_long_enough() {
        let stable = "指示".repeat(3_000); // 6000 文字
        let mut req = request(0);
        req.messages[0] = ChatMessage::system(format!("{stable}可変部分"));
        req.cacheable_prefix_len = stable.chars().count();

        let w = encode(&req);

        assert_eq!(w.system.len(), 2);
        assert!(w.system[0].cache_control.is_some());
        assert_eq!(w.system[1].text, "可変部分");
        assert!(w.system[1].cache_control.is_none());
    }

    #[test]
    fn tool_schema_uses_input_schema_key() {
        let json = serde_json::to_value(encode(&request(0))).unwrap();

        assert_eq!(json["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(json["tool_choice"]["type"], "tool");
        assert_eq!(json["tool_choice"]["name"], "emit_plan");
        assert!(json.get("temperature").is_none());
    }

    /// ツール往復の形。**OpenAI 互換とは構造が違う**ことを固定する。
    ///
    /// 結果は `role: "tool"` ではなく `user` メッセージの `tool_result` ブロック。
    /// ここを取り違えると、ツールを 1 回呼んだ瞬間に会話が壊れる。
    #[test]
    fn tool_round_trip_uses_content_blocks_not_a_tool_role() {
        let calls = vec![ToolCall {
            id: "tu_1".into(),
            name: "remember".into(),
            args: json!({ "note": "覚えること" }),
            extra: None,
        }];

        let mut req = request(0);
        req.messages = vec![
            ChatMessage::user("覚えておいて"),
            ChatMessage::assistant_tool_calls("", calls),
            ChatMessage::tool_result("tu_1", "remember", "書き留めました。"),
        ];

        let json = serde_json::to_value(encode(&req)).unwrap();
        let messages = json["messages"].as_array().unwrap();

        assert_eq!(messages.len(), 3);

        // 呼び出しは assistant の tool_use ブロック。input はオブジェクトのまま。
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"][0]["type"], "tool_use");
        assert_eq!(messages[1]["content"][0]["input"]["note"], "覚えること");

        // 結果は user の tool_result ブロック。
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[2]["content"][0]["type"], "tool_result");
        assert_eq!(messages[2]["content"][0]["tool_use_id"], "tu_1");
    }

    #[test]
    fn decode_normalizes_usage_and_tool_input() {
        let raw = r#"{
            "content": [
                { "type": "text", "text": "了解" },
                { "type": "tool_use", "id": "tu_1", "name": "emit_plan", "input": { "steps": ["a"] } }
            ],
            "stop_reason": "tool_use",
            "usage": { "input_tokens": 10, "output_tokens": 5,
                       "cache_read_input_tokens": 90, "cache_creation_input_tokens": 0 }
        }"#;
        let resp = decode(serde_json::from_str(raw).unwrap()).unwrap();

        assert_eq!(resp.text.as_deref(), Some("了解"));
        assert_eq!(resp.finish, Finish::ToolUse);
        assert_eq!(resp.tool_calls[0].args, json!({ "steps": ["a"] }));
        // キャッシュ読み取り分を含めた総入力量に正規化される。
        assert_eq!(resp.usage.prompt, 100);
        assert_eq!(resp.usage.cache_read, 90);
    }
}
