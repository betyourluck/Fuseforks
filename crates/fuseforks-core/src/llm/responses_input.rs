//! Responses ワイヤ 2 本（xAI / OpenAI）が共有する `input` 列の組み立て。
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

use super::canonical::{ChatMessage, Role};
use crate::attachment::AttachmentKind;
use super::wire;

/// canonical の発話列を Responses の `input` 列へ写す。
///
/// **添付は画像と PDF を組み立てる**（Spec 36 D9。`attachment_contract` 凍結 7 の
/// 据え置きを解いた）。これで「ネイティブを選ぶと画像が黙って落ちる」3 例目
/// （Spec 34 検収 7）が消える。音声・動画の part 型はこのワイヤに存在しない —
/// 400 の受理集合列挙で確認済み（`input_text` / `input_image` / `input_file` ほか）。
pub(super) fn encode(messages: &[ChatMessage]) -> Vec<wire::ResponsesInputItem> {
    let mut input = Vec::new();
    for message in messages {
        match message.role {
            Role::System => input.push(wire::ResponsesInputItem::Message {
                role: "system",
                content: wire::ResponsesContent::Text(message.content.clone()),
            }),
            Role::User => input.push(wire::ResponsesInputItem::Message {
                role: "user",
                content: encode_user_content(message),
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
                output: message.content.clone(),
            }),
        }
    }
    input
}

/// user 発話の本文（添付があればブロック列、無ければ素の文字列）。
///
/// **添付ゼロで形が変わらないことが不変条件** — 常にブロック列を作ると、
/// 添付を一度も使わない村の要求まで形が変わる（`OaiContent` と同じ規律）。
fn encode_user_content(message: &ChatMessage) -> wire::ResponsesContent {
    let mut parts: Vec<wire::ResponsesInputPart> = message
        .attachments
        .iter()
        .filter_map(|a| match a.kind() {
            AttachmentKind::Image => Some(wire::ResponsesInputPart::InputImage {
                image_url: a.data_url(),
            }),
            AttachmentKind::Pdf => Some(wire::ResponsesInputPart::InputFile {
                filename: a.file_name_or_default(),
                file_data: a.data_url(),
            }),
            // このワイヤに音声・動画の part 型は無い。正面の門は送信入口の
            // `carries`（Spec 36 D2）で、ここは最後の砦。
            AttachmentKind::Audio | AttachmentKind::Video => None,
        })
        .collect();
    if parts.is_empty() {
        return wire::ResponsesContent::Text(message.content.clone());
    }
    parts.push(wire::ResponsesInputPart::InputText {
        text: message.content.clone(),
    });
    wire::ResponsesContent::Parts(parts)
}
