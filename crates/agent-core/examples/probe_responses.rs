//! Spec 34 P0a の使い捨て probe（**Phase 終了時に削除する**。Spec 31 / 32 と同じ流儀）。
//!
//! D10 の順序どおりに撃つ。**`reasoning.summary` を最後に回す**のが要点で、
//! 未検証の組織では 400 が返り、前に置くと関数往復まで巻き添えで未測定になる。
//!
//! 実行: `OPENAI_API_KEY` / `XAI_API_KEY` を env に置いて
//! `cargo run -p agent-core --example probe_responses`
//!
//! **鍵は表示しない**（ヘッダへ入れるだけ）。応答本文は種別と欄名だけを出し、
//! 本文そのものは字数で出す（`failures.md` #71 — 計器は秘密の転送経路になる）。

use serde_json::{Value, json};

const OPENAI: &str = "https://api.openai.com/v1/responses";
const XAI: &str = "https://api.x.ai/v1/responses";
const MODEL: &str = "gpt-5.6-terra";
const XAI_MODEL: &str = "grok-4.5";

/// 1 発ぶん。戻すのは (HTTP status, 生 JSON)。
async fn shot(
    http: &reqwest::Client,
    label: &str,
    url: &str,
    key: &str,
    body: &Value,
) -> Option<Value> {
    println!("\n===== {label} =====");
    println!("send: {}", compact_request(body));

    let resp = match http.post(url).bearer_auth(key).json(body).send().await {
        Ok(r) => r,
        Err(err) => {
            println!("TRANSPORT ERROR: {err}");
            return None;
        }
    };
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    println!("HTTP {status}");

    if !status.is_success() {
        // 400 の本文はどのパラメータが拒まれたかを名指しする（#76 / #77 の診断経路）。
        println!("body: {}", &text.chars().take(700).collect::<String>());
        return None;
    }

    // 条件 2（D2 の分岐点）: 既存の xAI 型でそのまま読めるか。
    match serde_json::from_str::<agent_core::llm::wire::XaiResponse>(&text) {
        Ok(_) => println!("serde into wire::XaiResponse: OK"),
        Err(err) => println!("serde into wire::XaiResponse: **FAILED** {err}"),
    }

    let raw: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    describe(&raw);
    Some(raw)
}

/// 送ったボディの要点だけ（本文は字数）。
fn compact_request(body: &Value) -> String {
    let mut parts = Vec::new();
    for key in ["store", "temperature", "max_output_tokens"] {
        if let Some(v) = body.get(key) {
            parts.push(format!("{key}={v}"));
        }
    }
    if let Some(r) = body.get("reasoning") {
        parts.push(format!("reasoning={r}"));
    }
    if let Some(t) = body.get("tools").and_then(|t| t.as_array()) {
        let names: Vec<String> = t
            .iter()
            .map(|t| {
                let kind = t.get("type").and_then(Value::as_str).unwrap_or("?");
                match t.get("name").and_then(Value::as_str) {
                    Some(name) => format!("{kind}:{name}"),
                    None => kind.to_owned(),
                }
            })
            .collect();
        parts.push(format!("tools=[{}]", names.join(",")));
    }
    if let Some(i) = body.get("input").and_then(|i| i.as_array()) {
        parts.push(format!("input_items={}", i.len()));
    }
    parts.join(" ")
}

/// 応答の構造を出す。**本文は字数だけ**。
fn describe(raw: &Value) {
    println!(
        "status={:?} incomplete={:?}",
        raw.get("status").and_then(Value::as_str),
        raw.get("incomplete_details")
    );

    let Some(items) = raw.get("output").and_then(Value::as_array) else {
        println!("output: (無し) top-level keys={:?}", keys(raw));
        return;
    };

    for (i, item) in items.iter().enumerate() {
        let kind = item.get("type").and_then(Value::as_str).unwrap_or("?");
        print!("  [{i}] {kind}");
        print!(" keys={:?}", keys(item));

        if kind == "reasoning" {
            // T1: stateless で encrypted_content が既定で来るか。
            let enc = item
                .get("encrypted_content")
                .and_then(Value::as_str)
                .map(|s| s.chars().count());
            let summaries: Vec<usize> = item
                .get("summary")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|p| p.get("text").and_then(Value::as_str))
                        .map(|t| t.chars().count())
                        .collect()
                })
                .unwrap_or_default();
            print!(
                " encrypted_content_chars={enc:?} summary_text_chars={summaries:?} id={:?}",
                item.get("id").and_then(Value::as_str)
            );
        }
        if kind == "message" {
            let chars: usize = item
                .get("content")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|p| p.get("text").and_then(Value::as_str))
                        .map(|t| t.chars().count())
                        .sum()
                })
                .unwrap_or(0);
            print!(" text_chars={chars}");
        }
        if kind == "function_call" {
            print!(
                " name={:?} call_id={:?} args_chars={:?}",
                item.get("name").and_then(Value::as_str),
                item.get("call_id").and_then(Value::as_str),
                item.get("arguments").and_then(Value::as_str).map(|s| s.len())
            );
        }
        if kind.contains("search") {
            // T5: action は search / open_page / find_in_page の 3 種。
            print!(" action={:?}", item.get("action"));
        }
        println!();
    }

    if let Some(u) = raw.get("usage") {
        println!("  usage={u}");
    }
}

fn keys(v: &Value) -> Vec<&str> {
    v.as_object()
        .map(|o| o.keys().map(String::as_str).collect())
        .unwrap_or_default()
}

fn user(text: &str) -> Value {
    json!({"role": "user", "content": text})
}

fn price_tool() -> Value {
    json!({
        "type": "function",
        "name": "get_price",
        "description": "銘柄の現在価格を返す",
        "parameters": {
            "type": "object",
            "properties": {"symbol": {"type": "string"}},
            "required": ["symbol"]
        }
    })
}

#[tokio::main]
async fn main() {
    let openai_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY が未設定");
    let xai_key = std::env::var("XAI_API_KEY").expect("XAI_API_KEY が未設定");
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .expect("http client");

    // 2 段目だけ撃つ（1 段目の結果は Spec の P0a 記録に写してある）。
    if std::env::var("PROBE_STAGE").as_deref() == Ok("2") {
        // S8: `mode: pro`（S5 は standard しか測っていなかった）。
        shot(
            &http,
            "S8 reasoning.mode = pro",
            OPENAI,
            &openai_key,
            &json!({
                "model": MODEL,
                "input": [user("1 たす 1 は？ 数字だけ答えて。")],
                "reasoning": {"effort": "low", "mode": "pro"},
                "max_output_tokens": 2048,
                "store": false
            }),
        )
        .await;

        // S9 / S10: 検索ツールの固定費を 1 本ずつ切り分ける。
        // S4 は 2 本まとめて input=4,454 だった（素の S1 は 20）。
        for kind in ["web_search", "web_search_preview"] {
            shot(
                &http,
                &format!("S9 {kind} 単独の固定費"),
                OPENAI,
                &openai_key,
                &json!({
                    "model": MODEL,
                    "input": [user("OK とだけ答えて。検索はしないで。")],
                    "tools": [{"type": kind}],
                    "max_output_tokens": 512,
                    "store": false
                }),
            )
            .await;
        }
        println!("\n===== 2 段目 終わり =====");
        return;
    }

    // 3 段目: 条件 5 の測り漏らし。S4 / S9 は検索ツールだけで、
    // **関数ツールを混ぜていなかった**（xAI では同居したが、それは xAI であって
    // OpenAI ではない — #93 と同じ形の取り違えを自分でやりかけた）。
    if std::env::var("PROBE_STAGE").as_deref() == Ok("3") {
        shot(
            &http,
            "S11 検索ツールと関数ツールの同居（条件 5）",
            OPENAI,
            &openai_key,
            &json!({
                "model": MODEL,
                "input": [user("AAPL の現在価格を調べて。必ず get_price を使うこと。")],
                "tools": [{"type": "web_search"}, price_tool()],
                "max_output_tokens": 2048,
                "store": false
            }),
        )
        .await;
        println!("\n===== 3 段目 終わり =====");
        return;
    }

    // ---- S1: 素の 1 発（条件 1 / 2 / 6 の一部 / T1）----
    // reasoning も tools も temperature も送らない。ここが落ちたらワイヤごと不成立。
    let s1 = shot(
        &http,
        "S1 素（store:false・reasoning 送らず）",
        OPENAI,
        &openai_key,
        &json!({
            "model": MODEL,
            "input": [user("1 たす 1 は？ 数字だけ答えて。")],
            "max_output_tokens": 2048,
            "store": false
        }),
    )
    .await;
    if s1.is_none() {
        println!("\nS1 が落ちた。以降は測っても解釈できないので止める。");
        return;
    }

    // ---- S2: 関数ツール（条件 3 = #77 の 400 が出ないか）----
    let s2 = shot(
        &http,
        "S2 関数ツール（#77 の併用制約が消えているか）",
        OPENAI,
        &openai_key,
        &json!({
            "model": MODEL,
            "input": [user("AAPL の現在価格を調べて。必ずツールを使うこと。")],
            "tools": [price_tool()],
            "max_output_tokens": 2048,
            "store": false
        }),
    )
    .await;

    // ---- S3: 関数の往復（条件 4 / 4' = reasoning item の再送を要求されるか）----
    // S2 の実 call_id を使う。reasoning item は **返さない**（要求されるかを見る）。
    if let Some(raw) = &s2 {
        let call = raw
            .get("output")
            .and_then(Value::as_array)
            .and_then(|a| a.iter().find(|i| i.get("type").and_then(Value::as_str) == Some("function_call")))
            .cloned();

        if let Some(call) = call {
            let call_id = call.get("call_id").and_then(Value::as_str).unwrap_or("");
            let args = call.get("arguments").and_then(Value::as_str).unwrap_or("{}");
            shot(
                &http,
                "S3 関数の往復（reasoning を返さずに function_call_output だけ返す）",
                OPENAI,
                &openai_key,
                &json!({
                    "model": MODEL,
                    "input": [
                        user("AAPL の現在価格を調べて。必ずツールを使うこと。"),
                        {"type": "function_call", "call_id": call_id, "name": "get_price", "arguments": args},
                        {"type": "function_call_output", "call_id": call_id, "output": "{\"price\": 231.5}"}
                    ],
                    "tools": [price_tool()],
                    "max_output_tokens": 2048,
                    "store": false
                }),
            )
            .await;
        } else {
            println!("\n[S3 skip] S2 が function_call を返さなかった");
        }
    }

    // ---- S4: 検索ツールの type 2 種（条件 5'）----
    // **検索させない短い問い**で型の受理だけを見る（本当に検索させると input が 10 万規模）。
    // **temperature は混ぜない** — 400 になったとき、型の答えごと失われる。
    // 「400 の本文がパラメータを名指しするから混ぜてよい」は誤り:
    // 名指しされるのは**最初に落ちた 1 つ**で、後ろは検査されずに終わる（#76 の
    // 「1 件直した時点では直ったと言えない」がそのまま当たる）。
    shot(
        &http,
        "S4 web_search と web_search_preview の受理",
        OPENAI,
        &openai_key,
        &json!({
            "model": MODEL,
            "input": [user("OK とだけ答えて。検索はしないで。")],
            "tools": [{"type": "web_search"}, {"type": "web_search_preview"}],
            "max_output_tokens": 512,
            "store": false
        }),
    )
    .await;

    // ---- S4b: temperature 単独（条件 8）----
    shot(
        &http,
        "S4b temperature 単独",
        OPENAI,
        &openai_key,
        &json!({
            "model": MODEL,
            "input": [user("OK とだけ答えて。")],
            "temperature": 0.7,
            "max_output_tokens": 512,
            "store": false
        }),
    )
    .await;

    // ---- S5: reasoning の 4 欄のうち summary 以外（条件 7 / T4）----
    shot(
        &http,
        "S5 reasoning{effort,mode,context}（summary は送らない）",
        OPENAI,
        &openai_key,
        &json!({
            "model": MODEL,
            "input": [user("1 たす 1 は？ 数字だけ答えて。")],
            "reasoning": {"effort": "low", "mode": "standard", "context": "current_turn"},
            "max_output_tokens": 2048,
            "store": false
        }),
    )
    .await;

    // ---- S6: reasoning.summary（条件 9。**最後に置く理由がここ**）----
    // 未検証の組織では 400。落ちても S1〜S5 の結果は残る。
    for value in ["auto", "detailed"] {
        let ok = shot(
            &http,
            &format!("S6 reasoning.summary = {value}"),
            OPENAI,
            &openai_key,
            &json!({
                "model": MODEL,
                "input": [user("3 人のうち嘘つきは 1 人。A は「B が嘘つき」、B は「C が嘘つき」、C は「A が嘘つき」と言った。誰が嘘つき？")],
                "reasoning": {"effort": "low", "summary": value},
                "max_output_tokens": 4096,
                "store": false
            }),
        )
        .await;
        if ok.is_none() {
            println!("[S6] {value} が落ちたので残りの値は撃たない");
            break;
        }
    }

    // ---- S7: xAI へ store:false（D3 の派生。既存の穴の確認）----
    shot(
        &http,
        "S7 xAI へ store:false（欄を受けるか）",
        XAI,
        &xai_key,
        &json!({
            "model": XAI_MODEL,
            "input": [user("1 たす 1 は？ 数字だけ答えて。")],
            "include": ["no_inline_citations"],
            "max_output_tokens": 2048,
            "store": false
        }),
    )
    .await;

    println!("\n===== probe 終わり =====");
}
