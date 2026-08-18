//! Gemini ネイティブ adapter（`generateContent`）。
//!
//! canonical ⇄ wire の **encode / decode 純関数**のみを持つ。
//! HTTP・認証・再試行は [`super::client`] が担当する。
//!
//! ## なぜ OpenAI 互換層と別に要るのか
//!
//! Gemini は OpenAI 互換の口（`/chat/completions`）も持っており、関数呼び出しだけなら
//! そちらで足りる。だが **Google 検索による接地は互換層に露出していない** —
//! `tools: [{"type":"google_search"}]` は `400 Invalid tool type: google_search` で
//! 拒否される（実測 2026-07-29）。接地を使うにはネイティブ経路が要る。
//!
//! ## 互換層との差で効くところ
//!
//! - system は `contents` に混ぜず `systemInstruction` へ分ける
//! - role は `user` / `model` の 2 値。ツール結果も **`user` ロール**の part として積む
//! - `args` は最初から JSON オブジェクト（OpenAI 系の「文字列」方言が無い）
//! - part は `type` タグを持たず、どのキーがあるかで種別が決まる
//! - **`finishReason: "STOP"` は終了を意味しない**（関数呼び出し時も STOP が来る）

use serde_json::{Value, json};

use super::canonical::{
    ChatMessage, ChatRequest, ChatResponse, Finish, Grounding, GroundingSource, Role, ToolCall,
    ToolChoice, Usage,
};
use super::error::LlmError;
use super::wire;

/// Gemini の `Schema` が受け付けるキー。**これ以外は送ると 400 になる。**
///
/// `parameters` は JSON Schema ではなく **OpenAPI 3.0 の部分集合**で、
/// 未知キーを黙って無視せず `Unknown name "..." Cannot find field` で弾く。
///
/// 除外リストではなく**許可リスト**にしてあるのが要点。同梱ツールが出す
/// `additionalProperties` だけなら列挙で足りるが、MCP ツールのスキーマは
/// 接続先のサーバーが書くもので、こちらから中身を制限できない
/// （実際に `$schema` 付きのものが来て 400 になった）。知らないキーが
/// 増えるたびに落ちる形にしてはいけない。
const SCHEMA_KEYS: &[&str] = &[
    "type",
    "format",
    "title",
    "description",
    "nullable",
    "enum",
    "items",
    "properties",
    "required",
    "minItems",
    "maxItems",
    "minProperties",
    "maxProperties",
    "minimum",
    "maximum",
    "minLength",
    "maxLength",
    "pattern",
    "example",
    "default",
    "anyOf",
    "propertyOrdering",
];

/// 思考署名を [`ToolCall::extra`] に載せるときのキー。
///
/// `extra` はプロバイダ固有の不透明枠で、**decode した adapter だけが encode で読み戻す**
/// という契約。OpenAI 互換経路が同じ枠に `{"google":{"thought_signature":...}}` を
/// 入れるのとは形が違うが、読み手が同じなので衝突しない。
const THOUGHT_SIGNATURE: &str = "thoughtSignature";

/// canonical → Gemini wire。
///
/// `google_search` が真なら、関数宣言とは**別要素**として `tools` に積む。
/// 両者は併用でき、検索 → 関数呼び出しが 1 応答の中で連鎖する（実測 2026-07-29）。
///
/// `use_tools` に相当するフォールバック（スキーマをプロンプトに載せる経路）は持たない。
/// ネイティブ経路は関数呼び出しを常に解釈できるため、Anthropic adapter と同じ扱いにする。
pub fn encode(req: &ChatRequest, google_search: bool) -> wire::GeminiRequest {
    // system は contents に混ぜられない。複数あれば改行で畳んで 1 つにする。
    let system_text = req
        .messages
        .iter()
        .filter(|m| m.role == Role::System)
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");

    let system_instruction = (!system_text.trim().is_empty()).then(|| wire::GeminiContent {
        role: None,
        parts: vec![wire::GeminiPart {
            text: Some(system_text),
            ..Default::default()
        }],
    });

    let contents = req
        .messages
        .iter()
        .filter(|m| m.role != Role::System)
        .map(encode_message)
        .collect();

    let mut tools = Vec::new();
    if google_search {
        tools.push(wire::GeminiTool {
            google_search: Some(json!({})),
            function_declarations: None,
        });
    }
    if !req.tools.is_empty() {
        tools.push(wire::GeminiTool {
            google_search: None,
            function_declarations: Some(
                req.tools
                    .iter()
                    .map(|t| wire::GeminiFunctionDeclaration {
                        name: t.name.clone(),
                        description: t.description.clone(),
                        // JSON Schema をそのまま送ると未知キーで 400 になる。
                        parameters: parameters_or_none(&t.parameters),
                    })
                    .collect(),
            ),
        });
    }

    // 関数を 1 つも提示していないなら mode を送らない。宣言が無い状態で ANY を送ると
    // 「呼べるものが無いのに呼べ」という矛盾した指示になる。
    let function_calling_config = (!req.tools.is_empty()).then(|| match &req.tool_choice {
        ToolChoice::None => config("NONE", None),
        ToolChoice::Auto => config("AUTO", None),
        ToolChoice::Required => config("ANY", None),
        ToolChoice::Specific(name) => config("ANY", Some(vec![name.clone()])),
    });

    // 組み込みツールを使うなら、その実行記録を応答に含めるよう明示しないと
    // 関数呼び出しとの併用そのものが拒否される。
    let include_server_side_tool_invocations = google_search.then_some(true);

    let tool_config = (function_calling_config.is_some()
        || include_server_side_tool_invocations.is_some())
    .then_some(wire::GeminiToolConfig {
        function_calling_config,
        include_server_side_tool_invocations,
    });

    wire::GeminiRequest {
        contents,
        system_instruction,
        tools,
        tool_config,
        generation_config: wire::GeminiGenerationConfig {
            temperature: req.temperature,
            max_output_tokens: req.max_tokens,
            // **常に送る**（Spec 33 D3）。送らないと思考の part が返らないが、
            // 思考自体は起きていて課金もされている — 返し方だけを変える欄。
            // 接地（`google_search`）とは独立で、あちらの「使うときだけ送る」
            // 規律とは条件が違う（思考は接地の有無と無関係に常に起きている）。
            thinking_config: wire::GeminiThinkingConfig {
                include_thoughts: true,
            },
        },
    }
}

/// JSON Schema を Gemini の `Schema` へ削り落とす（純関数）。
///
/// やることは 3 つだけ。
///
/// 1. [`SCHEMA_KEYS`] に無いキーを落とす（`$schema` / `additionalProperties` など）
/// 2. `const: X` を `enum: [X]` へ写す。Gemini に `const` は無いが `enum` はあり、
///    「この値だけ」という意味は保てる。落とすと制約ごと消える
/// 3. `type: ["string","null"]` を `type: "string"` + `nullable: true` へ写す。
///    Gemini の `type` は単一文字列で、配列を送ると弾かれる
///
/// **`$ref` / `$defs` は解決しない。** 参照を使う MCP スキーマは、参照先を失った
/// 空の項目になって Gemini に拒否される。そこを埋めるには型を推測することになり、
/// 嘘のスキーマでモデルを動かすほうが害が大きい。踏んだら Spec 05 で扱う。
fn sanitize_schema(schema: &Value) -> Value {
    let Some(object) = schema.as_object() else {
        // 配列や真偽値の schema（`additionalProperties: false` の値など）は
        // ここへ来ない。来ても写しようがないので素通しする。
        return schema.clone();
    };

    let mut out = serde_json::Map::new();

    for (key, value) in object {
        if !SCHEMA_KEYS.contains(&key.as_str()) {
            continue;
        }
        let mapped = match key.as_str() {
            "properties" => match value.as_object() {
                Some(props) => Value::Object(
                    props
                        .iter()
                        .map(|(name, sub)| (name.clone(), sanitize_schema(sub)))
                        .collect(),
                ),
                None => continue,
            },
            "items" => sanitize_schema(value),
            "anyOf" => match value.as_array() {
                Some(variants) => Value::Array(variants.iter().map(sanitize_schema).collect()),
                None => continue,
            },
            // 単一の型名はそのまま。配列なら null を抜いて nullable へ移す。
            "type" => match value.as_array() {
                Some(types) => {
                    if types.iter().any(|t| t == "null") {
                        out.insert("nullable".into(), Value::Bool(true));
                    }
                    match types.iter().find(|t| *t != "null") {
                        Some(first) => first.clone(),
                        None => continue,
                    }
                }
                None => value.clone(),
            },
            _ => value.clone(),
        };
        out.insert(key.clone(), mapped);
    }

    // `const` は許可リストに無いので上のループでは拾われない。ここで enum へ写す。
    if let Some(value) = object.get("const") {
        out.entry("enum".to_owned())
            .or_insert_with(|| Value::Array(vec![value.clone()]));
    }

    Value::Object(out)
}

/// 引数を取らない関数では `parameters` ごと省く。
///
/// 削った結果 `properties` が空になることがある（中身が全部未対応キーだった場合）。
/// 空の `properties` を送ると「引数があるはずなのに何も定義されていない」形になるので、
/// キーごと落として「引数なし」であることを素直に伝える。
fn parameters_or_none(schema: &Value) -> Option<Value> {
    let sanitized = sanitize_schema(schema);
    let empty = sanitized
        .get("properties")
        .and_then(Value::as_object)
        .is_none_or(serde_json::Map::is_empty);
    (!empty).then_some(sanitized)
}

/// `functionCallingConfig` を組む小さなヘルパ。
fn config(mode: &'static str, allowed: Option<Vec<String>>) -> wire::GeminiFunctionCallingConfig {
    wire::GeminiFunctionCallingConfig {
        mode,
        allowed_function_names: allowed,
    }
}

/// canonical の 1 発話を Gemini の形へ写す。
///
/// ツール結果が `user` ロールになるのが最大の差。`tool` という役割が存在せず、
/// 「モデルへ渡す入力」は全部 `user` に畳まれる。
///
/// **添付は `inline_data` として組み立てる**（Spec 36 D8。`attachment_contract`
/// 凍結 7 を覆した）。旧凍結は「画像は互換で運べるからネイティブに要らない」
/// だったが、**音声と動画を受けるのはこのワイヤだけ**なので前提が種別の側から
/// 消えた。覆したことは契約の当該節に取り消し線つきで書いてある。
fn encode_message(message: &ChatMessage) -> wire::GeminiContent {
    match message.role {
        Role::Tool => wire::GeminiContent {
            role: Some("user".into()),
            parts: vec![wire::GeminiPart {
                function_response: Some(wire::GeminiFunctionResponse {
                    id: message.tool_call_id.clone(),
                    name: message.tool_name.clone().unwrap_or_default(),
                    // 文字列を直に置けないのでオブジェクトで包む。
                    response: json!({ "result": message.content }),
                }),
                ..Default::default()
            }],
        },
        Role::Assistant => {
            let mut parts = Vec::new();
            if !message.content.is_empty() {
                parts.push(wire::GeminiPart {
                    text: Some(message.content.clone()),
                    ..Default::default()
                });
            }
            for call in &message.tool_calls {
                parts.push(wire::GeminiPart {
                    function_call: Some(wire::GeminiFunctionCall {
                        id: (!call.id.is_empty()).then(|| call.id.clone()),
                        name: call.name.clone(),
                        args: call.args.clone(),
                    }),
                    // 受け取った署名をそのまま返す。欠くと次の周が落ちうる。
                    thought_signature: thought_signature(call),
                    ..Default::default()
                });
            }
            wire::GeminiContent {
                role: Some("model".into()),
                parts,
            }
        }
        // System はこの関数へ来ない（encode で除外済み）。来ても user として扱えば壊れない。
        _ => {
            // 添付はテキストより前（他社と同じ並び）。**4 種別すべてを運ぶ唯一の
            // ワイヤ**で、音声と動画をここでしか送れないことが Spec 23 D8 を
            // 覆した理由（Spec 36 D8）。
            let mut parts: Vec<wire::GeminiPart> = message
                .attachments
                .iter()
                .map(|a| wire::GeminiPart {
                    inline_data: Some(wire::GeminiInlineData {
                        mime_type: a.media_type.as_str().to_owned(),
                        data: a.data.clone(),
                    }),
                    ..Default::default()
                })
                .collect();
            // **添付が無いときは今日までと 1 バイトも変わらない** — 空の本文でも
            // text パートを 1 つ出す形を保つ（golden で凍結）。
            if parts.is_empty() || !message.content.is_empty() {
                parts.push(wire::GeminiPart {
                    text: Some(message.content.clone()),
                    ..Default::default()
                });
            }
            wire::GeminiContent {
                role: Some("user".into()),
                parts,
            }
        }
    }
}

/// [`ToolCall::extra`] から思考署名を取り出す。
fn thought_signature(call: &ToolCall) -> Option<String> {
    call.extra
        .as_ref()?
        .get(THOUGHT_SIGNATURE)?
        .as_str()
        .map(str::to_owned)
}

/// Gemini wire → canonical。
///
/// ## 終了判定
///
/// `finishReason` を素直に信じてはいけない。関数呼び出しを返したときも `STOP` が来る
/// （実測: `finishMessage: "Model generated function call(s)."`）。**parts に
/// `functionCall` があれば [`Finish::ToolUse`]** とする。
///
/// ## 落としているもの
///
/// Google 側が実行済みの組み込みツール（`toolCall` / `toolResponse` パート）は
/// canonical に写す先が無いため**落としている**。検索語（`toolCall.args.queries`）は
/// 利用者に見せる価値があり、履歴として返す必要があるかも未確認。
/// どちらも Spec 05 の宿題で、確定するまで嘘の席を作らない。
pub fn decode(resp: wire::GeminiResponse) -> Result<ChatResponse, LlmError> {
    let usage = resp
        .usage_metadata
        .as_ref()
        .map(|u| Usage {
            prompt: u.prompt_token_count,
            // 思考も課金される出力。実測で
            // totalTokenCount == prompt + candidates + thoughts が成り立つ。
            completion: u.candidates_token_count + u.thoughts_token_count,
            cache_read: u.cached_content_token_count,
            // **書き込みの欄は Gemini に無い**（Spec 40）。明示キャッシュは事前に
            // cache を作る API 呼び出しが要り、**この村は呼んでいない**（暗黙のみ）。
            // 呼ぶようになったら作成コストが別に立つので、そのときここへ戻る。
            cache_write: 0,
            cache_write_1h: 0,
            // 内数として**同じ値をもう 1 度**入れる（Spec 32）。足し込みは上の
            // 行のまま触らない — `completion` の数え方を変えると
            // totalTokenCount との一致（実測で凍結済み）が壊れる。
            reasoning: u.thoughts_token_count,
        })
        .unwrap_or_default();

    let Some(candidate) = resp.candidates.into_iter().next() else {
        return Ok(ChatResponse {
            text: None,
            tool_calls: Vec::new(),
            finish: Finish::Other,
            usage,
            grounding: Grounding::default(),
            reasoning_summary: Vec::new(),
        });
    };

    // 接地の来歴。参照元は groundingMetadata にしか無く、検索語はそこと
    // toolCall パートの両方に現れうるので、両方から集めて重複を潰す。
    let mut grounding = Grounding::default();
    if let Some(meta) = candidate.grounding_metadata {
        grounding.queries = meta.web_search_queries;
        grounding.sources = meta
            .grounding_chunks
            .into_iter()
            .filter_map(|chunk| chunk.web)
            .filter_map(|web| {
                web.uri.map(|uri| GroundingSource {
                    uri,
                    title: web.title.unwrap_or_default(),
                })
            })
            .collect();
    }

    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut reasoning_summary: Vec<String> = Vec::new();

    for part in candidate.content.map(|c| c.parts).unwrap_or_default() {
        // Google が代行した組み込みツール。実行するものは無いが、
        // 起きた事実は残す（検索語は args.queries に構造化されている）。
        if let Some(server) = part.tool_call {
            for query in server
                .args
                .get("queries")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default()
            {
                if let Some(text) = query.as_str()
                    && !grounding.queries.iter().any(|q| q == text)
                {
                    grounding.queries.push(text.to_owned());
                }
            }
            continue;
        }
        // 思考の part は**答えの本文ではない**。`text` へ混ぜると内部の独白が
        // 答えとして表示される。**要約としては受け取る**（Spec 33）。
        //
        // **0 字は入れない**（`reasoning_summary` 契約の凍結 2）。
        // なお `thoughtSignature` はこの part ではなく `functionCall` の part に
        // 付いており、その往復は既に `ToolCall::extra` で成立している —
        // **思考の本文まで戻す要求は無い**（落とす / 戻すの対で実測）。
        if part.thought == Some(true) {
            if let Some(chunk) = part.text
                && !chunk.is_empty()
            {
                reasoning_summary.push(chunk);
            }
            continue;
        }
        if let Some(chunk) = part.text {
            text.push_str(&chunk);
        }
        if let Some(call) = part.function_call {
            tool_calls.push(ToolCall {
                id: call.id.unwrap_or_default(),
                name: call.name,
                args: call.args,
                extra: part
                    .thought_signature
                    .map(|sig| json!({ THOUGHT_SIGNATURE: sig })),
            });
        }
    }

    let finish = if !tool_calls.is_empty() {
        Finish::ToolUse
    } else {
        match candidate.finish_reason.as_deref() {
            Some("STOP") => Finish::Stop,
            Some("MAX_TOKENS") => Finish::Length,
            _ => Finish::Other,
        }
    };

    Ok(ChatResponse {
        text: (!text.is_empty()).then_some(text),
        tool_calls,
        finish,
        usage,
        grounding,
        reasoning_summary,
    })
}

/// エンドポイントのパス。モデル名が URL に埋まるのが他プロバイダとの構造的な差。
pub fn path(model: &str) -> String {
    format!("/models/{model}:generateContent")
}

/// 認証ヘッダの名前。Bearer でも `x-api-key` でもない第三の形。
pub const AUTH_HEADER: &str = "x-goog-api-key";

/// `プロトコル` を Gemini に切り替えたときの base URL 既定値。
pub const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::canonical::{ChatMessage, ToolSpec};

    fn tool() -> ToolSpec {
        ToolSpec {
            name: "transfer_to_zari".into(),
            description: "ザリへ転送する".into(),
            parameters: json!({ "type": "object", "properties": {} }),
        }
    }

    fn request(messages: Vec<ChatMessage>) -> ChatRequest {
        ChatRequest {
            model: "gemini-3.5-flash".into(),
            messages,
            tools: vec![tool()],
            tool_choice: ToolChoice::Auto,
            temperature: None,
            max_tokens: 4096,
            effort: None,
            cacheable_prefix_len: 0,
        }
    }

    #[test]
    fn system_goes_to_system_instruction_not_contents() {
        let req = request(vec![
            ChatMessage::system("あなたはザリ"),
            ChatMessage::user("こんにちは"),
        ]);
        let wire = encode(&req, false);

        assert_eq!(wire.contents.len(), 1, "system は contents に混ざらない");
        assert_eq!(wire.contents[0].role.as_deref(), Some("user"));
        let sys = wire.system_instruction.expect("systemInstruction が要る");
        assert_eq!(sys.parts[0].text.as_deref(), Some("あなたはザリ"));
    }

    /// **添付が無い発話は Spec 36 の前後でバイト等価**。
    ///
    /// 旧テスト `attachments_are_ignored_on_the_native_path`（Spec 23 D8 =
    /// ネイティブに添付を実装しない、の凍結）を**この形へ置き換えた**。
    /// D8 は Spec 36 で覆したので「添付を無視する」は守るべき性質ではなくなり、
    /// 残る不変条件は**添付を使わない村が 1 バイトも変わらないこと**だけ。
    #[test]
    fn encoding_without_attachments_matches_the_golden_bytes() {
        let wire = encode(&request(vec![ChatMessage::user("こんにちは")]), false);
        let json = serde_json::to_value(&wire).unwrap();
        assert_eq!(
            json["contents"],
            serde_json::json!([{ "role": "user", "parts": [{ "text": "こんにちは" }] }]),
            "添付なしの発話に inlineData の欄は生えない"
        );
    }

    /// **4 種別すべてが `inlineData` として乗る**（Spec 36 D8。音声と動画を
    /// 受けるのはこのワイヤだけで、それが D8 を覆した理由）。
    ///
    /// 添付はテキストより前・`mimeType` は各種別の MIME・キーは camelCase。
    #[test]
    fn every_kind_rides_as_inline_data_before_the_text() {
        use crate::llm::canonical::{PromptAttachment, PromptMediaType};
        for (media, mime) in [
            (PromptMediaType::Webp, "image/webp"),
            (PromptMediaType::Wav, "audio/wav"),
            (PromptMediaType::Mp3, "audio/mpeg"),
            (PromptMediaType::Mp4, "video/mp4"),
            (PromptMediaType::Pdf, "application/pdf"),
        ] {
            let wire = encode(
                &request(vec![ChatMessage::user_with_attachments(
                    "これは何？",
                    vec![PromptAttachment::new(media, "QUJD")],
                )]),
                false,
            );
            let json = serde_json::to_value(&wire).unwrap();
            assert_eq!(
                json["contents"][0]["parts"],
                serde_json::json!([
                    { "inlineData": { "mimeType": mime, "data": "QUJD" } },
                    { "text": "これは何？" },
                ]),
                "{media:?} が inlineData として先頭に乗る"
            );
        }
    }

    #[test]
    fn google_search_and_functions_are_separate_tool_entries() {
        let wire = encode(&request(vec![ChatMessage::user("調べて")]), true);

        assert_eq!(wire.tools.len(), 2, "組み込みと関数宣言は別要素");
        assert!(wire.tools[0].google_search.is_some());
        assert!(wire.tools[1].function_declarations.is_some());

        // 実測で通った綴りをそのまま送っているか。
        let json = serde_json::to_value(&wire).unwrap();
        assert!(json["tools"][0].get("google_search").is_some());
        assert!(json["tools"][1].get("functionDeclarations").is_some());
    }

    #[test]
    fn google_search_alone_still_sends_tools() {
        let mut req = request(vec![ChatMessage::user("調べて")]);
        req.tools.clear();
        let wire = encode(&req, true);

        assert_eq!(wire.tools.len(), 1);
        let cfg = wire.tool_config.expect("接地を使うなら toolConfig は要る");
        assert!(
            cfg.function_calling_config.is_none(),
            "宣言が無いなら mode を送らない"
        );
        assert_eq!(cfg.include_server_side_tool_invocations, Some(true));
    }

    #[test]
    fn combining_google_search_with_functions_enables_server_side_invocations() {
        // これが無いと 400:
        // "Please enable tool_config.include_server_side_tool_invocations
        //  to use Built-in tools with Function calling."
        let json = serde_json::to_value(encode(&request(vec![ChatMessage::user("調べて")]), true))
            .unwrap();

        assert_eq!(
            json["toolConfig"]["includeServerSideToolInvocations"],
            serde_json::Value::Bool(true)
        );
        assert_eq!(json["toolConfig"]["functionCallingConfig"]["mode"], "AUTO");
    }

    #[test]
    fn no_google_search_means_no_server_side_flag() {
        // 接地を使わないなら送らない。無関係なキーを足すと、
        // 組み込みツール非対応のモデルで弾かれうる。
        let json = serde_json::to_value(encode(&request(vec![ChatMessage::user("hi")]), false))
            .unwrap();

        assert!(
            json["toolConfig"]
                .get("includeServerSideToolInvocations")
                .is_none()
        );
        assert_eq!(json["toolConfig"]["functionCallingConfig"]["mode"], "AUTO");
    }

    #[test]
    fn temperature_is_omitted_when_unset() {
        let json = serde_json::to_value(encode(&request(vec![ChatMessage::user("hi")]), false)).unwrap();
        assert!(json["generationConfig"].get("temperature").is_none());
    }

    #[test]
    fn tool_result_is_encoded_as_user_role_function_response() {
        let req = request(vec![ChatMessage::tool_result("c4b3", "grep", "3 件")]);
        let wire = encode(&req, false);

        assert_eq!(wire.contents[0].role.as_deref(), Some("user"), "tool ロールは無い");
        let fr = wire.contents[0].parts[0]
            .function_response
            .as_ref()
            .expect("functionResponse");
        assert_eq!(fr.name, "grep");
        assert_eq!(fr.id.as_deref(), Some("c4b3"));
        assert_eq!(fr.response["result"], json!("3 件"));
    }

    #[test]
    fn function_call_returns_tool_use_despite_stop_finish_reason() {
        // 実測の形: functionCall を返しても finishReason は STOP。
        let raw = r#"{
          "candidates": [{
            "content": {"role":"model","parts":[
              {"functionCall":{"id":"j1f","name":"transfer_to_zari","args":{"message":"晴れ"}},
               "thoughtSignature":"SIG"}
            ]},
            "finishReason": "STOP"
          }],
          "usageMetadata": {"promptTokenCount":170,"candidatesTokenCount":97,"thoughtsTokenCount":407}
        }"#;
        let resp: wire::GeminiResponse = serde_json::from_str(raw).unwrap();
        let decoded = decode(resp).unwrap();

        assert_eq!(decoded.finish, Finish::ToolUse, "STOP を終了と読まない");
        assert_eq!(decoded.tool_calls.len(), 1);
        assert_eq!(decoded.tool_calls[0].name, "transfer_to_zari");
        // 思考も課金対象の出力として数える（実測の totalTokenCount と一致する数え方）。
        assert_eq!(decoded.usage.completion, 504);
        assert_eq!(decoded.usage.total(), 674);
        // Spec 32: 思考ぶんを**内数として**別に持つ。足し込み（504）は変えない。
        assert_eq!(decoded.usage.reasoning, 407);
        assert_eq!(
            decoded.usage.completion - decoded.usage.reasoning,
            97,
            "差は本文（candidatesTokenCount）でなければならない"
        );
    }

    /// `includeThoughts` は**常に送る**（Spec 33 D3）。
    ///
    /// 送らないと `thought: true` の part がそもそも返らない（実測 — 既定の応答は
    /// 答えの part 1 つだけ）。**モデルによるガードは要らない** —
    /// `gemini-3.5-flash-lite` / `gemini-3.6-flash` の双方が 200 で受けた。
    #[test]
    fn thinking_config_is_always_requested() {
        let req = ChatRequest::plain("gemini-3.6-flash", vec![ChatMessage::user("問い")], 512);
        let json = serde_json::to_value(encode(&req, false)).unwrap();
        assert_eq!(
            json["generationConfig"]["thinkingConfig"],
            serde_json::json!({ "includeThoughts": true })
        );
        // 接地を使わない要求でも送る（`includeServerSideToolInvocations` と違い、
        // 思考は接地の有無と無関係に起きているため）。
        assert!(json.get("toolConfig").is_none(), "接地なしでは toolConfig は出ない");
    }

    /// 思考の part は**要約として**受け取り、**答えの本文へは混ぜない**（Spec 33）。
    #[test]
    fn thought_parts_become_the_summary_not_the_answer() {
        let raw = r#"{
          "candidates":[{
            "content":{"role":"model","parts":[
              {"text":"**Logical Deduction**\nOkay, here's my read.","thought":true},
              {"text":"犯人は A です"}
            ]},
            "finishReason":"STOP"
          }],
          "usageMetadata":{"promptTokenCount":64,"candidatesTokenCount":10,"thoughtsTokenCount":2994}
        }"#;
        let out = decode(serde_json::from_str::<wire::GeminiResponse>(raw).unwrap()).unwrap();

        assert_eq!(out.text.as_deref(), Some("犯人は A です"), "本文は混ざらない");
        assert_eq!(
            out.reasoning_summary,
            vec!["**Logical Deduction**\nOkay, here's my read.".to_owned()]
        );
        assert_eq!(out.usage.reasoning, 2_994);
    }

    /// 空の思考 part は列へ入れない（`reasoning_summary` 契約の凍結 2）。
    /// **答えだけの応答**（`includeThoughts` が効かない回）でも壊れない。
    #[test]
    fn an_empty_or_absent_thought_part_yields_no_summary() {
        let raw = r#"{
          "candidates":[{
            "content":{"role":"model","parts":[
              {"text":"","thought":true},
              {"text":"2 です"}
            ]},
            "finishReason":"STOP"
          }],
          "usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":3}
        }"#;
        let out = decode(serde_json::from_str::<wire::GeminiResponse>(raw).unwrap()).unwrap();

        assert!(out.reasoning_summary.is_empty());
        assert_eq!(out.text.as_deref(), Some("2 です"));
    }

    #[test]
    fn server_side_search_parts_do_not_become_tool_calls() {
        // Google が実行済みの toolCall / toolResponse を functionCall と取り違えると、
        // 実行済みの検索をこちらが二重に実行しにいく。
        let raw = r#"{
          "candidates": [{
            "content": {"role":"model","parts":[
              {"toolCall":{"toolType":"GOOGLE_SEARCH_WEB","args":{"queries":["東京 天気"]},"id":"c4b3"},
               "thoughtSignature":"A"},
              {"toolResponse":{"toolType":"GOOGLE_SEARCH_WEB","response":{"search_suggestions":"<style>"},"id":"c4b3"},
               "thoughtSignature":"B"},
              {"text":"東京は晴れです"}
            ]},
            "finishReason": "STOP"
          }]
        }"#;
        let resp: wire::GeminiResponse = serde_json::from_str(raw).unwrap();
        let decoded = decode(resp).unwrap();

        assert!(decoded.tool_calls.is_empty(), "組み込みツールは呼び出しではない");
        assert_eq!(decoded.text.as_deref(), Some("東京は晴れです"));
        assert_eq!(decoded.finish, Finish::Stop);
    }

    #[test]
    fn grounding_sources_are_captured_not_dropped() {
        // ここに URL が来るかどうかが、モデルに出典を訊けるかどうかを決める。
        let raw = r#"{"candidates":[{"content":{"parts":[{"text":"熊本で爆発"}]},
          "finishReason":"STOP",
          "groundingMetadata":{
            "webSearchQueries":["熊本 爆発"],
            "groundingChunks":[
              {"web":{"uri":"https://news.example.jp/a","title":"爆発の続報"}},
              {"web":{"title":"URL の無い塊"}},
              {"retrievedContext":{"text":"web 以外の接地"}}
            ]}}]}"#;
        let resp: wire::GeminiResponse = serde_json::from_str(raw).unwrap();
        let g = decode(resp).unwrap().grounding;

        assert_eq!(g.queries, vec!["熊本 爆発"]);
        assert_eq!(g.sources.len(), 1, "URL を持つものだけを出典として数える");
        assert_eq!(g.sources[0].uri, "https://news.example.jp/a");
        assert_eq!(g.sources[0].title, "爆発の続報");
    }

    #[test]
    fn server_side_search_queries_survive_without_grounding_metadata() {
        // groundingMetadata が付かない形（最終出力が functionCall のとき）でも、
        // 「何を検索したか」だけは toolCall から拾える。
        let raw = r#"{"candidates":[{"content":{"parts":[
          {"toolCall":{"toolType":"GOOGLE_SEARCH_WEB","args":{"queries":["東京 天気","東京 気温"]},"id":"c1"}},
          {"functionCall":{"name":"transfer_to_zari","args":{"message":"晴れ"}}}
        ]},"finishReason":"STOP"}]}"#;
        let resp: wire::GeminiResponse = serde_json::from_str(raw).unwrap();
        let decoded = decode(resp).unwrap();

        assert_eq!(decoded.grounding.queries, vec!["東京 天気", "東京 気温"]);
        assert!(decoded.grounding.sources.is_empty(), "出典は無い = 無いと言える");
        assert_eq!(decoded.tool_calls.len(), 1, "検索は呼び出しに数えない");
    }

    #[test]
    fn duplicate_queries_from_both_sources_are_folded() {
        let raw = r#"{"candidates":[{"content":{"parts":[
          {"toolCall":{"toolType":"GOOGLE_SEARCH_WEB","args":{"queries":["熊本 爆発","嘉島町"]}}},
          {"text":"ok"}
        ]},"finishReason":"STOP",
        "groundingMetadata":{"webSearchQueries":["熊本 爆発"]}}]}"#;
        let resp: wire::GeminiResponse = serde_json::from_str(raw).unwrap();

        assert_eq!(decode(resp).unwrap().grounding.queries, vec!["熊本 爆発", "嘉島町"]);
    }

    #[test]
    fn no_grounding_means_empty_not_absent() {
        let raw = r#"{"candidates":[{"content":{"parts":[{"text":"ok"}]},"finishReason":"STOP"}]}"#;
        let resp: wire::GeminiResponse = serde_json::from_str(raw).unwrap();

        assert!(decode(resp).unwrap().grounding.is_empty());
    }

    #[test]
    fn thought_blocks_never_leak_into_text() {
        let raw = r#"{"candidates":[{"content":{"parts":[
          {"text":"内部の独白","thought":true},
          {"text":"見せる本文"}
        ]},"finishReason":"STOP"}]}"#;
        let resp: wire::GeminiResponse = serde_json::from_str(raw).unwrap();

        assert_eq!(decode(resp).unwrap().text.as_deref(), Some("見せる本文"));
    }

    #[test]
    fn thought_signature_round_trips_through_extra() {
        let raw = r#"{"candidates":[{"content":{"parts":[
          {"functionCall":{"name":"grep","args":{}},"thoughtSignature":"SIG"}
        ]},"finishReason":"STOP"}]}"#;
        let resp: wire::GeminiResponse = serde_json::from_str(raw).unwrap();
        let calls = decode(resp).unwrap().tool_calls;

        // 履歴として送り返したとき、同じ署名が同じ場所に戻ること。
        let req = request(vec![ChatMessage::assistant_tool_calls("", calls)]);
        let wire = encode(&req, false);
        assert_eq!(
            wire.contents[0].parts[0].thought_signature.as_deref(),
            Some("SIG")
        );
    }

    #[test]
    fn unknown_finish_reason_does_not_break_parsing() {
        let raw = r#"{"candidates":[{"content":{"parts":[{"text":"ok"}]},"finishReason":"SAFETY"}]}"#;
        let resp: wire::GeminiResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(decode(resp).unwrap().finish, Finish::Other);
    }

    #[test]
    fn bundled_tool_schema_sheds_additional_properties() {
        // 実際に 400 を出した形。同梱ツール 7 本すべてがこれを出していた。
        let mut req = request(vec![ChatMessage::user("探して")]);
        req.tools[0].parameters = json!({
            "type": "object",
            "properties": { "pattern": { "type": "string", "description": "正規表現" } },
            "required": ["pattern"],
            "additionalProperties": false
        });
        let json = serde_json::to_value(encode(&req, false)).unwrap();
        let params = &json["tools"][0]["functionDeclarations"][0]["parameters"];

        assert!(params.get("additionalProperties").is_none(), "未対応キーは落とす");
        assert_eq!(params["properties"]["pattern"]["type"], "string", "中身は保つ");
        assert_eq!(params["required"], json!(["pattern"]));
    }

    #[test]
    fn mcp_tool_schema_sheds_unknown_keys_recursively() {
        // MCP サーバーが書くスキーマ。$schema 付きで、入れ子にも未対応キーが混ざる。
        let mut req = request(vec![ChatMessage::user("hi")]);
        req.tools[0].parameters = json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "path": { "type": "string", "$comment": "無視されるべき" },
                "depth": { "type": "integer", "exclusiveMinimum": 0 },
                "items": { "type": "array", "items": { "type": "string", "$id": "x" } }
            }
        });
        let json = serde_json::to_value(encode(&req, false)).unwrap();
        let params = &json["tools"][0]["functionDeclarations"][0]["parameters"];

        assert!(params.get("$schema").is_none());
        assert!(params["properties"]["path"].get("$comment").is_none(), "入れ子も削る");
        assert!(params["properties"]["depth"].get("exclusiveMinimum").is_none());
        assert!(
            params["properties"]["items"]["items"].get("$id").is_none(),
            "items の下まで再帰する"
        );
        assert_eq!(params["properties"]["items"]["items"]["type"], "string");
    }

    #[test]
    fn const_becomes_enum_instead_of_vanishing() {
        // Gemini に const は無いが enum はある。落とすと制約ごと消える。
        let mut req = request(vec![ChatMessage::user("hi")]);
        req.tools[0].parameters = json!({
            "type": "object",
            "properties": { "mode": { "type": "string", "const": "preview" } }
        });
        let json = serde_json::to_value(encode(&req, false)).unwrap();
        let mode = &json["tools"][0]["functionDeclarations"][0]["parameters"]["properties"]["mode"];

        assert!(mode.get("const").is_none());
        assert_eq!(mode["enum"], json!(["preview"]));
    }

    #[test]
    fn nullable_type_array_becomes_single_type_plus_nullable() {
        let mut req = request(vec![ChatMessage::user("hi")]);
        req.tools[0].parameters = json!({
            "type": "object",
            "properties": { "note": { "type": ["string", "null"] } }
        });
        let json = serde_json::to_value(encode(&req, false)).unwrap();
        let note = &json["tools"][0]["functionDeclarations"][0]["parameters"]["properties"]["note"];

        assert_eq!(note["type"], "string", "配列の type は弾かれる");
        assert_eq!(note["nullable"], json!(true));
    }

    #[test]
    fn parameterless_tool_omits_the_key_entirely() {
        let mut req = request(vec![ChatMessage::user("hi")]);
        req.tools[0].parameters = json!({ "type": "object", "properties": {} });
        let json = serde_json::to_value(encode(&req, false)).unwrap();

        assert!(
            json["tools"][0]["functionDeclarations"][0]
                .get("parameters")
                .is_none(),
            "空の properties は送らない"
        );
    }

    #[test]
    fn path_embeds_model_name() {
        assert_eq!(path("gemini-3.5-flash"), "/models/gemini-3.5-flash:generateContent");
    }
}
