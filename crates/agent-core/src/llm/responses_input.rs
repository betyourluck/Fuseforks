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
use super::wire;

/// canonical の発話列を Responses の `input` 列へ写す。
///
/// **添付画像は組み立てない**（`attachment_contract` 凍結 7 の据え置き。
/// 画像は互換経路のみ）。
pub(super) fn encode(messages: &[ChatMessage]) -> Vec<wire::ResponsesInputItem> {
    let mut input = Vec::new();
    for message in messages {
        match message.role {
            Role::System => input.push(wire::ResponsesInputItem::Message {
                role: "system",
                content: message.content.clone(),
            }),
            Role::User => input.push(wire::ResponsesInputItem::Message {
                role: "user",
                content: message.content.clone(),
            }),
            Role::Assistant => {
                // 空本文の assistant は出さない（#29 — 空発話を積むと次のターンが 400）。
                if !message.content.is_empty() {
                    input.push(wire::ResponsesInputItem::Message {
                        role: "assistant",
                        content: message.content.clone(),
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
