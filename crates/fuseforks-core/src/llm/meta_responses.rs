//! Meta Responses（`api.meta.ai/v1/responses`）の encode / decode（Spec 37）。
//!
//! `api.meta.ai` の web 検索は Responses にしか無い。互換の口
//! （`/v1/chat/completions`）には露出しておらず、gemini / xai_responses /
//! openai_responses と**同じ構図の 4 例目**。
//!
//! 凍結は `data_contract.yaml` の `meta_responses` が正。実装に効く要点:
//!
//! - **ストリーミングは不要**（提示された curl の `stream: true` /
//!   `Accept: text/event-stream` は**どちらも省いて 200**。実測 2026-08-13）。
//!   この村に SSE は 1 行も無いので、ここが必須なら規模が桁で変わっていた
//! - **`tool_choice` は `"auto"` のみ** — `none` / `required` / 名指しは 400。
//!   [`wire::MetaResponsesRequest`] は**欄そのものを持たない**ので送りようがない
//! - **受理集合を列挙するかは層で違う**（2026-08-13 に前半を実測で訂正）。
//!   **スカラの enum 欄は全列挙する** — `reasoning.effort: max` を送ったら
//!   `` expected one of `none`, `minimal`, `low`, `medium`, `high`, `xhigh` ``
//!   が返り、Spec 34 の「誤った値を 1 つ送ると 400 が教える」手筋がそのまま効いた。
//!   **列挙しないのは content の型のほう**（`input_video` は「どの型にも一致しない」
//!   としか言わない）。**旧記述は content 側の観察を欄全般へ広げていた。**
//!   測っていない**型**は送らない、が正しい射程
//! - **4 種別すべてを運ぶ**（Gemini に次ぐ 2 本目）。`input_audio` と
//!   `input_video` はこのワイヤだけが持つ

use super::canonical::{
    ChatRequest, ChatResponse, Effort, Finish, Grounding, GroundingEngine, GroundingSource,
    PromptAttachment, ToolCall, ToolChoice, Usage,
};
use super::error::LlmError;
use super::{responses_input, wire};
use crate::attachment::AttachmentKind;
use serde_json::Value;

/// 送る検索ツールの `type`。
const WEB_SEARCH_TOOL: &str = "web_search";

/// 検索呼び出しとして受ける output 種別。
const SEARCH_CALL_KIND: &str = "web_search_call";

/// 添付 1 件を Meta の part へ写す（**このワイヤだけが 4 種別すべてを持つ**）。
///
/// **`Provider::carries` を読まない**（Spec 37 D2）— 読むと
/// `adapters_match_the_carries_table` が同語反復になって網が死ぬ。
/// ここは「自分が何を組み立てられるか」の独立した主張で、表との一致は
/// テストが確かめる。
///
/// 形が社ごとに揃っていない点が 2 つある（実測）:
/// - **音声は data URL ではなく生の base64 + 短い形式名**（`input_image` と非対称）
/// - **動画は `video_url` に data URL** を入れる。`file_id` の経路もあるが、
///   そちらは**数値 ID** で `/v1/files` への事前アップロードが要る
///   （この村は事前アップロードを持たない）
pub(super) fn attachment_part(
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
        AttachmentKind::Audio => {
            attachment
                .media_type
                .audio_format()
                .map(|format| wire::ResponsesInputPart::InputAudio {
                    input_audio: wire::ResponsesInputAudio {
                        data: attachment.data.clone(),
                        format: format.to_owned(),
                    },
                })
        }
        AttachmentKind::Video => Some(wire::ResponsesInputPart::InputVideo {
            video_url: attachment.data_url(),
        }),
    }
}

/// canonical の思考段階 → Meta が受理する値。
///
/// **`max` だけがはみ出す。** 受理集合はサーバー自身が 400 で全列挙した
/// （2026-08-13 実機）:
///
/// ```text
/// `reasoning.effort`: unknown variant `max`,
///  expected one of `none`, `minimal`, `low`, `medium`, `high`, `xhigh`
/// ```
///
/// **丸め先が `high` ではなく `xhigh` なのは、Meta の最上位が `xhigh` だから。**
/// `openai_compat` が `high` へ落とすのは、そちらの相手が `xhigh` を受けないため
/// （同じ「丸め」でも落とす先はプロバイダの天井で決まる）。
///
/// `none` / `minimal` は canonical に対応する段階が無いので送らない。**送れない
/// 値があること自体は害ではない** — 害は、受理されない値を送れてしまうこと。
///
/// **`match` はワイルドカードを使わない。** 段階が 6 つ目になったとき、
/// コンパイラにここを指させる（Spec 37 で `Provider::carries` が種別側の
/// ワイルドカードで新 variant を黙って吸った、その裏返し）。
fn meta_effort(effort: Effort) -> &'static str {
    match effort {
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
        Effort::XHigh => "xhigh",
        // 受理されない唯一の値。天井へ寄せる。
        Effort::Max => "xhigh",
    }
}

/// canonical → Meta Responses wire。
///
/// `use_tools` が偽、または `tool_choice` が [`ToolChoice::None`] のときは
/// **関数ツールも検索ツールも送らない**（xAI / OpenAI と同じ規律 —
/// 「ツールを使わせない」は server-side tool にも及ぶ）。
///
/// **このワイヤでは、その分岐が `tool_choice` を送れないことの逃げ道も兼ねる** —
/// `ToolChoice::None` を表す欄が無いので、**ツールごと出さない**ことで表す。
/// `Required` / `Specific` は表現できず `Auto` と同じ扱いになる
/// （このワイヤは強制ツール呼び出しを持てない）。
pub fn encode(
    req: &ChatRequest,
    use_tools: bool,
    web_search: bool,
) -> wire::MetaResponsesRequest {
    let offer_tools = use_tools && req.tool_choice != ToolChoice::None;
    let mut tools = Vec::new();
    if offer_tools {
        if web_search {
            tools.push(wire::ResponsesTool::Server {
                kind: WEB_SEARCH_TOOL,
            });
        }
        for spec in &req.tools {
            tools.push(wire::ResponsesTool::Function {
                kind: "function",
                name: spec.name.clone(),
                description: spec.description.clone(),
                parameters: spec.parameters.clone(),
            });
        }
    }

    wire::MetaResponsesRequest {
        model: req.model.clone(),
        input: responses_input::encode(&req.messages, attachment_part),
        tools,
        // `max` だけ `xhigh` へ丸める（[`meta_effort`] を参照）。**旧版はここで
        // `e.as_str()` をそのまま送っており、「`medium` は実測で通った」から
        // 全段階が通ると書いていた。** 1 値の実測を集合全体へ広げた誤りで、
        // 実機の `max` が `fatal=true` の 400 で個体ごと落ちて分かった。
        // `openai_compat::reasoning_effort` を呼ばないのは openai_responses と同じ
        // 理由 — あの `"none"` 強制は chat/completions 固有の制約への対処。
        reasoning: req.effort.map(|e| wire::MetaReasoning {
            effort: meta_effort(e),
        }),
        max_output_tokens: req.max_tokens,
        temperature: req.temperature,
    }
}

/// Meta Responses wire → canonical。
///
/// 応答の形は他の Responses 2 本と同じなので [`wire::ResponsesResponse`] を共有する
/// （`kinds=['reasoning','message',…]` を実測）。
pub fn decode(resp: wire::ResponsesResponse) -> Result<ChatResponse, LlmError> {
    let mut texts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    let mut grounding = Grounding {
        engine: GroundingEngine::Meta,
        ..Grounding::default()
    };
    let mut search_calls = 0u32;
    let mut reasoning_summary: Vec<String> = Vec::new();
    let mut dropped: Vec<String> = Vec::new();

    for item in resp.output {
        match item.kind.as_str() {
            // **`phase` 欄は読まない**（`intentionally_unread`）。実測では
            // `phase: "commentary"` の message が本文の前に 1 通来ることがあり、
            // それはモデルが自分の段取りを述べたもの。**本文として繋ぐ** —
            // 他社の message と同じ扱いで、ここだけ独自の選別を持ち込むと
            // 「なぜこの社だけ発言が消えるのか」が画面から読めなくなる。
            // 実運用で雑音だと分かったら、そのとき観測を根拠に落とす。
            "message" => {
                for part in item.content.unwrap_or_default() {
                    if let Some(text) = part.text
                        && !text.is_empty()
                    {
                        texts.push(text);
                    }
                    for ann in part.annotations {
                        if ann.kind != "url_citation" {
                            continue;
                        }
                        let Some(url) = ann.url else { continue };
                        if grounding.sources.iter().any(|s| s.uri == url) {
                            continue;
                        }
                        let title = ann.title.filter(|t| t != &url).unwrap_or_default();
                        grounding.sources.push(GroundingSource { uri: url, title });
                    }
                }
            }
            // 思考の要約。**捨てていないので `dropped` へは入れない**。
            "reasoning" => {
                for part in item.summary.iter().flatten() {
                    if part.kind != "summary_text" {
                        continue;
                    }
                    if let Some(text) = part.text.as_ref()
                        && !text.is_empty()
                    {
                        reasoning_summary.push(text.clone());
                    }
                }
            }
            SEARCH_CALL_KIND => {
                search_calls += 1;
                if let Some(query) = item.action.as_ref().and_then(|a| a.query.clone())
                    && !grounding.queries.contains(&query)
                {
                    grounding.queries.push(query);
                }
            }
            "function_call" => {
                let raw = item.arguments.unwrap_or_default();
                let args: Value = serde_json::from_str(&raw).map_err(|source| LlmError::Parse {
                    source,
                    raw: raw.clone(),
                })?;
                tool_calls.push(ToolCall {
                    id: item.call_id.unwrap_or_default(),
                    name: item.name.unwrap_or_default(),
                    args,
                    extra: None,
                });
            }
            other => dropped.push(other.to_owned()),
        }
    }

    let usage = resp
        .usage
        .as_ref()
        .map(|u| Usage {
            prompt: u.input_tokens,
            completion: u.output_tokens,
            // **両方とも実在する**（実測 — `input_tokens_details.cached_tokens` /
            // `output_tokens_details.reasoning_tokens`）。Spec 32 の思考トークン
            // 計上とキャッシュ率がそのまま効く。
            cache_read: u
                .input_tokens_details
                .as_ref()
                .map(|d| d.cached_tokens)
                .unwrap_or_default(),
            // **`ResponsesInputTokensDetails` は 3 本の Responses ワイヤで共有**なので、
            // 欄を 1 つ足すと 3 本に効く。ただし decode の式は 3 ファイルに別々に
            // 書かれているので、テストも 3 本要る（Spec 40 P1）。
            cache_write: u
                .input_tokens_details
                .as_ref()
                .map(|d| d.cache_write_tokens)
                .unwrap_or_default(),
            cache_write_1h: 0,
            reasoning: u
                .output_tokens_details
                .as_ref()
                .map(|d| d.reasoning_tokens)
                .unwrap_or_default(),
        })
        .unwrap_or_default();

    // 未知種別は数えてから捨てる（#72）。
    if !dropped.is_empty() {
        crate::note!(
            "dropped content blocks: kinds={} count={} output_tokens={} text_chars={} tool_calls={}",
            dropped.join("+"),
            dropped.len(),
            usage.completion,
            texts.iter().map(|t| t.chars().count()).sum::<usize>(),
            tool_calls.len(),
        );
    }

    // 検索の計器。**行名は engine ごとに分ける**（`xai search:` /
    // `openai search:` と同じ理由 — 過去の観測記録がその文字列を指す）。
    //
    // **`actions=` と `sources=` の内訳は書かない** — Meta の `web_search_call` に
    // `action.sources` が在るかを測っていない。**無い欄を `-` で埋めると
    // 「測ったが無かった」と「見ていない」を畳む**（Spec 34 の ticks と同じ判断）。
    if search_calls > 0 {
        crate::note!(
            "meta search: calls={} sources={} queries={}",
            search_calls,
            grounding.sources.len(),
            grounding.queries.len(),
        );
    }

    let text = if texts.is_empty() {
        None
    } else {
        Some(texts.join("\n\n"))
    };

    let finish = if !tool_calls.is_empty() {
        Finish::ToolUse
    } else {
        match resp.status.as_deref() {
            Some("completed") => Finish::Stop,
            Some("incomplete")
                if resp
                    .incomplete_details
                    .as_ref()
                    .and_then(|d| d.reason.as_deref())
                    == Some("max_output_tokens") =>
            {
                Finish::Length
            }
            _ => Finish::Other,
        }
    };

    Ok(ChatResponse {
        text,
        tool_calls,
        finish,
        usage,
        grounding,
        reasoning_summary,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ChatMessage;

    /// Meta 自身が 400 で全列挙した受理集合（2026-08-13 実機）。
    ///
    /// **この配列は実測の写し。** Spec 37 は「Meta は受理集合を列挙しない」と
    /// 凍結したが、それが当たるのは content の型（`input_video` は「どの型にも
    /// 一致しない」としか言わない）で、**スカラの enum 欄は全列挙する**。
    const ACCEPTED: [&str; 6] = ["none", "minimal", "low", "medium", "high", "xhigh"];

    #[test]
    fn every_effort_maps_into_the_set_meta_accepts() {
        for effort in [
            Effort::Low,
            Effort::Medium,
            Effort::High,
            Effort::XHigh,
            Effort::Max,
        ] {
            let sent = meta_effort(effort);
            assert!(
                ACCEPTED.contains(&sent),
                "{effort:?} が受理されない値 `{sent}` になる（400 でターンごと落ちる）"
            );
        }
        assert_eq!(meta_effort(Effort::Max), "xhigh", "max は天井へ寄せる");
    }

    /// **`encode` が実際に丸めを通ること。**
    ///
    /// 述語だけ検査しても、`encode` が `e.as_str()` のままなら 400 は再発する。
    /// 提示側だけ守って判定側を守らない形（Spec 27 P1 で踏んだ穴）を塞ぐ。
    #[test]
    fn encode_routes_effort_through_the_clamp() {
        let mut req = ChatRequest::plain("m", vec![ChatMessage::user("問い")], 64);
        req.effort = Some(Effort::Max);
        assert_eq!(
            encode(&req, true, false).reasoning.map(|r| r.effort),
            Some("xhigh"),
            "encode が丸めを通していない（max が素通りしている）"
        );
    }
}
