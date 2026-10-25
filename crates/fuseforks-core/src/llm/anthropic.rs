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
    ChatMessage, ChatResponse, Finish, Grounding, GroundingEngine, GroundingSource, Role,
    ToolCall, ToolChoice, Usage,
};
use super::error::LlmError;
use crate::attachment::AttachmentKind;
use super::wire;
use crate::llm::canonical::ChatRequest;

/// Anthropic の固有スキル（Spec 71）。値渡し（Gemini の `GeminiSkills` と同じ形）。
///
/// 値は [`crate::model::ModelTemplate::anthropic_web_search_active`] の判定済み。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AnthropicSkills {
    /// サーバー側の web 検索を tools の末尾に付けるか。
    pub web_search: bool,
}

/// web 検索の版（Spec 71 D1）。**基本版**を採る — 20260209 以降は既定で dynamic filtering の
/// コード実行のブロックが応答に混ざり、P0 (j) では入力 3 倍・引用 0 件だった。
const WEB_SEARCH_TOOL_TYPE: &str = "web_search_20250305";

/// web 検索のツール名。関数ツールに同じ名前が居ると 400（`Tool names must be unique.`）。
const WEB_SEARCH_TOOL_NAME: &str = "web_search";

/// 1 要求あたりの検索回数の上限（Spec 71 未決 1 の利用者裁定 2026-10-25）。
///
/// **1 要求の天井であって 1 ターンの天井ではない**（1 ターンは `5 × max_tool_iterations`）。
/// 送らない 1 要求は検索 17 回・入力 236,999・55 秒になった（P0 (e0)）— 検索 1 回の結果が
/// 約 1 万トークンなので、検索料だけでなく入力トークンと所要時間の天井でもある。
pub const ANTHROPIC_WEB_SEARCH_MAX_USES: u32 = 5;

/// canonical → Anthropic wire。
///
/// `cacheable_prefix_len > 0` のとき、system プロンプトを
/// 「安定部分 + 可変部分」の 2 ブロックに割り、安定部分の末尾に `cache_control` を打つ。
/// 分割位置は呼び出し側が文字数で宣言する契約にしてある。
///
/// `skills` が既定値（すべて偽）のとき、送る JSON は Spec 71 の前と 1 バイトも変わらない。
pub fn encode(req: &ChatRequest, skills: AnthropicSkills) -> wire::AnthropicRequest {
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
                // 応答のブロックの写しを持つ発話も「空」ではない（Spec 71 D3）—
                // 止まった応答（pause_turn）は本文も呼び出しも無いことがある。
                || m.replay.is_some()
        })
        .map(encode_message)
        .collect();

    let mut tools = req
        .tools
        .iter()
        .map(|t| {
            wire::AnthropicToolDef::Function(wire::AnthropicTool {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.parameters.clone(),
            })
        })
        .collect::<Vec<_>>();
    // サーバー側の web 検索は**末尾に 1 つ**（Spec 71 D1）。並びを固定するのは前方一致の
    // キャッシュのため（トグルを切り替えた 1 回だけ tools の層が書き直しになる）。
    if skills.web_search {
        if req.tools.iter().any(|t| t.name == WEB_SEARCH_TOOL_NAME) {
            // 今の命名規則（MCP は必ず `{サーバー}__{ツール}`）では起きない — 網。
            // 送ると 400 `tools: Tool names must be unique.`（P0 (k)）なので、黙って 400 に
            // せず検索だけを外して 1 行残す。
            crate::note!(
                "anthropic web search skipped: reason=name_clash name={WEB_SEARCH_TOOL_NAME}"
            );
        } else {
            tools.push(wire::AnthropicToolDef::WebSearch(wire::AnthropicWebSearchTool {
                kind: WEB_SEARCH_TOOL_TYPE,
                name: WEB_SEARCH_TOOL_NAME,
                max_uses: ANTHROPIC_WEB_SEARCH_MAX_USES,
            }));
        }
    }

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
        thinking: thinking_request(&req.model),
    }
}

/// 拡張思考の要求を組む（純関数。Spec 33 D3）。
///
/// **5 世代のモデルにだけ送る。** 旧世代へ `type: "adaptive"` を送ると
/// `400 adaptive thinking is not supported on this model`（実測 2026-08-10、
/// `claude-haiku-4-5`）。拒否されるのは `display` ではなく **thinking の形そのもの**。
///
/// **旧世代へ旧形式（`enabled` + `budget_tokens`）を送る道は採らない。**
/// 通ることは実測済み（272 字の要約が返った）だが、**旧世代は何も送らなければ
/// 思考しない**（同日実測 — `thinking` ブロックも `output_tokens_details` も
/// 出ない）。つまり旧形式を送るのは*見せる*ことではなく**思考を有効化して
/// コストを増やす**ことで、受け取りの Spec の目的から外れる。
///
/// 5 世代へ常に送るのは逆に**払っているものを見せるだけ**だから —
/// あちらは何も送らなくても思考しており（Spec 32 P4 で出力 2,048 トークン
/// 全部が思考の応答を観測）、`display` の既定 `"omitted"` が本文を伏せている。
/// 課金は `display` に依存しない（5 観測すべてで差がばらつきに埋もれた）。
///
/// 判定をモデル名で行うのは `reasoning_effort` と同じ**「同じワイヤ内の方言の
/// 吸収」**で、Spec 31 D2 が禁じた「ワイヤの選択に名前を使う」には当たらない。
fn thinking_request(model: &str) -> Option<wire::AnthropicThinking> {
    is_five_generation(model).then_some(wire::AnthropicThinking {
        kind: "adaptive",
        display: "summarized",
    })
}

/// 5 世代の Anthropic モデルか（`claude-sonnet-5` / `claude-opus-5` /
/// `claude-fable-5` と、その日付サフィックス付き）。
///
/// **確認できたものだけを真にする。** 一次資料では Opus 4.7 も `adaptive` を
/// 受けるとされるが**測っていない**ので偽に倒す — 外すと 400 でターンが落ちるが、
/// 内側へ倒しすぎても失うのは要約の表示だけ（#93 の逆側の用心）。
/// **世代は位置で決まる。部分一致で数えない。**
///
/// 最初にこれを `contains("-5-")` で書いて外した — `claude-haiku-4-5-20251001`
/// （4.5 世代）と `claude-3-5-sonnet` の両方に当たる。**版番号は語の中に
/// 何度も現れるので、「5 が含まれるか」は世代の判定にならない。**
///
/// 形は `claude-{系統}-5[-{日付}]`:
/// - 2 番目の区画が系統名（英字）
/// - 3 番目の区画がちょうど `"5"`
/// - それ以降があるなら日付（全部数字）
fn is_five_generation(model: &str) -> bool {
    let parts: Vec<&str> = model.split('-').collect();
    let [prefix, family, generation, rest @ ..] = parts.as_slice() else {
        return false;
    };
    *prefix == "claude"
        && family.chars().all(|c| c.is_ascii_alphabetic())
        && *generation == "5"
        && rest.iter().all(|s| s.chars().all(|c| c.is_ascii_digit()))
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
fn tool_tokens(tools: &[wire::AnthropicToolDef]) -> usize {
    tools
        .iter()
        .map(|t| match t {
            wire::AnthropicToolDef::Function(t) => {
                approx_tokens(&t.name)
                    + approx_tokens(&t.description)
                    // スキーマは JSON 文字列として数える（構造は問わない）。
                    + approx_tokens(&t.input_schema.to_string())
            }
            // サーバー側ツールの説明はプロバイダが足すので、こちらからは見えない。
            // 数えない = 少なめに見積もる側（この関数の既定の向き）。
            wire::AnthropicToolDef::WebSearch(t) => approx_tokens(t.name),
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
            wire::AnthropicRequestBlock::ToolResult { content, .. } => match content {
                wire::AnthropicToolResultContent::Text(text) => approx_tokens(text),
                // 中の画像は下の Image と同じ理由で数えない。
                wire::AnthropicToolResultContent::Blocks(blocks) => blocks
                    .iter()
                    .map(|block| match block {
                        wire::AnthropicToolResultBlock::Text { text } => approx_tokens(text),
                        wire::AnthropicToolResultBlock::Image { .. } => 0,
                    })
                    .sum(),
            },
            // 視覚トークンは寸法（28px パッチ数）からしか出ず、adapter は base64 しか
            // 持たない。数えない = **少なめに見積もる側へ倒す**（この関数の既定の向き。
            // 画像つきの発話はどのみち履歴に残らないので恒常的な過小にはならない）。
            wire::AnthropicRequestBlock::Image { .. }
            | wire::AnthropicRequestBlock::Document { .. } => 0,
            // 写しは JSON 文字列として数える。`encrypted_content` は base64 状の ASCII なので
            // 実際の検索結果のトークン数より少なめに出る（この関数の既定の向き）。
            wire::AnthropicRequestBlock::Raw(raw) => approx_tokens(&raw.to_string()),
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
        | wire::AnthropicRequestBlock::Image { cache_control, .. }
        | wire::AnthropicRequestBlock::Document { cache_control, .. } => {
            *cache_control = ephemeral();
        }
        // 写し（Spec 71 D3）。境界を打つのはこの関数で、オブジェクトへ鍵を書き足す。
        // `server_tool_use` / `web_search_tool_result` に打っても 200（P0 (f)）。
        wire::AnthropicRequestBlock::Raw(raw) => {
            if let Some(object) = raw.as_object_mut() {
                object.insert(
                    "cache_control".to_owned(),
                    serde_json::to_value(ephemeral()).unwrap_or(serde_json::Value::Null),
                );
            }
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
                content: tool_result_content(message),
                cache_control: None,
            }],
        };
    }

    // 写しを持つ assistant の発話（Spec 71 D3）。**組み立てを丸ごと飛ばし、受け取った順に
    // 並べる** — 添付・本文・`tool_calls` からは作らない（固定順の組み立てでは
    // `server_tool_use` の位置を保てず、`tool_use` を二重に作ることになる）。
    // `tool_calls` はターンの層（実行・RepeatGuard・計器）のために残るが、ここでは読まない。
    if message.role == Role::Assistant
        && let Some(serde_json::Value::Array(blocks)) = &message.replay
    {
        return wire::AnthropicMessage {
            role: "assistant",
            content: blocks
                .iter()
                .cloned()
                .map(wire::AnthropicRequestBlock::Raw)
                .collect(),
        };
    }

    let mut content = Vec::new();
    // 添付はテキストより前に置く（公式の推奨。Spec 23 P2）。
    // **音声と動画の variant はこのワイヤに無い**（400 の受理集合列挙で確認。
    // Spec 36 P0）ので落ちる。正面の門は送信入口の `carries`（D2）。
    for attachment in &message.attachments {
        let source = wire::AnthropicBase64Source {
            kind: "base64",
            media_type: attachment.media_type.as_str().to_owned(),
            data: attachment.data.clone(),
        };
        let block = match attachment.kind() {
            AttachmentKind::Image => wire::AnthropicRequestBlock::Image {
                source,
                cache_control: None,
            },
            AttachmentKind::Pdf => wire::AnthropicRequestBlock::Document {
                source,
                cache_control: None,
            },
            AttachmentKind::Audio | AttachmentKind::Video => continue,
        };
        content.push(block);
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

/// ツール結果の本文（Spec 70 D1 = ネイティブ）。
///
/// 画像は `tool_result` の `content` の配列へ、**テキストより前**に置く（P0 で通った並び。
/// 利用者の発話の添付と同じ向き）。運ぶのは画像だけ — ツール結果に他の種別は載らない
/// （`mcp_image_result_contract` の凍結 1）。**画像が無ければ素の文字列**のまま。
fn tool_result_content(message: &ChatMessage) -> wire::AnthropicToolResultContent {
    let mut blocks: Vec<wire::AnthropicToolResultBlock> = message
        .attachments
        .iter()
        .filter(|a| a.kind() == AttachmentKind::Image)
        .map(|a| wire::AnthropicToolResultBlock::Image {
            source: wire::AnthropicBase64Source {
                kind: "base64",
                media_type: a.media_type.as_str().to_owned(),
                data: a.data.clone(),
            },
        })
        .collect();
    if blocks.is_empty() {
        return wire::AnthropicToolResultContent::Text(message.content.clone());
    }
    // 空のテキストブロックは拒否されるので、本文があるときだけ積む。
    if !message.content.is_empty() {
        blocks.push(wire::AnthropicToolResultBlock::Text {
            text: message.content.clone(),
        });
    }
    wire::AnthropicToolResultContent::Blocks(blocks)
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
            // **`prompt` の内数**（Spec 40）。rev1 まではこの値が `prompt` へ足し込まれる
            // だけで独立した行き先を持たず、読み取り 0.1× と書き込み 2.0× が同じ袋に
            // 入っていた（`CACHE_TTL = "1h"`）。
            cache_write: u.cache_creation_input_tokens,
            // 合計の**部分集合**。内訳を返さない応答では 0（合計だけが立つ）。
            // **ここで `cache_write` へ丸めない** — 不変条件は実データで確かめる側で、
            // decode で強制すると検定が同語反復になる。
            cache_write_1h: u
                .cache_creation
                .as_ref()
                .map_or(0, |c| c.ephemeral_1h_input_tokens),
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
        // サーバー側ツールのループが止まった（Spec 71 D4）。終了ではない。
        Some("pause_turn") => Finish::Paused,
        _ => Finish::Other,
    };
    // 課金の単位（Spec 71 D6）。検索を実行しなかった応答では欄ごと無い（0 と読む）。
    let search_requests = resp
        .usage
        .as_ref()
        .and_then(|u| u.server_tool_use.as_ref())
        .map_or(0, |s| s.web_search_requests);

    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut reasoning_summary: Vec<String> = Vec::new();
    let mut dropped: Vec<&'static str> = Vec::new();
    // web 検索（Spec 71 D6）。写しの材料と計器の数。
    let mut search = SearchTally::default();
    let mut replay_blocks: Vec<serde_json::Value> = Vec::new();
    let mut has_server_block = false;
    for wire::AnthropicResponseBlock { block, raw } in resp.content {
        // 写しには思考のブロックを入れない（Spec 71 D3。P0 (c3) で 200）。
        let is_thinking = matches!(
            block,
            wire::AnthropicContentBlock::Thinking { .. }
                | wire::AnthropicContentBlock::RedactedThinking
        );
        if !is_thinking {
            replay_blocks.push(raw);
        }
        match block {
            wire::AnthropicContentBlock::Text {
                text: chunk,
                citations,
            } => {
                text.push_str(&chunk);
                for citation in citations {
                    search.cite(citation);
                }
            }
            wire::AnthropicContentBlock::ServerToolUse { id, name, input } => {
                has_server_block = true;
                search.call(id, &name, &input);
            }
            wire::AnthropicContentBlock::WebSearchToolResult {
                tool_use_id,
                content,
            } => {
                has_server_block = true;
                search.result(tool_use_id, &content);
            }
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
            // 要約は受け取る（Spec 33）。**答えの本文へは混ぜない** —
            // 内部の独白が答えとして表示される。`dropped` へは従来どおり
            // 数え上げる（本文でないものが来た事実は、要約が取れても変わらない）。
            //
            // **0 字は入れない**。`display` を送っていない要求と、思考が短い回で
            // 起きる（`reasoning_summary` 契約の凍結 2）。
            wire::AnthropicContentBlock::Thinking { thinking } => {
                if !thinking.is_empty() {
                    reasoning_summary.push(thinking);
                }
                dropped.push("thinking");
            }
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

    let paused = finish == Finish::Paused;
    // 計器（Spec 71 D6）。検索を含む応答か、止まった応答でだけ出す。
    // **本文と検索語は書かない**（`failures.md` #71）。`agent=` は無い（decode 層は個体を知らない）。
    if has_server_block || paused {
        crate::note!(
            "anthropic search: calls={} results={} cited={} requests={} errors={} deferred={} paused={}",
            search.calls,
            search.results,
            search.cited,
            search_requests,
            if search.errors.is_empty() {
                "-".to_owned()
            } else {
                search.errors.join("+")
            },
            search.deferred(),
            u8::from(paused),
        );
    }

    // 写しを作るのは、サーバー側ツールのブロックが在るか止まった応答のときだけ（D3）。
    // それ以外は None — 送信バイトは従来と 1 バイトも変わらない。
    let replay = (has_server_block || paused).then_some(serde_json::Value::Array(replay_blocks));

    Ok(ChatResponse {
        text: if text.is_empty() { None } else { Some(text) },
        tool_calls,
        finish,
        usage,
        grounding: search.into_grounding(),
        reasoning_summary,
        replay,
    })
}

/// web 検索のブロックを 1 応答ぶん数える（Spec 71 D6）。出典と計器の材料。
#[derive(Debug, Default)]
struct SearchTally {
    /// `server_tool_use` の数（保留を含む。課金の数 `requests` とは 1 対 1 にならない）。
    calls: usize,
    /// 検索結果（`web_search_result`）の件数。
    results: usize,
    /// web 検索の引用（`web_search_result_location`）の件数。
    cited: usize,
    /// 失敗の `error_code`（現れた順）。
    errors: Vec<String>,
    /// 呼び出しの ID（保留の判定に使う）。
    call_ids: Vec<String>,
    /// 結果が届いた呼び出しの ID。
    result_ids: Vec<String>,
    /// 検索語（`Grounding.queries`）。
    queries: Vec<String>,
    /// 引用された出典（`Grounding.sources`。URL で重複を落とす）。
    sources: Vec<GroundingSource>,
}

impl SearchTally {
    fn call(&mut self, id: String, name: &str, input: &serde_json::Value) {
        self.calls += 1;
        self.call_ids.push(id);
        if name == WEB_SEARCH_TOOL_NAME
            && let Some(query) = input.get("query").and_then(serde_json::Value::as_str)
            && !self.queries.iter().any(|q| q == query)
        {
            self.queries.push(query.to_owned());
        }
    }

    /// `content` は結果の配列か失敗のオブジェクト（P0 (h)）。どちらでもターンは止めない。
    fn result(&mut self, tool_use_id: String, content: &serde_json::Value) {
        self.result_ids.push(tool_use_id);
        match content {
            serde_json::Value::Array(items) => self.results += items.len(),
            other => {
                let code = other
                    .get("error_code")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown");
                self.errors.push(code.to_owned());
            }
        }
    }

    /// 出典は**引用されたものだけ**（D6）。`title` が空なら URL を表題にする。
    fn cite(&mut self, citation: wire::AnthropicCitation) {
        if citation.kind != "web_search_result_location" || citation.url.is_empty() {
            return;
        }
        self.cited += 1;
        if self.sources.iter().any(|s| s.uri == citation.url) {
            return;
        }
        let title = if citation.title.is_empty() {
            citation.url.clone()
        } else {
            citation.title
        };
        self.sources.push(GroundingSource {
            uri: citation.url,
            title,
        });
    }

    /// 結果の無い `server_tool_use` の数（村のツールと同じ周の保留。D5）。
    fn deferred(&self) -> usize {
        self.call_ids
            .iter()
            .filter(|id| !self.result_ids.contains(id))
            .count()
    }

    /// 検索が無ければ空の接地（Spec 71 の前と同じ）。
    fn into_grounding(self) -> Grounding {
        if self.queries.is_empty() && self.sources.is_empty() {
            return Grounding::default();
        }
        Grounding {
            queries: self.queries,
            sources: self.sources,
            engine: GroundingEngine::Anthropic,
        }
    }
}

/// エラー本文から再試行の材料を取り出す（Spec 52 D1。純関数）。
///
/// Anthropic の封筒 `{"type": "error", "error": {"type": "overloaded_error", "message": …}}`。
/// `code` は `error.type`。**表に載せている値は無い**（`billing_error` は文書にも実機にも
/// 当てていない。実機で出たのは 529 の `overloaded_error` だけ）ので、分類は status 既定へ
/// 落ち、`type` は計器の `code=` に出るだけ。本文に待ち時間は無い。
pub fn error_signal(body: &str) -> super::retry::ErrorSignal {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return super::retry::ErrorSignal::default();
    };
    super::retry::ErrorSignal {
        code: value["error"]["type"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned),
        retry_after: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::canonical::{ChatMessage, ToolSpec};
    use serde_json::json;

    /// 固有スキルなし（Spec 71 の前と同じ送信バイト）。
    const OFF: AnthropicSkills = AnthropicSkills { web_search: false };

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
        let json = serde_json::to_value(encode(&req, OFF)).unwrap();

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

        let w = encode(&req, OFF);

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
            serde_json::to_string(&encode(&req, OFF)).unwrap(),
            r#"{"model":"claude-opus-5","max_tokens":512,"system":[{"type":"text","text":"s"}],"messages":[{"role":"user","content":[{"type":"text","text":"こんにちは"}]}],"thinking":{"type":"adaptive","display":"summarized"}}"#
        );
    }

    /// **Spec 33 が golden を 1 度だけ動かした。** 5 世代へ `thinking` を送る
    /// ようになったため。**旧世代の形は 1 バイトも動いていない**ことを、
    /// 同じ golden の形で別に留める — 「送らない」は省略であって、
    /// 空オブジェクトを送ることではない。
    #[test]
    fn an_older_model_keeps_the_pre_spec33_bytes() {
        let req = ChatRequest::plain(
            "claude-haiku-4-5-20251001",
            vec![ChatMessage::system("s"), ChatMessage::user("こんにちは")],
            512,
        );
        assert_eq!(
            serde_json::to_string(&encode(&req, OFF)).unwrap(),
            r#"{"model":"claude-haiku-4-5-20251001","max_tokens":512,"system":[{"type":"text","text":"s"}],"messages":[{"role":"user","content":[{"type":"text","text":"こんにちは"}]}]}"#
        );
    }

    /// 添付は image ブロックになり、**テキストより前**に置かれる（Spec 23 P2）。
    #[test]
    fn attachments_become_image_blocks_before_text() {
        use crate::llm::canonical::{PromptAttachment, PromptMediaType};
        let req = ChatRequest::plain(
            "claude-opus-5",
            vec![ChatMessage::user_with_attachments(
                "何が見える？",
                vec![PromptAttachment::new(PromptMediaType::Webp, "QUJD")],
            )],
            512,
        );
        let json = serde_json::to_value(&encode(&req, OFF).messages).unwrap();
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
        use crate::llm::canonical::{PromptAttachment, PromptMediaType};
        let req = ChatRequest::plain(
            "claude-opus-5",
            vec![ChatMessage::user_with_attachments(
                "",
                vec![PromptAttachment::new(PromptMediaType::Webp, "QUJD")],
            )],
            512,
        );
        let w = encode(&req, OFF);
        assert_eq!(w.messages.len(), 1, "画像だけの発話は落ちない");
        let json = serde_json::to_value(&w.messages[0].content).unwrap();
        let blocks = json.as_array().unwrap();
        assert_eq!(blocks.len(), 1, "空テキストも「発言なし」も入らない: {json}");
        assert_eq!(blocks[0]["type"], "image");
    }

    #[test]
    fn system_is_lifted_out_of_messages() {
        let w = encode(&request(0), OFF);

        assert_eq!(w.system.len(), 1);
        assert_eq!(w.system[0].text, "あなたは計画立案担当です。");
        assert_eq!(w.messages.len(), 1, "system は messages に残らない");
        assert_eq!(w.messages[0].role, "user");
    }

    #[test]
    fn a_short_prefix_is_not_marked_for_caching() {
        // 最小長を下回る安定部分にキャッシュ指示を出しても再利用されない。
        // 効かない指示は送らない。
        let w = encode(&request(6), OFF);

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

        let w = encode(&req, OFF);

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

        let control = encode(&req, OFF).system[0].cache_control.clone().unwrap();
        assert_eq!(control.kind, "ephemeral");
        assert_eq!(control.ttl, Some("1h"));

        // ワイヤ形も確認する。ttl が抜けると黙って 5 分へ戻る。
        let json = serde_json::to_value(encode(&req, OFF)).unwrap();
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

        let w = encode(&req, OFF);

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

        let w = encode(&req, OFF);

        assert_eq!(w.system.len(), 1, "割る先が無いので 1 ブロック");
        assert!(w.system[0].cache_control.is_some(), "それでもキャッシュは要求する");
    }

    /// 短すぎるプレフィックスには要求しないこと（効かない指示は送らない）。
    #[test]
    fn a_tiny_prompt_with_few_tools_does_not_request_caching() {
        let mut req = request(0);
        req.messages[0] = ChatMessage::system("短い指示可変");
        req.cacheable_prefix_len = 4;

        let w = encode(&req, OFF);

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

        let w = encode(&req, OFF);

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

        let json = serde_json::to_value(encode(&req, OFF)).unwrap();
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

        let json = serde_json::to_value(encode(&req, OFF)).unwrap().to_string();

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

        let json = serde_json::to_value(encode(&req, OFF)).unwrap().to_string();

        assert_eq!(
            json.matches(r#""cache_control""#).count(),
            2,
            "system の安定部分と履歴の末尾の 2 箇所: {json}"
        );
    }

    #[test]
    fn tool_schema_uses_input_schema_key() {
        let json = serde_json::to_value(encode(&request(0), OFF)).unwrap();

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

        let json = serde_json::to_value(encode(&req, OFF)).unwrap();
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

    /// 5 世代へは `thinking` を送り、旧世代へは**キーごと省く**（Spec 33 D3）。
    ///
    /// 省いた形が Spec 33 より前と**バイト等価**であることまで見る —
    /// 旧世代の村のワイヤを 1 バイトも動かさないのが「送らない」の意味。
    #[test]
    fn thinking_is_requested_only_for_five_generation_models() {
        for model in [
            "claude-sonnet-5",
            "claude-opus-5",
            "claude-fable-5",
            "claude-sonnet-5-20260101",
        ] {
            let req = ChatRequest::plain(model, vec![ChatMessage::user("問い")], 1_024);
            let json = serde_json::to_value(encode(&req, OFF)).unwrap();
            assert_eq!(
                json["thinking"],
                serde_json::json!({ "type": "adaptive", "display": "summarized" }),
                "{model} は 5 世代"
            );
        }

        for model in ["claude-haiku-4-5-20251001", "claude-3-5-sonnet", "gpt-4o"] {
            let req = ChatRequest::plain(model, vec![ChatMessage::user("問い")], 1_024);
            let json = serde_json::to_value(encode(&req, OFF)).unwrap();
            assert!(
                json.get("thinking").is_none(),
                "{model} へは thinking のキーごと送らない（adaptive は 400 になる）"
            );
        }
    }

    /// 思考の要約を受け取る（Spec 33）。**答えの本文へは混ぜない。**
    ///
    /// 落としたことは従来どおり数える — 要約が取れても、
    /// 「本文でないブロックが来た」事実は変わらない。
    #[test]
    fn thinking_text_is_captured_as_a_summary_not_as_the_answer() {
        let raw = serde_json::json!({
            "content": [
                { "type": "thinking", "thinking": "I'm working through a logic puzzle.",
                  "signature": "Eo8CC…" },
                { "type": "text", "text": "答えは B です" }
            ],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 10, "output_tokens": 100,
                       "output_tokens_details": { "thinking_tokens": 80 } }
        });
        let out = decode(serde_json::from_value::<wire::AnthropicResponse>(raw).unwrap()).unwrap();

        assert_eq!(out.text.as_deref(), Some("答えは B です"), "本文は混ざらない");
        assert_eq!(
            out.reasoning_summary,
            vec!["I'm working through a logic puzzle.".to_owned()]
        );
        assert_eq!(out.usage.reasoning, 80);
    }

    /// **0 字の要約は入れない**（`reasoning_summary` 契約の凍結 2）。
    ///
    /// `display` を送っていない要求（5 世代の既定は `"omitted"`）と、
    /// 思考が短い回で起きる。実測では**ツール併用の回がほぼこれ**。
    #[test]
    fn an_omitted_thinking_block_yields_no_summary_but_is_still_counted() {
        let raw = serde_json::json!({
            "content": [
                { "type": "thinking", "thinking": "", "signature": "EoYxCok…" },
                { "type": "text", "text": "はい" }
            ],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 10, "output_tokens": 50,
                       "output_tokens_details": { "thinking_tokens": 34 } }
        });
        let out = decode(serde_json::from_value::<wire::AnthropicResponse>(raw).unwrap()).unwrap();

        assert!(out.reasoning_summary.is_empty(), "0 字は列へ入れない");
        assert_eq!(out.usage.reasoning, 34, "数は取れている（中身が無いだけ）");
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
        assert_eq!(resp.usage.cache_write, 0);
        assert_eq!(resp.usage.cache_write_1h, 0);
    }

    /// **キャッシュ書き込みが独立して取れる**（Spec 40 P1）。
    ///
    /// rev1 まで `cache_creation_input_tokens` は `prompt` へ足し込まれるだけで、
    /// 独立した行き先を持っていなかった。読み取り 0.1× に対し書き込みは 2.0×
    /// （`CACHE_TTL = "1h"`）なので、畳むと外の単価表で金額が狂う。
    ///
    /// **`prompt` が 3 項の和のままであることを同じテストで留める** — 欄を足した
    /// ときに `prompt` の意味を動かすと、8 日ぶんの実測と比較できなくなる。
    #[test]
    fn decode_splits_cache_write_from_prompt_and_keeps_the_sum() {
        let raw = r#"{
            "content": [{ "type": "text", "text": "ok" }],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 10, "output_tokens": 5,
                       "cache_read_input_tokens": 90,
                       "cache_creation_input_tokens": 400,
                       "cache_creation": { "ephemeral_5m_input_tokens": 150,
                                           "ephemeral_1h_input_tokens": 250 } }
        }"#;
        let resp = decode(serde_json::from_str(raw).unwrap()).unwrap();

        assert_eq!(resp.usage.cache_write, 400, "合計は平坦な欄から");
        assert_eq!(resp.usage.cache_write_1h, 250, "1h は内訳から");
        // 5 分ぶんは欄を持たず引き算で出す（独立な数は 2 つ）。
        assert_eq!(resp.usage.cache_write - resp.usage.cache_write_1h, 150);
        assert!(resp.usage.cache_write_1h <= resp.usage.cache_write);
        // **`prompt` は 3 項の和のまま**（10 + 90 + 400）。
        assert_eq!(resp.usage.prompt, 500);
        // 素の未キャッシュはこの引き算で出る。
        assert_eq!(
            resp.usage.prompt - resp.usage.cache_read - resp.usage.cache_write,
            10
        );
    }

    /// **内訳が返らない応答では、合計だけが立つ**（Spec 40 P1）。
    ///
    /// `cache_creation` を返さない世代・応答があるので、**内訳の欠落を
    /// 「書き込みが無かった」と読まない**。
    #[test]
    fn decode_keeps_the_cache_write_total_when_the_ttl_breakdown_is_absent() {
        let raw = r#"{
            "content": [{ "type": "text", "text": "ok" }],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 10, "output_tokens": 5,
                       "cache_read_input_tokens": 0,
                       "cache_creation_input_tokens": 400 }
        }"#;
        let resp = decode(serde_json::from_str(raw).unwrap()).unwrap();

        assert_eq!(resp.usage.cache_write, 400);
        assert_eq!(resp.usage.cache_write_1h, 0, "内訳が無ければ 0");
        assert_eq!(resp.usage.prompt, 410);
    }

    /// ツールを呼んで結果が返った 3 発話（Spec 70）。`with_image` なら結果に画像と代替の本文が付く。
    fn tool_image_request(with_image: bool) -> ChatRequest {
        use crate::llm::canonical::{PromptAttachment, PromptMediaType};
        let mut result = ChatMessage::tool_result("call_1", "camera", r#"{"width":8}"#);
        if with_image {
            result.attachments = vec![PromptAttachment::new(PromptMediaType::Jpeg, "SU1H")];
            result.attachment_caption = Some("（camera の結果の画像 1 枚）".into());
        }
        let call = ToolCall {
            id: "call_1".into(),
            name: "camera".into(),
            args: json!({}),
            extra: None,
        };
        ChatRequest::plain(
            "claude-sonnet-5",
            vec![
                ChatMessage::user("撮って"),
                ChatMessage::assistant_tool_calls("", vec![call]),
                result,
            ],
            256,
        )
    }

    /// 画像は `tool_result` の中へ、テキストより前（Spec 70 D1 = ネイティブ）。
    #[test]
    fn a_tool_result_image_goes_inside_the_tool_result() {
        let json = serde_json::to_value(encode(&tool_image_request(true), OFF)).unwrap();
        let messages = json["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3, "画像のために発話を足さない");
        let result = &messages[2]["content"][0];
        assert_eq!(result["type"], "tool_result");
        assert_eq!(
            result["content"],
            json!([
                {"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":"SU1H"}},
                {"type":"text","text":r#"{"width":8}"#},
            ])
        );
        assert!(
            !json.to_string().contains("camera の結果の画像"),
            "代替の本文はネイティブのワイヤでは使わない"
        );
    }

    /// 画像の無いツール結果は Spec 70 の前と同じ素の文字列。
    #[test]
    fn a_tool_result_without_images_keeps_a_plain_string() {
        let json = serde_json::to_value(encode(&tool_image_request(false), OFF)).unwrap();
        assert_eq!(json["messages"][2]["content"][0]["content"], r#"{"width":8}"#);
    }

    #[test]
    fn error_signal_reads_the_error_type() {
        let s = error_signal(
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"},"request_id":"req_1"}"#,
        );
        assert_eq!(s.code.as_deref(), Some("overloaded_error"));
        assert_eq!(s.retry_after, None);
        assert_eq!(error_signal("").code, None);
    }

    // ---- Spec 71: web 検索（サーバー側ツール）と写し ----

    const WEB_ON: AnthropicSkills = AnthropicSkills { web_search: true };

    fn decode_json(raw: serde_json::Value) -> ChatResponse {
        decode(serde_json::from_value::<wire::AnthropicResponse>(raw).unwrap()).unwrap()
    }

    /// D1 — 検索は tools の**末尾に 1 つ**。関数ツールの並びと形は変わらない。
    /// OFF のときは tools に何も足さない（golden の側は既存の
    /// `encoding_without_attachments_matches_the_golden_bytes` が留める）。
    #[test]
    fn web_search_rides_at_the_end_of_tools_only_when_on() {
        let off = serde_json::to_value(encode(&request(0), OFF)).unwrap();
        assert_eq!(off["tools"].as_array().unwrap().len(), 1);

        let on = serde_json::to_value(encode(&request(0), WEB_ON)).unwrap();
        let tools = on["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0], off["tools"][0], "関数ツールは 1 バイトも変わらない");
        assert_eq!(
            tools[1],
            json!({ "type": "web_search_20250305", "name": "web_search", "max_uses": 5 })
        );
    }

    /// D1 — 関数ツールに `web_search` が居たら送らない（重複は 400 — P0 (k)）。
    #[test]
    fn a_function_named_web_search_keeps_the_server_tool_out() {
        let mut req = request(0);
        req.tools[0].name = "web_search".into();
        let json = serde_json::to_value(encode(&req, WEB_ON)).unwrap();
        let tools = json["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert!(tools[0].get("type").is_none(), "残るのは関数ツールだけ");
    }

    /// P0 (a) の実物を縮めた形。検索 → 結果 → 思考 → 引用つきの本文（0 字の本文と
    /// 0 字の `cited_text` を含む）。
    fn searched() -> serde_json::Value {
        json!({
            "content": [
                { "type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search",
                  "input": { "query": "日銀 政策金利 2026年" } },
                { "type": "web_search_tool_result", "tool_use_id": "srvtoolu_1",
                  "content": [
                    { "type": "web_search_result", "url": "https://a.test/1", "title": "A",
                      "page_age": "36 days ago", "encrypted_content": "EqgfCio" },
                    { "type": "web_search_result", "url": "https://b.test/2", "title": "B",
                      "page_age": "85 days ago", "encrypted_content": "EqgfCiq" }
                  ] },
                { "type": "thinking", "thinking": "summary", "signature": "Eo8B" },
                { "type": "text", "text": "政策金利は" },
                { "type": "text", "text": "0.75% です。",
                  "citations": [
                    { "type": "web_search_result_location", "url": "https://a.test/1",
                      "title": "A", "cited_text": "0.75%", "encrypted_index": "Eo8BC" }
                  ] },
                { "type": "text", "text": "",
                  "citations": [
                    { "type": "web_search_result_location", "url": "https://a.test/1",
                      "title": "", "cited_text": "", "encrypted_index": "Eo8BD" },
                    { "type": "web_search_result_location", "url": "https://b.test/2",
                      "title": "", "cited_text": "", "encrypted_index": "Eo8BE" }
                  ] }
            ],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 11943, "output_tokens": 711,
                       "server_tool_use": { "web_search_requests": 1, "web_fetch_requests": 0 } }
        })
    }

    /// D6 — 検索語・出典（引用されたものだけ・URL で重複を落とす・表題が空なら URL）・
    /// エンジン。本文は連結する。
    #[test]
    fn decode_reads_queries_and_cited_sources() {
        let out = decode_json(searched());
        assert_eq!(out.text.as_deref(), Some("政策金利は0.75% です。"));
        assert_eq!(out.grounding.engine, GroundingEngine::Anthropic);
        assert_eq!(out.grounding.queries, vec!["日銀 政策金利 2026年".to_owned()]);
        let sources: Vec<(&str, &str)> = out
            .grounding
            .sources
            .iter()
            .map(|s| (s.uri.as_str(), s.title.as_str()))
            .collect();
        assert_eq!(
            sources,
            vec![("https://a.test/1", "A"), ("https://b.test/2", "https://b.test/2")]
        );
        assert_eq!(out.reasoning_summary, vec!["summary".to_owned()]);
    }

    /// D3 — 写しは**受け取った順・受け取った欄のまま**で、思考のブロックだけを落とす
    /// （P0 (c3)）。`encrypted_content` / `encrypted_index` が残っていること。
    #[test]
    fn the_replay_keeps_blocks_verbatim_without_thinking() {
        let raw = searched();
        let out = decode_json(raw.clone());
        let replay = out.replay.expect("検索のブロックがあれば写しを作る");
        let expected: Vec<serde_json::Value> = raw["content"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|b| b["type"] != "thinking")
            .cloned()
            .collect();
        assert_eq!(replay, serde_json::Value::Array(expected));
    }

    /// D3 — 検索の無い応答は写しを作らない（送信バイトは従来のまま）。
    #[test]
    fn a_response_without_server_blocks_has_no_replay() {
        let out = decode_json(json!({
            "content": [{ "type": "text", "text": "こんにちは" }],
            "stop_reason": "end_turn"
        }));
        assert_eq!(out.replay, None);
        assert!(out.grounding.is_empty());
    }

    /// D4 — `pause_turn` は `Paused`。**サーバー側ツールのブロックが無くても写しを作る**
    /// （rev2 — 査読 4）。
    #[test]
    fn pause_turn_is_paused_and_always_carries_a_replay() {
        let out = decode_json(json!({
            "content": [{ "type": "text", "text": "調べています" }],
            "stop_reason": "pause_turn"
        }));
        assert_eq!(out.finish, Finish::Paused);
        assert_eq!(
            out.replay,
            Some(json!([{ "type": "text", "text": "調べています" }]))
        );
    }

    /// D6 — 失敗は 200 の中のオブジェクト（P0 (h)）。読めて、ターンを止めない。
    #[test]
    fn a_failed_search_is_read_as_an_object() {
        let out = decode_json(json!({
            "content": [
                { "type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search",
                  "input": { "query": "東京 気温" } },
                { "type": "server_tool_use", "id": "srvtoolu_2", "name": "web_search",
                  "input": { "query": "NY 気温" } },
                { "type": "web_search_tool_result", "tool_use_id": "srvtoolu_1", "content": [] },
                { "type": "web_search_tool_result", "tool_use_id": "srvtoolu_2",
                  "content": { "type": "web_search_tool_result_error",
                               "error_code": "max_uses_exceeded" } },
                { "type": "text", "text": "片方しか調べられませんでした" }
            ],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 10, "output_tokens": 5,
                       "server_tool_use": { "web_search_requests": 1 } }
        }));
        assert_eq!(out.finish, Finish::Stop);
        assert_eq!(out.grounding.queries.len(), 2);
        assert!(out.replay.is_some());
    }

    /// D3 / D5 — 写しを持つ assistant の発話は**写しのブロックだけ**を並べる。
    /// `tool_calls` から `tool_use` を二重に作らず、写しの中の `tool_use` の ID 集合は
    /// decode が返した `tool_calls` の ID 集合と同じ。
    #[test]
    fn a_replayed_message_is_sent_verbatim_and_tool_use_is_not_doubled() {
        let raw = json!({
            "content": [
                { "type": "tool_use", "id": "toolu_1", "name": "emit_plan", "input": {} },
                { "type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search",
                  "input": { "query": "東京 天気" } }
            ],
            "stop_reason": "tool_use"
        });
        let out = decode_json(raw.clone());
        let replay_ids: Vec<&str> = out
            .replay
            .as_ref()
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .filter(|b| b["type"] == "tool_use")
            .map(|b| b["id"].as_str().unwrap())
            .collect();
        let call_ids: Vec<&str> = out.tool_calls.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(replay_ids, call_ids);

        let mut req = request(0);
        req.messages.push(
            ChatMessage::assistant_tool_calls(String::new(), out.tool_calls.clone())
                .with_replay(out.replay.clone()),
        );
        req.messages.push(ChatMessage::tool_result("toolu_1", "emit_plan", "ok"));
        let json = serde_json::to_value(encode(&req, WEB_ON)).unwrap();
        assert_eq!(json["messages"][1]["content"], raw["content"]);
    }

    /// D3 — 本文も呼び出しも無い写しだけの発話（止まった応答）を「空」として落とさない。
    #[test]
    fn a_replay_only_message_is_not_dropped_as_empty() {
        let mut req = request(0);
        req.messages.push(
            ChatMessage::assistant("").with_replay(Some(json!([
                { "type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search",
                  "input": { "query": "q" } }
            ]))),
        );
        let w = encode(&req, WEB_ON);
        assert_eq!(w.messages.len(), 2);
        assert_eq!(w.messages[1].role, "assistant");
    }

    /// D3 — キャッシュの境界は写しのブロックにも打つ（P0 (f) で 200）。
    #[test]
    fn the_breakpoint_lands_on_a_replayed_block() {
        let mut req = request(0);
        req.messages = vec![
            ChatMessage::system("短い指示"),
            ChatMessage::user(format!("調べて{}", "履歴".repeat(3_000))),
            ChatMessage::assistant("").with_replay(Some(json!([
                { "type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search",
                  "input": { "query": "q" } }
            ]))),
        ];
        let json = serde_json::to_value(encode(&req, WEB_ON)).unwrap();
        assert_eq!(
            json["messages"][1]["content"][0]["cache_control"],
            json!({ "type": "ephemeral", "ttl": "1h" })
        );
    }
}
