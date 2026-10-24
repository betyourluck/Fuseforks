//! Responses ワイヤ 4 本（xAI / OpenAI / Meta / Perplexity）が共有する `input` 列の組み立て。
//! （Spec 34 で 2 本から始まり、Spec 37 と Spec 45 で `part_for` を渡す形のまま 4 本になった。）
//!
//! **共有するのは要素の型が同じだと実測しているから**（Spec 34 D2 rev6）—
//! `function_call` / `function_call_output` / メッセージのどれも、
//! 両社で同じ形が通ることを P0a の probe で確かめている。
//!
//! **トップレベルの要求構造体は共有しない**（`include` / `reasoning` /
//! `temperature` で割れる）。分けたのは「自分が決める側」で、
//! 揃っているのは「相手が決める側」— **共有の可否をその 2 つで別々に判断した**
//! のがこの分割の理由。
//!
//! 1 実装にしてあるのは、2 箇所に写すと**片方だけ直す**形が生まれるため
//! （`failures.md` #88 / #96 と同じ「関数は正しい / 射程だけが違う」）。

use super::canonical::{ChatMessage, PromptAttachment, Role};
use crate::attachment::AttachmentKind;
use super::wire;

/// 添付 1 件を、そのワイヤの part へ写す関数（**adapter が渡す**）。
///
/// **共有するのは「相手が決める側」だけ**（Spec 37 D2）。Responses を名乗る
/// 3 社は**関数往復の形が完全に同じ**だと実測しているが（`call_id` の命名も
/// `arguments` が JSON 文字列であることも一致）、**添付の part は割れる** —
/// Meta だけが `input_audio` / `input_video` を持つ。
///
/// `function_call_output.output` を「string 限定」と書いていたのは誤りだった —
/// **`input_text` と `input_image` の列も 4 社とも受ける**（Spec 70 P0）。
/// 列を送ったことが無かったので、限定かどうかを測っていなかった。
///
/// **`Provider::carries` をここから読まない。** 読むと
/// `adapters_match_the_carries_table` が同語反復になり、**表と実装が食い違う
/// ことを検出する網が 1 枚死ぬ**。各 adapter は「自分が何を組み立てられるか」を
/// 独立に書き、テストがそれと表を突き合わせる。
pub(super) type AttachmentPart = fn(&PromptAttachment) -> Option<wire::ResponsesInputPart>;

/// 画像と PDF だけを組み立てる（xAI / OpenAI Responses の共通の腕）。
///
/// **2 社で同じなのは偶然ではなく実測** — どちらも 400 の受理集合列挙で
/// `input_text` / `input_image` / `input_file` を返し、音声・動画の part は
/// 列挙に無かった。**Meta は別の関数を持つ**（`meta_responses::attachment_part`）。
pub(super) fn image_and_pdf_part(
    attachment: &PromptAttachment,
) -> Option<wire::ResponsesInputPart> {
    match attachment.kind() {
        AttachmentKind::Image => Some(wire::ResponsesInputPart::InputImage {
            image_url: attachment.data_url(),
        }),
        AttachmentKind::Pdf => Some(wire::ResponsesInputPart::InputFile {
            filename: attachment.file_name_or_default(),
            file_data: attachment.data_url(),
        }),
        AttachmentKind::Audio | AttachmentKind::Video => None,
    }
}

/// canonical の発話列を Responses の `input` 列へ写す（4 社が共有）。
///
/// **骨格（テキストと関数往復）だけを共有し、添付の part は `part_for` が決める**
/// （Spec 37 D2）。Spec 34 D2 の分割規則
/// 「揃っているのは相手が決める側 / 分けたのは自分が決める側」を content 層へ
/// そのまま当てたもの。
pub(super) fn encode(
    messages: &[ChatMessage],
    part_for: AttachmentPart,
) -> Vec<wire::ResponsesInputItem> {
    let mut input = Vec::new();
    for message in messages {
        match message.role {
            Role::System => input.push(wire::ResponsesInputItem::Message {
                role: "system",
                content: wire::ResponsesContent::Text(message.content.clone()),
            }),
            Role::User => input.push(wire::ResponsesInputItem::Message {
                role: "user",
                content: encode_user_content(message, part_for),
            }),
            Role::Assistant => {
                // 空本文の assistant は出さない（#29 — 空発話を積むと次のターンが 400）。
                if !message.content.is_empty() {
                    input.push(wire::ResponsesInputItem::Message {
                        role: "assistant",
                        content: wire::ResponsesContent::Text(message.content.clone()),
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
                output: encode_tool_output(message, part_for),
            }),
        }
    }
    input
}

/// ツール結果の本文（Spec 70 D1 = ネイティブ）。
///
/// **画像が無ければ素の文字列**（Spec 70 の前と同じバイト列）。あれば `input_text` の後ろに
/// 画像を並べる（P0 で 4 社とも通った並び）。運ぶのは画像だけで、part の形は利用者の発話と
/// 同じ `part_for` に任せる — 画像の part を 2 箇所に書くと、片方だけ直す形が生まれる。
fn encode_tool_output(message: &ChatMessage, part_for: AttachmentPart) -> wire::ResponsesContent {
    let images: Vec<wire::ResponsesInputPart> = message
        .attachments
        .iter()
        .filter(|a| a.kind() == AttachmentKind::Image)
        .filter_map(part_for)
        .collect();
    if images.is_empty() {
        return wire::ResponsesContent::Text(message.content.clone());
    }
    let mut parts = Vec::with_capacity(images.len() + 1);
    if !message.content.is_empty() {
        parts.push(wire::ResponsesInputPart::InputText {
            text: message.content.clone(),
        });
    }
    parts.extend(images);
    wire::ResponsesContent::Parts(parts)
}

/// user 発話の本文（添付があればブロック列、無ければ素の文字列）。
///
/// **添付ゼロで形が変わらないことが不変条件** — 常にブロック列を作ると、
/// 添付を一度も使わない村の要求まで形が変わる（`OaiContent` と同じ規律）。
fn encode_user_content(
    message: &ChatMessage,
    part_for: AttachmentPart,
) -> wire::ResponsesContent {
    // 何を組み立てられるかは `part_for`（= adapter）が決める。正面の門は
    // 送信入口の `carries`（Spec 36 D2）で、ここは最後の砦。
    let mut parts: Vec<wire::ResponsesInputPart> =
        message.attachments.iter().filter_map(part_for).collect();
    if parts.is_empty() {
        return wire::ResponsesContent::Text(message.content.clone());
    }
    parts.push(wire::ResponsesInputPart::InputText {
        text: message.content.clone(),
    });
    wire::ResponsesContent::Parts(parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::canonical::{PromptAttachment, PromptMediaType, ToolCall};
    use serde_json::json;

    /// ツールを呼んで結果が返った 3 発話（Spec 70）。
    fn tool_messages(with_image: bool) -> Vec<ChatMessage> {
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
        vec![
            ChatMessage::user("撮って"),
            ChatMessage::assistant_tool_calls("", vec![call]),
            result,
        ]
    }

    /// 画像は `function_call_output.output` の列へ、本文の後ろ（Spec 70 D1 = ネイティブ。
    /// P0 で 4 社とも通った並び）。
    #[test]
    fn a_tool_result_image_goes_into_the_function_call_output() {
        let input = serde_json::to_value(encode(&tool_messages(true), image_and_pdf_part)).unwrap();
        let items = input.as_array().unwrap();
        assert_eq!(items.len(), 3, "画像のために発話を足さない");
        assert_eq!(items[2]["type"], "function_call_output");
        assert_eq!(
            items[2]["output"],
            json!([
                {"type":"input_text","text":r#"{"width":8}"#},
                {"type":"input_image","image_url":"data:image/jpeg;base64,SU1H"},
            ])
        );
        assert!(!input.to_string().contains("camera の結果の画像"));
    }

    /// 画像の無いツール結果は素の文字列（Spec 70 の前と同じバイト列）。
    #[test]
    fn a_tool_result_without_images_keeps_a_plain_string() {
        let input = serde_json::to_value(encode(&tool_messages(false), image_and_pdf_part)).unwrap();
        assert_eq!(input[2]["output"], r#"{"width":8}"#);
    }
}
