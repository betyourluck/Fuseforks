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

use super::canonical::{
    ChatMessage, ChatResponse, Finish, Grounding, Role, ToolCall, ToolChoice, Usage,
};
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

    // キャッシュされるのは **tools + system の安定部分**（`cache_control` を打った
    // ブロックまでの全体で、tools は system より前に置かれる）。判定に tools を
    // 数えないと、**道具を多く提示しているエージェントほど判定を外す** — 提示量が
    // 多いほどキャッシュの利得は大きいのに、そこで切ってしまう。
    let system = build_system_blocks(&system_text, req.cacheable_prefix_len, tool_tokens(&tools));

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

/// プロンプトキャッシュを要求する最小トークン数。
///
/// キャッシュには最小長があり、それを下回るプレフィックスは指示を出しても
/// 再利用されない。効かないと分かっている指示は送らない。
/// 実際の下限はモデル階層で違う（上位ほど小さい）ので、**一番厳しい側**に合わせる。
///
/// > 当初はこれを「4,000 **文字**」で判定していた。2 つ外していた。
/// > (1) 判定対象が system の安定部分だけで、**tools を数えていなかった** —
/// >     キャッシュされるのは tools + system なのに。
/// > (2) 文字数の閾値は**言語をまたげない**。英語は 4 文字 ≈ 1 トークンだが
/// >     日本語は 1 文字 ≈ 1 トークンで、同じ 4,000 でも要求が 4 倍変わる。
/// >     4,000 は英語で較正された値だった。
/// > 結果、日本語で設定された実機の 5 体全員が 900〜1,100 文字で足切りされ、
/// > **キャッシュが一度も効いていなかった**（failures.md #33）。
const MIN_CACHEABLE_TOKENS: usize = 2_048;

/// 概算のトークン数。
///
/// 正確な数はトークナイザ無しには出せないが、キャッシュ判定は
/// 「最小要件を超えるか」の粗い足切りなので、この粒度で足りる。
/// ASCII は 4 文字 ≈ 1 トークン、それ以外（日本語・絵文字など）は
/// 1 文字 ≈ 1 トークンとして数える。**少なめに見積もる側へ倒す** —
/// 足りないのに要求するより、足りているのに見送るほうが害が小さい。
fn approx_tokens(text: &str) -> usize {
    let (ascii, wide) = text
        .chars()
        .fold((0usize, 0usize), |(a, w), c| {
            if c.is_ascii() { (a + 1, w) } else { (a, w + 1) }
        });
    ascii / 4 + wide
}

/// ツール定義がプロンプトに占める概算のトークン数。
///
/// ツール定義は **system より前**に置かれ、`cache_control` を system に打つと
/// まとめてキャッシュ対象に入る。判定に数えないと、道具を多く提示している
/// エージェントほど判定を外す — 提示量が多いほど利得は大きいのに。
fn tool_tokens(tools: &[wire::AnthropicTool]) -> usize {
    tools
        .iter()
        .map(|t| {
            approx_tokens(&t.name)
                + approx_tokens(&t.description)
                // スキーマは JSON 文字列として数える（構造は問わない）。
                + approx_tokens(&t.input_schema.to_string())
        })
        .sum()
}

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
/// キャッシュを要求するのは、**tools + 安定部分**の概算トークン数が
/// [`MIN_CACHEABLE_TOKENS`] 以上のときだけ。`tool_tokens` を足さないと
/// 道具の多いエージェントほど判定を外す。
///
/// 境界は文字数指定だが、マルチバイト文字の途中で切らないよう `char_indices` で丸める。
fn build_system_blocks(
    text: &str,
    prefix_len: usize,
    tool_tokens: usize,
) -> Vec<wire::AnthropicTextBlock> {
    if text.is_empty() {
        return Vec::new();
    }

    let ephemeral = || {
        Some(wire::AnthropicCacheControl {
            kind: "ephemeral",
        })
    };

    let stable: String = text.chars().take(prefix_len).collect();
    if tool_tokens + approx_tokens(&stable) < MIN_CACHEABLE_TOKENS {
        return vec![wire::AnthropicTextBlock {
            kind: "text",
            text: text.to_owned(),
            cache_control: None,
        }];
    }

    match text.char_indices().nth(prefix_len) {
        // 境界が本文の内側にある。安定部分と可変部分の 2 ブロックへ割る。
        Some((idx, _)) if idx > 0 => vec![
            wire::AnthropicTextBlock {
                kind: "text",
                text: text[..idx].to_owned(),
                cache_control: ephemeral(),
            },
            wire::AnthropicTextBlock {
                kind: "text",
                text: text[idx..].to_owned(),
                cache_control: None,
            },
        ],
        // 境界が本文の**末尾以降**にある = 可変部分が空（Memory.md が未記入など）。
        // 割らずに全体へ `cache_control` を打つ。ここを `None` に落とすと、
        // 「記憶がまだ空のエージェントだけキャッシュが効かない」という
        // 気づきにくい欠落になる。
        None => vec![wire::AnthropicTextBlock {
            kind: "text",
            text: text.to_owned(),
            cache_control: ephemeral(),
        }],
        // 境界が先頭（安定部分が空）。打つ場所が無い。
        Some(_) => vec![wire::AnthropicTextBlock {
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
        // このプロバイダは接地を代行しない。
        grounding: Grounding::default(),
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

    /// 実機の構成（日本語の短い設定 + 十数本のツール）でキャッシュが要求されること。
    ///
    /// 当初は system の安定部分だけで判定しており、村の 5 体全員が
    /// 900〜1,100 文字で足切りされ**キャッシュが一度も効いていなかった**。
    /// 設定は日本語なのでバイト数の 1/3 しか文字数が無く、4,000 に遠く届かない。
    /// キャッシュされるのは tools + system なので、判定にも tools を数える。
    #[test]
    fn a_real_village_agent_still_gets_caching_thanks_to_its_tools() {
        // 条例 235 + 固定テンプレ約 300 + SKILL 588 ≒ 1,123 文字（実測値）。
        let stable = "村".repeat(1_123);
        let mut req = request(0);
        req.messages[0] = ChatMessage::system(format!("{stable}記憶"));
        req.cacheable_prefix_len = stable.chars().count();

        // 同梱 5 + transfer_to_*/ask_* 8 = 13 本。説明文は日本語 2 文程度。
        req.tools = (0..13)
            .map(|i| ToolSpec {
                name: format!("transfer_to_agent_{i}"),
                description: "相手へメッセージを渡して、会話を続ける。\
                     相手は自分で考えて返事をするので、返事を代筆しないこと。"
                    .into(),
                parameters: json!({
                    "type": "object",
                    "properties": { "message": { "type": "string", "description": "伝える内容" } },
                    "required": ["message"],
                    "additionalProperties": false
                }),
            })
            .collect();

        let w = encode(&req);

        assert_eq!(w.system.len(), 2, "安定部分と可変部分に割れること");
        assert!(
            w.system[0].cache_control.is_some(),
            "system だけで足切りしない（tools を数えれば最小要件を超える）"
        );
    }

    /// 可変部分が空でもキャッシュを要求すること。
    ///
    /// `Memory.md` が未記入だと安定部分が本文全体になり、境界の探索が
    /// 末尾を越えて `None` を返す。ここを無キャッシュに落とすと
    /// 「記憶がまだ空のエージェントだけ効かない」という気づきにくい欠落になる。
    #[test]
    fn an_agent_without_memory_yet_is_still_cached() {
        let stable = "指示".repeat(3_000);
        let mut req = request(0);
        req.messages[0] = ChatMessage::system(stable.clone());
        req.cacheable_prefix_len = stable.chars().count();

        let w = encode(&req);

        assert_eq!(w.system.len(), 1, "割る先が無いので 1 ブロック");
        assert!(w.system[0].cache_control.is_some(), "それでもキャッシュは要求する");
    }

    /// 短すぎるプレフィックスには要求しないこと（効かない指示は送らない）。
    #[test]
    fn a_tiny_prompt_with_few_tools_does_not_request_caching() {
        let mut req = request(0);
        req.messages[0] = ChatMessage::system("短い指示可変");
        req.cacheable_prefix_len = 4;

        let w = encode(&req);

        assert_eq!(w.system.len(), 1);
        assert!(w.system[0].cache_control.is_none());
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
