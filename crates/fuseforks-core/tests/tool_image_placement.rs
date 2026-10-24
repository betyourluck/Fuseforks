//! **ツール結果の画像をどこへ置くかを、7 ワイヤの表で留める**（Spec 70 D1。契約は
//! `data_contract.yaml` の `mcp_image_result_contract` の凍結 2）。
//!
//! 置き場は 2 つ — ツール結果の中（ネイティブ）と、ツール結果の直後の利用者の発話（代替）。
//! どちらにするかは adapter が決める（プロンプトを組む側は場合分けを抱えない）ので、
//! **表は adapter の外ではここにしか無い**。実行時に表を読む者が居ないので、
//! `Provider::carries` のような述語は置いていない — 置くと、テストだけが読む 2 つ目の写しになる。
//!
//! 見るのは構造の 1 点だけ: **画像を載せたときに、要求の発話の列が 1 つ伸びるか**。
//! 型名は社ごとに違う（`tool_result` / `functionResponse` / `function_call_output` /
//! `role: "tool"`）ので、種別ごとの形はそれぞれの adapter の単体テストが持つ。

use fuseforks_core::llm::{
    ChatMessage, ChatRequest, PromptAttachment, PromptMediaType, Provider, ToolCall,
};
use serde_json::{Value, json};

const DATA: &str = "SU1HX1NQRUNfNzA";
const CAPTION: &str = "（camera の結果の画像 1 枚）";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placement {
    /// ツール結果の中。
    Native,
    /// ツール結果の直後の利用者の発話。
    FollowingUserMessage,
}

/// ツールを呼んで結果が返った 3 発話。`with_image` なら結果に画像と代替の本文が付く。
fn request(with_image: bool) -> ChatRequest {
    let mut result = ChatMessage::tool_result("call_1", "camera", r#"{"width":8}"#);
    if with_image {
        result.attachments = vec![PromptAttachment::new(PromptMediaType::Jpeg, DATA)];
        result.attachment_caption = Some(CAPTION.into());
    }
    let call = ToolCall {
        id: "call_1".into(),
        name: "camera".into(),
        args: json!({}),
        extra: None,
    };
    ChatRequest::plain(
        "m",
        vec![
            ChatMessage::user("撮って"),
            ChatMessage::assistant_tool_calls("", vec![call]),
            result,
        ],
        64,
    )
}

/// そのワイヤの encode 結果と、発話の列の欄名。
fn encoded(provider: Provider, req: &ChatRequest) -> (Value, &'static str) {
    use fuseforks_core::llm::{
        anthropic, gemini, meta_responses, openai_compat, openai_responses,
        perplexity_responses, xai_responses,
    };
    match provider {
        Provider::OpenAiCompat => (
            serde_json::to_value(openai_compat::encode(req, true)).unwrap(),
            "messages",
        ),
        Provider::Anthropic => (
            serde_json::to_value(anthropic::encode(req)).unwrap(),
            "messages",
        ),
        Provider::Gemini => (
            serde_json::to_value(gemini::encode(req, gemini::GeminiSkills::default())).unwrap(),
            "contents",
        ),
        Provider::XaiResponses => (
            serde_json::to_value(xai_responses::encode(req, true, false, false)).unwrap(),
            "input",
        ),
        Provider::OpenAiResponses => (
            serde_json::to_value(openai_responses::encode(req, true, false, false)).unwrap(),
            "input",
        ),
        Provider::MetaResponses => (
            serde_json::to_value(meta_responses::encode(req, true, false)).unwrap(),
            "input",
        ),
        Provider::PerplexityResponses => (
            serde_json::to_value(perplexity_responses::encode(
                req,
                true,
                perplexity_responses::Tools::default(),
            ))
            .unwrap(),
            "input",
        ),
    }
}

/// adapter の組み立てから、置き場を読み取る。
fn observed(provider: Provider) -> Placement {
    let (plain, key) = encoded(provider, &request(false));
    let (with_image, _) = encoded(provider, &request(true));
    let before = plain[key].as_array().expect("発話の列").len();
    let after = with_image[key].as_array().expect("発話の列");
    assert!(
        with_image.to_string().contains(DATA),
        "{provider:?}: 画像がワイヤへ出ていない"
    );
    if after.len() == before {
        assert!(
            !with_image.to_string().contains(CAPTION),
            "{provider:?}: ネイティブのワイヤが代替の本文まで送っている"
        );
        return Placement::Native;
    }
    assert_eq!(after.len(), before + 1, "{provider:?}: 足す発話は 1 つ");
    let carrier = after.last().unwrap().to_string();
    assert!(carrier.contains(DATA) && carrier.contains(CAPTION), "{provider:?}: 末尾の発話が運び台");
    assert!(
        !after[..before].iter().any(|m| m.to_string().contains(DATA)),
        "{provider:?}: 代替なのにツール結果の中にも画像がある"
    );
    Placement::FollowingUserMessage
}

/// **P0 の実測で決めた表を逐語で凍結する**（2026-10-24。合成の 2 色の画像で、色を読めたかで判定）。
///
/// **`Provider::ALL` を使わない** — variant を足した人が、この行でも置き場を決めさせられる
/// （`tests/carries_table.rs` と同じ規律）。新しいワイヤの置き場は測らないと決まらない。
#[test]
fn every_wire_places_tool_images_where_the_probes_said() {
    use Placement::{FollowingUserMessage, Native};
    let expected = [
        // ネイティブは接続先しだい（Gemini の互換の口は 400、xAI の互換の口は通る）なので、
        // 測った 2 つで通る代替に固定した。
        (Provider::OpenAiCompat, FollowingUserMessage),
        (Provider::Anthropic, Native),
        (Provider::Gemini, Native),
        (Provider::XaiResponses, Native),
        (Provider::OpenAiResponses, Native),
        (Provider::MetaResponses, Native),
        // 画像を受けるモデル（google/gemini-3.7-flash）で読めた。deepseek はモデルの層で 400。
        (Provider::PerplexityResponses, Native),
    ];
    for (provider, placement) in expected {
        assert_eq!(observed(provider), placement, "{provider:?} の置き場");
    }
    assert_eq!(expected.len(), 7, "表は 7 ワイヤを覆う");
}

/// 画像の無いツール結果は、7 ワイヤとも本文を文字列のまま運ぶ（Spec 70 の前と同じ形）。
#[test]
fn a_tool_result_without_images_is_plain_text_on_every_wire() {
    let req = request(false);
    let text = r#"{"width":8}"#;
    let (oai, _) = encoded(Provider::OpenAiCompat, &req);
    assert_eq!(oai["messages"][2]["content"], text);
    let (ant, _) = encoded(Provider::Anthropic, &req);
    assert_eq!(ant["messages"][2]["content"][0]["content"], text);
    let (gem, _) = encoded(Provider::Gemini, &req);
    assert!(gem["contents"][2]["parts"][0]["functionResponse"].get("parts").is_none());
    for provider in [
        Provider::XaiResponses,
        Provider::OpenAiResponses,
        Provider::MetaResponses,
        Provider::PerplexityResponses,
    ] {
        let (json, _) = encoded(provider, &req);
        assert_eq!(json["input"][2]["output"], text, "{provider:?}");
    }
}
