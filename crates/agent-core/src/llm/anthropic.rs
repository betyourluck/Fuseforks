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

    let mut messages: Vec<wire::AnthropicMessage> = req
        .messages
        .iter()
        .filter(|m| m.role != Role::System)
        // 中身が完全に空の発話は送らない。空のテキストブロックも空の content 配列も
        // API に拒否される（400: text content blocks must be non-empty）。
        // 空の assistant 履歴が 1 件混ざるだけで**以後の全リクエストが失敗し続ける**
        // 毒になる（実機で発生。failures.md #29）。
        .filter(|m| {
            m.role == Role::Tool
                || !m.content.is_empty()
                || !m.tool_calls.is_empty()
                // 本文なしの画像だけの発話は「空」ではない（Spec 23）。
                || !m.attachments.is_empty()
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
    let tool_tokens = tool_tokens(&tools);
    let system = build_system_blocks(&system_text, req.cacheable_prefix_len, tool_tokens);

    // 境界をもう 1 つ、**増える側**（履歴）の末尾へ回す。安定プレフィックスだけを
    // 守っても、コストを支配するのは毎周伸びる履歴のほうで、そこに境界が無ければ
    // 節約の上限は「小さくて変わらない部分」に閉じる（failures.md #42 の一般化 2）。
    place_message_breakpoint(&mut messages, tool_tokens + approx_tokens(&system_text));

    let tool_choice = if tools.is_empty() {
        None
    } else {
        match &req.tool_choice {
            // 明示の "none" を送る。欠落（= 既定 auto）に写すと「定義は見せるが
            // 使わせない」が表現できない — まとめ呼び出し（ツール上限後の 1 回）は
            // 履歴の tool ブロックのために tools を残す必要があり、そこで使用まで
            // 許すと打ち切ったはずのツールをもう一度呼んでくる。
            ToolChoice::None => Some(wire::AnthropicToolChoice {
                kind: "none",
                name: None,
            }),
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

/// キャッシュの生存期間。
///
/// 既定は 5 分だが、**この製品の使われ方には短すぎる**。人が読んで考えて次を
/// 打つまでの間隔は普通に 5 分を超え、超えた瞬間に次のターンの 1 回目が
/// また書き込みになる。実機の観測でも、ターン数が増えるほど命中率が
/// 下がっていた（37% → 24%）。
///
/// 損得は明快で、**余計な書き込みが 1 回でも減れば長いほうが得**:
///
/// | | 書き込み | 読み取り |
/// |---|---|---|
/// | 5 分 | 1.25× | 0.1× |
/// | 1 時間 | 2.0× | 0.1× |
///
/// 5 分で切れて 2 回書くと 2.5×。1 時間なら 1 回で 2.0× + 0.1×。
/// **1 回の追加書き込みで既に逆転する。**
const CACHE_TTL: &str = "1h";

/// 5 分を超える TTL を要求するためのベータ機能名。
///
/// これを送らないと `ttl` が黙って無視される（既定の 5 分に戻る）。
/// **黙って効かない**のがこの機構の厄介なところで、指定したつもりで
/// 命中率だけが伸びない状態になる。
pub const EXTENDED_CACHE_BETA: &str = "extended-cache-ttl-2025-04-11";

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

/// キャッシュ指示の値。打ち場所によらず同じ TTL を要求する。
fn ephemeral() -> Option<wire::AnthropicCacheControl> {
    Some(wire::AnthropicCacheControl {
        kind: "ephemeral",
        ttl: Some(CACHE_TTL),
    })
}

/// 会話履歴が占める概算のトークン数。
///
/// キャッシュ判定は「最小要件を超えるか」の粗い足切りなので、この粒度で足りる。
/// ツール引数と結果本文も数える — **長いのはたいていそちら**で、テキストだけを
/// 数えるとツールループの履歴を実際より小さく見積もる。
fn message_tokens(messages: &[wire::AnthropicMessage]) -> usize {
    messages
        .iter()
        .flat_map(|m| m.content.iter())
        .map(|block| match block {
            wire::AnthropicRequestBlock::Text { text, .. } => approx_tokens(text),
            wire::AnthropicRequestBlock::ToolUse { name, input, .. } => {
                approx_tokens(name) + approx_tokens(&input.to_string())
            }
            wire::AnthropicRequestBlock::ToolResult { content, .. } => approx_tokens(content),
            // 視覚トークンは寸法（28px パッチ数）からしか出ず、adapter は base64 しか
            // 持たない。数えない = **少なめに見積もる側へ倒す**（この関数の既定の向き。
            // 画像つきの発話はどのみち履歴に残らないので恒常的な過小にはならない）。
            wire::AnthropicRequestBlock::Image { .. } => 0,
        })
        .sum()
}

/// 履歴の末尾へキャッシュの境界を打つ（純関数）。
///
/// ツールループは周回ごとに `messages` へ**追記しかしない**ので、次の周の前方一致は
/// 構造的に保証される。前周の書き込みを読み取りで拾い、その周の増分だけを書く形になる。
///
/// `prefix_tokens` は tools + system の概算トークン数。キャッシュされるのは
/// **tools + system + ここまでの履歴**なので、判定にはその全部を数える。
///
/// **打つのは最後の発話の最後のブロック 1 箇所だけ。** 境界の後方探索は
/// 20 ブロックまでなので、1 周の追記がそれを超えると前周の書き込みを見つけられず、
/// その周だけ書き直しになる（実機の 2〜3 本並列は 1 周 7 ブロック前後で収まる）。
///
/// なお**まとめ呼び出しの周は必ず書き込みになる** — `tool_choice` の変更は
/// tools / system 層のキャッシュは保つが履歴層は落とすため。1 ターンに 1 回なので
/// 打ち消す仕掛けは置かない。
fn place_message_breakpoint(messages: &mut [wire::AnthropicMessage], prefix_tokens: usize) {
    if prefix_tokens + message_tokens(messages) < MIN_CACHEABLE_TOKENS {
        return;
    }
    let Some(block) = messages.last_mut().and_then(|m| m.content.last_mut()) else {
        return;
    };
    match block {
        wire::AnthropicRequestBlock::Text { cache_control, .. }
        | wire::AnthropicRequestBlock::ToolUse { cache_control, .. }
        | wire::AnthropicRequestBlock::ToolResult { cache_control, .. }
        // 「種別は問わない」（prompt_cache の凍結）。本文なしの画像だけの発話では
        // 末尾ブロックが Image になる — ここに打てないと、その周だけ境界が抜ける。
        | wire::AnthropicRequestBlock::Image { cache_control, .. } => {
            *cache_control = ephemeral();
        }
    }
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
                cache_control: None,
            }],
        };
    }

    let mut content = Vec::new();
    // 画像はテキストより前に置く（公式の推奨。Spec 23 P2）。
    for attachment in &message.attachments {
        content.push(wire::AnthropicRequestBlock::Image {
            source: wire::AnthropicImageSource {
                kind: "base64",
                media_type: attachment.media_type.as_str().to_owned(),
                data: attachment.data.clone(),
            },
            cache_control: None,
        });
    }
    // 空のテキストブロックは拒否されるので、中身があるときだけ積む。
    if !message.content.is_empty() {
        content.push(wire::AnthropicRequestBlock::Text {
            text: message.content.clone(),
            cache_control: None,
        });
    }
    for call in &message.tool_calls {
        content.push(wire::AnthropicRequestBlock::ToolUse {
            id: call.id.clone(),
            name: call.name.clone(),
            // Anthropic の input は最初からオブジェクト。文字列化しない。
            input: call.args.clone(),
            cache_control: None,
        });
    }
    // ここへ来る発話は encode() で「完全に空」を落とし済みなので、通常この分岐は
    // 通らない。万一素通りしても**空のテキストブロックは送らない** — 空ブロックは
    // 空の content 配列と同じく API に拒否され、400 の毒として全ターンに波及する。
    if content.is_empty() {
        content.push(wire::AnthropicRequestBlock::Text {
            text: "（発言なし）".to_owned(),
            cache_control: None,
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
            // `output_tokens` の内数。足さない。
            //
            // **Spec 32 P1 はここを 0 で固定し、「Anthropic は usage に欄が無く
            // 構造的に取れない」と契約へ書いた。それは誤りだった**（P4 で訂正）。
            // 見ていたのは `AnthropicUsage`（当時 4 欄）で、**実際のワイヤは
            // それより多くの欄を返していた**。しかも
            // **`thinking` を 1 つも送っていない要求の応答にも付く** —
            // claude-sonnet-5 は既定で思考する（実測 5/5）。
            reasoning: u
                .output_tokens_details
                .as_ref()
                .map_or(0, |d| d.thinking_tokens),
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
    let mut dropped: Vec<&'static str> = Vec::new();
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
            // 本文にもツール呼び出しにも写せないブロック。**落とすが、数える。**
            wire::AnthropicContentBlock::Thinking => dropped.push("thinking"),
            wire::AnthropicContentBlock::RedactedThinking => dropped.push("redacted_thinking"),
            wire::AnthropicContentBlock::Other => dropped.push("unknown"),
        }
    }

    // **落としたブロックがあり、しかも本文が空なら、そのターンは画面から
    // 何も読めない。** 実機で出力 399 トークンを使って本文もツール呼び出しも
    // 無いターンが 2 回続き、どの計器にも痕跡が無かった（2026-08-06）。
    //
    // ここは #47 の L0 と同じ立場 — **挙動は変えず、気づけるようにするだけ**。
    // 頻度を見てから機構を足すかを決める（thinking を本文へ昇格させる、
    // 再試行する等は、1〜2 回の観測で作ると効いているか分からない機構が増える）。
    if !dropped.is_empty() {
        crate::note!(
            "dropped content blocks: kinds={} count={} output_tokens={} text_chars={} tool_calls={}",
            dropped.join("+"),
            dropped.len(),
            usage.completion,
            text.chars().count(),
            tool_calls.len(),
        );
    }

    Ok(ChatResponse {
        text: if text.is_empty() { None } else { Some(text) },
        tool_calls,
        finish,
        usage,
        // このプロバイダは接地を代行しない。
        grounding: Grounding::default(),
        reasoning_summary: Vec::new(),
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

    /// ツール定義を残したまま使用だけを禁じる形（まとめ呼び出しのワイヤ契約）。
    ///
    /// 履歴に tool_use / tool_result ブロックが残る限り tools の定義は必須で、
    /// 空にすると API が 400 を返す（failures.md #36）。だから「取り上げる」は
    /// `tools` を消すことではなく、明示の `tool_choice: none` で表現する。
    /// 欠落（= 既定 auto）に写すと、打ち切ったはずのツールをもう一度呼んでくる。
    #[test]
    fn tool_choice_none_is_sent_explicitly_with_tools_kept() {
        let mut req = request(0);
        req.tool_choice = ToolChoice::None;
        let json = serde_json::to_value(encode(&req)).unwrap();

        assert_eq!(json["tool_choice"]["type"], "none");
        assert_eq!(json["tools"][0]["name"], "emit_plan", "定義は残ること");
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

    /// **添付なしのエンコード結果はバイト等価**（Spec 23 P2 の凍結）。
    ///
    /// golden を文字列リテラルで固定する。ここが崩れると、画像を一度も
    /// 使わない村の全リクエストの形が変わり、キャッシュの前方一致が割れる —
    /// そして画面には何も出ない。
    #[test]
    fn encoding_without_attachments_matches_the_golden_bytes() {
        let req = ChatRequest::plain(
            "claude-opus-5",
            vec![ChatMessage::system("s"), ChatMessage::user("こんにちは")],
            512,
        );
        assert_eq!(
            serde_json::to_string(&encode(&req)).unwrap(),
            r#"{"model":"claude-opus-5","max_tokens":512,"system":[{"type":"text","text":"s"}],"messages":[{"role":"user","content":[{"type":"text","text":"こんにちは"}]}]}"#
        );
    }

    /// 添付は image ブロックになり、**テキストより前**に置かれる（Spec 23 P2）。
    #[test]
    fn attachments_become_image_blocks_before_text() {
        use crate::llm::canonical::{ImageAttachment, ImageMediaType};
        let req = ChatRequest::plain(
            "claude-opus-5",
            vec![ChatMessage::user_with_attachments(
                "何が見える？",
                vec![ImageAttachment {
                    media_type: ImageMediaType::Webp,
                    data: "QUJD".into(),
                }],
            )],
            512,
        );
        let json = serde_json::to_value(&encode(&req).messages).unwrap();
        assert_eq!(
            json[0]["content"],
            json!([
                {"type":"image","source":{"type":"base64","media_type":"image/webp","data":"QUJD"}},
                {"type":"text","text":"何が見える？"}
            ])
        );
    }

    /// 本文なしの画像だけの発話は「空」ではない — #29 の空発話フィルタに
    /// 食われず、空テキストのフォールバックも入らない。
    #[test]
    fn an_image_only_message_is_not_dropped_as_empty() {
        use crate::llm::canonical::{ImageAttachment, ImageMediaType};
        let req = ChatRequest::plain(
            "claude-opus-5",
            vec![ChatMessage::user_with_attachments(
                "",
                vec![ImageAttachment {
                    media_type: ImageMediaType::Webp,
                    data: "QUJD".into(),
                }],
            )],
            512,
        );
        let w = encode(&req);
        assert_eq!(w.messages.len(), 1, "画像だけの発話は落ちない");
        let json = serde_json::to_value(&w.messages[0].content).unwrap();
        let blocks = json.as_array().unwrap();
        assert_eq!(blocks.len(), 1, "空テキストも「発言なし」も入らない: {json}");
        assert_eq!(blocks[0]["type"], "image");
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

    /// 既定の 5 分ではなく 1 時間を要求すること。
    ///
    /// 対話的な使い方では次の発話まで 5 分を超えるのが普通で、超えた瞬間に
    /// 書き込みからやり直しになる。書き込みは読み取りの 10 倍以上なので、
    /// 余計な書き込みが 1 回でも減れば長い TTL のほうが得。
    #[test]
    fn cache_requests_the_extended_ttl_not_the_five_minute_default() {
        let stable = "指示".repeat(3_000);
        let mut req = request(0);
        req.messages[0] = ChatMessage::system(format!("{stable}可変部分"));
        req.cacheable_prefix_len = stable.chars().count();

        let control = encode(&req).system[0].cache_control.clone().unwrap();
        assert_eq!(control.kind, "ephemeral");
        assert_eq!(control.ttl, Some("1h"));

        // ワイヤ形も確認する。ttl が抜けると黙って 5 分へ戻る。
        let json = serde_json::to_value(encode(&req)).unwrap();
        assert_eq!(json["system"][0]["cache_control"]["ttl"], "1h");
        assert_eq!(json["system"][0]["cache_control"]["type"], "ephemeral");
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

    /// 履歴の末尾にも境界を打つこと。
    ///
    /// 安定プレフィックスだけを守っても、毎周伸びる履歴は守られない。
    /// system にしか打っていなかった頃は 1 ターンの入力 2,052,314 トークンのうち
    /// 1,826,109 が素の値段で通っていた（failures.md #42）。
    #[test]
    fn the_history_carries_a_cache_breakpoint_at_its_tail() {
        let mut req = request(0);
        req.messages = vec![
            ChatMessage::system("短い指示"),
            ChatMessage::user(format!("覚えて{}", "履歴".repeat(3_000))),
            ChatMessage::assistant("承知"),
        ];

        let w = encode(&req);

        assert_eq!(w.messages.len(), 2);
        assert!(
            matches!(
                w.messages[1].content.last().unwrap(),
                wire::AnthropicRequestBlock::Text {
                    cache_control: Some(_),
                    ..
                }
            ),
            "末尾に打つこと: {:?}",
            w.messages[1].content
        );
        assert!(
            matches!(
                w.messages[0].content.last().unwrap(),
                wire::AnthropicRequestBlock::Text {
                    cache_control: None,
                    ..
                }
            ),
            "打つのは末尾の 1 箇所だけ: {:?}",
            w.messages[0].content
        );
    }

    /// ツールループの周は `tool_result` で終わる。そこにも打てること。
    ///
    /// テキストだけを対象にすると、**ツールループの周回だけ境界が抜ける** —
    /// キャッシュが最も効いてほしい経路がちょうど外れる。
    #[test]
    fn a_tool_result_tail_also_carries_the_breakpoint() {
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
            ChatMessage::tool_result("tu_1", "remember", "結果".repeat(3_000)),
        ];

        let json = serde_json::to_value(encode(&req)).unwrap();
        let last = json["messages"].as_array().unwrap().last().unwrap();

        assert_eq!(last["content"][0]["type"], "tool_result");
        assert_eq!(last["content"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(last["content"][0]["cache_control"]["ttl"], "1h");
    }

    /// 短い履歴には要求しないこと（効かない指示は送らない）。
    #[test]
    fn a_short_history_is_not_marked_for_caching() {
        let mut req = request(0);
        req.messages = vec![ChatMessage::system("短い指示"), ChatMessage::user("やあ")];

        let json = serde_json::to_value(encode(&req)).unwrap().to_string();

        assert!(!json.contains("cache_control"), "効かない指示は送らない: {json}");
    }

    /// 境界は system と履歴の 2 つだけ。Anthropic の上限は 4 なので枠は残る。
    #[test]
    fn a_turn_spends_two_of_the_four_breakpoints() {
        let stable = "指示".repeat(3_000);
        let mut req = request(0);
        req.messages = vec![
            ChatMessage::system(format!("{stable}可変部分")),
            ChatMessage::user("進捗を教えて"),
        ];
        req.cacheable_prefix_len = stable.chars().count();

        let json = serde_json::to_value(encode(&req)).unwrap().to_string();

        assert_eq!(
            json.matches(r#""cache_control""#).count(),
            2,
            "system の安定部分と履歴の末尾の 2 箇所: {json}"
        );
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

    /// **本文以外のブロックだけが返ったターンを、空として黙って通さない。**
    ///
    /// 実機で出力 399 トークン・本文なし・ツール呼び出しなしが 2 回続き、
    /// **どの計器にも痕跡が無かった**（2026-08-06）。decode は写す先が無い
    /// ブロックを落とすが、落としたことは数えて 1 行出す（#47 の L0 と同じ立場 —
    /// 挙動は変えず、気づけるようにするだけ）。
    #[test]
    fn thinking_only_responses_decode_to_no_text_and_are_counted() {
        let raw = serde_json::json!({
            "content": [
                { "type": "thinking", "thinking": "…" },
                { "type": "redacted_thinking", "data": "…" }
            ],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 10, "output_tokens": 399 }
        });
        let resp: wire::AnthropicResponse = serde_json::from_value(raw).unwrap();
        let out = decode(resp).unwrap();

        assert!(out.text.is_none(), "本文は無い（写す先が無いので落とす）");
        assert!(out.tool_calls.is_empty());
        assert_eq!(
            out.usage.completion, 399,
            "出力トークンは数えられている — ここが 0 なら、消えたのはトークンではなく計器のほう"
        );
    }

    /// **#72 の実物**（実測 2026-08-10。probe D の応答そのままの形）。
    ///
    /// **`thinking` を 1 つも送っていない要求**に対し、claude-sonnet-5 が
    /// 出力 2,048 トークン**全部**を思考に使い、本文を 1 ブロックも返さなかった。
    /// `output_tokens == thinking_tokens == max_tokens` が同時に成り立つ。
    ///
    /// Spec 32 P1 はこの欄を読まず `reasoning: 0` で固定し、契約へ
    /// 「Anthropic は構造的に取れない」と書いた。**P4 で訂正した誤り。**
    #[test]
    fn a_turn_that_spent_everything_on_thinking_reports_it_as_reasoning() {
        let raw = serde_json::json!({
            "content": [
                { "type": "thinking", "thinking": "", "signature": "EoYxCokBCBAY…" }
            ],
            "stop_reason": "max_tokens",
            "usage": {
                "input_tokens": 101,
                "output_tokens": 2048,
                "output_tokens_details": { "thinking_tokens": 2048 }
            }
        });
        let out = decode(serde_json::from_value::<wire::AnthropicResponse>(raw).unwrap()).unwrap();

        assert!(out.text.is_none());
        assert_eq!(out.usage.completion, 2_048);
        assert_eq!(
            out.usage.reasoning, 2_048,
            "本文ゼロのターンで、払ったものが全部思考だったと読める"
        );
        // 内数。外数で実装すると 4,197 になる。
        assert_eq!(out.usage.total(), 2_149);
    }

    /// 内訳欄を持たない応答（古い API 版・互換の中継）では 0 に落ちる。
    /// **欄が無いことは失敗ではない。**
    #[test]
    fn a_response_without_the_breakdown_reads_as_zero_reasoning() {
        let raw = serde_json::json!({
            "content": [{ "type": "text", "text": "はい" }],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 10, "output_tokens": 5 }
        });
        let out = decode(serde_json::from_value::<wire::AnthropicResponse>(raw).unwrap()).unwrap();

        assert_eq!(out.usage.reasoning, 0);
        assert_eq!(out.usage.completion, 5);
    }

    /// thinking と本文が並んでいれば、本文はそのまま取れる（既存の挙動を保つ）。
    #[test]
    fn thinking_alongside_text_still_yields_the_text() {
        let raw = serde_json::json!({
            "content": [
                { "type": "thinking", "thinking": "…" },
                { "type": "text", "text": "了解" }
            ],
            "stop_reason": "end_turn"
        });
        let resp: wire::AnthropicResponse = serde_json::from_value(raw).unwrap();
        assert_eq!(decode(resp).unwrap().text.as_deref(), Some("了解"));
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
