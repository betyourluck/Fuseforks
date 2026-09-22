//! TypeSafe Jev — 判断専用モデルの口（Cloudflare Workers AI 経由。Spec 59）。
//!
//! Jev は**文章を書かない**。状態と質問を受け取り、確率つきの型付き判断だけを
//! 並列で返す。ゆえに canonical な `ChatRequest` には**乗せない** — messages が
//! 無く tool-use でもないので、`Provider` の match（chat のための seam）へ足すと
//! canonical が汚れる。共有するのは HTTP と、再試行の分類
//! （[`crate::llm::retry::classify`]）だけ。
//!
//! 移植元は Kataribe の `crates/llm_client/src/jev.rs`（同じ API の 2 実装目）。
//! 写したのは形で、コードは書き直している。
//!
//! # 実測で決まった使い方（Kataribe 2026-09-21 / この村の P0 2026-09-22）
//!
//! - **1 質問 1 論点**。複合質問だと同じ違反で 0.28、分解すると 0.95 まで上がった
//! - **`criteria` を付ける**。付けないと精度が落ちる
//! - **`state` は構造化 JSON で渡す**。人間可読の 1 枚テキストにすると照合が効かない
//! - 質問は**並列・独立**に評価されるので、束ねてもレイテンシはほぼ伸びない
//! - **質問文と criteria は P0 で実測した文面の逐語**。一文の増減で 0.36 動く
//!   （Kataribe `failures.md` #104）ので、変えたら測り直す

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::llm::retry::{self, RetryClass, Verdict};
use crate::prune::{ParagraphScorer, ScoreError, ScoreReport};

/// Cloudflare Workers AI の既定オリジン。
pub const DEFAULT_BASE_URL: &str = "https://api.cloudflare.com/client/v4";
/// 既定のモデル名（Workers AI のパートナーモデル識別子）。
pub const DEFAULT_MODEL: &str = "typesafe/jev";
/// 1 束ねの再送回数。**実際の上限は締め切り**（`prune::DEADLINE` の 20 秒）で、
/// ここは「一過性なら 1 度は待つ」を表すだけ。
const MAX_RETRIES: u32 = 2;

/// 判定基準の「真」側。**P0 で実測した文面の逐語**。
const CRITERIA_YES: &str = "依頼に答える材料 (事実・数値・主張・手順・限界) が含まれる";
/// 判定基準の「偽」側。**P0 で実測した文面の逐語**。
const CRITERIA_NO: &str =
    "ナビゲーション・広告・定型文・著者欄・無関係な話題など、依頼に答える材料を含まない";

/// 段落 1 つに割り当てる質問 id。`state.chunks` の鍵と一致させる。
///
/// **4 桁の 0 詰め**なのは、質問文がこの id を逐語で名指しするため
/// （`chunks.c0000` のようにパス参照させると照合が効く）。
#[must_use]
pub fn chunk_id(index: usize) -> String {
    format!("c{index:04}")
}

/// 段落 1 つへの質問文。**P0 で実測した文面の逐語**。
#[must_use]
pub fn instructions(id: &str) -> String {
    format!("`chunks.{id}` の文章は、`request` の依頼に答えるために必要な情報を含むか。")
}

/// 接続設定。
///
/// **キーが無ければ機構ごと存在しない**（opt-in）。設定していない村に
/// 2 つ目の API 契約を強いない — `Shared.paragraph_scorer` が `None` のままなら
/// 圧縮の経路そのものが走らない。
#[derive(Debug, Clone)]
pub struct JevConfig {
    /// Cloudflare のアカウント ID（32 桁 hex）。
    pub account_id: String,
    /// API トークン。**`SecretStore` から渡す**（`jev.json` には書かない）。
    pub api_token: String,
    /// オリジン。既定は [`DEFAULT_BASE_URL`]。
    pub base_url: String,
    /// モデル名。既定は [`DEFAULT_MODEL`]。
    pub model: String,
    /// 閾値。この値**未満**の段落を落とす（`tool_prune_contract` の離散 4 値）。
    pub threshold: f32,
}

impl JevConfig {
    /// `POST` 先。`{base}/accounts/{id}/ai/run`。
    #[must_use]
    pub fn endpoint(&self) -> String {
        format!(
            "{}/accounts/{}/ai/run",
            self.base_url.trim_end_matches('/'),
            self.account_id
        )
    }
}

// ---- 送信ボディ（純関数で組む） ----

/// 送信ボディ。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct JevRequest<'a> {
    /// モデル名。
    pub model: &'a str,
    /// 状態と質問。
    pub input: JevInput<'a>,
}

/// `input`。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct JevInput<'a> {
    /// 判定の材料。質問から `chunks.c0000` のようにパス参照できる。
    pub state: JevState<'a>,
    /// 質問。id は [`chunk_id`]。
    pub questions: BTreeMap<String, NoulQuestion>,
}

/// `state`。**基準と段落だけ**（`tool_prune_contract` の D10）。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct JevState<'a> {
    /// 基準 = そのターンの依頼文の先頭 2,000 字。
    pub request: &'a str,
    /// 採点する段落。**6,000 字超の段落はここに入らない**（送らない）。
    pub chunks: BTreeMap<String, &'a str>,
}

/// 「この文は真か」を 0.0〜1.0 で答えさせる質問。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct NoulQuestion {
    /// 常に `"noul"`。Jev API は質問の型をこの欄で見る。
    #[serde(rename = "type")]
    ty: &'static str,
    /// 質問文。
    pub instructions: String,
    /// 判定基準。**常に付ける**（付けないと精度が落ちる）。
    pub criteria: NoulCriteria,
}

/// Noul の判定基準。**真偽それぞれに一文**。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct NoulCriteria {
    /// 真と判定する条件。
    #[serde(rename = "true")]
    pub yes: &'static str,
    /// 偽と判定する条件。
    #[serde(rename = "false")]
    pub no: &'static str,
}

/// 送信ボディを組む（純関数）。
///
/// `batch` は `(段落の index, 本文)` の組。id は [`chunk_id`] で作るので、
/// **`spread` の並びと独立に元の index へ戻せる**。
#[must_use]
pub fn encode<'a>(model: &'a str, basis: &'a str, batch: &[(usize, &'a str)]) -> JevRequest<'a> {
    let mut chunks = BTreeMap::new();
    let mut questions = BTreeMap::new();
    for (index, text) in batch {
        let id = chunk_id(*index);
        chunks.insert(id.clone(), *text);
        questions.insert(
            id.clone(),
            NoulQuestion {
                ty: "noul",
                instructions: instructions(&id),
                criteria: NoulCriteria {
                    yes: CRITERIA_YES,
                    no: CRITERIA_NO,
                },
            },
        );
    }
    JevRequest {
        model,
        input: JevInput {
            state: JevState {
                request: basis,
                chunks,
            },
            questions,
        },
    }
}

// ---- Cloudflare の二重包装を剥がす wire 型 ----

#[derive(Debug, Deserialize)]
struct CfEnvelope {
    #[serde(default)]
    result: Option<CfResult>,
}

#[derive(Debug, Deserialize)]
struct CfResult {
    #[serde(default)]
    result: Option<JevPayload>,
}

#[derive(Debug, Deserialize)]
struct JevPayload {
    #[serde(default)]
    model: String,
    #[serde(default)]
    answers: BTreeMap<String, RawAnswer>,
    #[serde(default)]
    usage: Option<JevUsage>,
}

#[derive(Debug, Deserialize)]
struct RawAnswer {
    #[serde(rename = "type", default)]
    ty: String,
    #[serde(default)]
    noul: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct JevUsage {
    #[serde(default)]
    input_tokens: u64,
}

/// 受理した答え。v1 は Noul のみ。
#[derive(Debug, Clone, PartialEq)]
pub struct JevAnswers {
    /// サーバーが名乗ったモデル版（例 `jev-1.13.0`）。診断用。
    pub model: String,
    /// 質問 id → 0.0〜1.0。**未回答は載せない**（0.0 へ潰さない）。
    pub scores: BTreeMap<String, f32>,
    /// 入力トークン。**`TurnSpend` にも予算にも載せない**（ログだけ）。
    pub input_tokens: u64,
}

/// 応答本文を解く（純関数）。
///
/// **`json()` 直ではなく text を受けて解く** — 2xx なのに形が違うとき、本文を
/// 捨てると「missing field」だけが残って真因が消える。
///
/// `type` が `noul` でない答えは載せない（v1 は Noul しか問わないので、
/// 呼び出し側は「返らなかった段落」= 残す、として受ける）。
///
/// # Errors
///
/// JSON として読めない、または `result` が無いとき。
pub fn decode(raw: &str) -> Result<JevAnswers, JevError> {
    let env: CfEnvelope =
        serde_json::from_str(raw).map_err(|e| JevError::Parse(format!("{e}: {}", head(raw))))?;
    let payload = env
        .result
        .and_then(|r| r.result)
        .ok_or_else(|| JevError::Parse(format!("応答に result が無い: {}", head(raw))))?;

    let scores = payload
        .answers
        .into_iter()
        .filter_map(|(k, a)| {
            if a.ty == "noul" {
                #[allow(clippy::cast_possible_truncation)]
                a.noul.map(|v| (k, v as f32))
            } else {
                None
            }
        })
        .collect();
    Ok(JevAnswers {
        model: payload.model,
        scores,
        input_tokens: payload.usage.map_or(0, |u| u.input_tokens),
    })
}

/// 本文の先頭だけ。**全文をエラーへ載せない**（ログへ出ないとはいえ、
/// 応答本文は外の文字列なので長さを縛る）。
fn head(raw: &str) -> String {
    raw.chars().take(300).collect()
}

/// Jev の失敗。
///
/// **文面は計器に出さない**（`failures.md` #71）。`tool prune:` の `outcome=` は
/// `failed` の 1 語だけで、ここに載る文字列はテストと診断のためにある。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JevError {
    /// 解釈できない応答（非 JSON・`result` が無い）。
    Parse(String),
    /// ステータスのエラー。`class` が再試行の可否を決める。
    Api {
        /// HTTP ステータス。
        status: u16,
        /// 再試行の分類（Spec 52 の表を共有する）。
        class: RetryClass,
    },
    /// 接続・タイムアウト。
    Http(String),
}

impl JevError {
    /// 再送で回復しうるか。**判定は Spec 52 の [`retry::verdict`] に委ねる** —
    /// ここで 2 つ目の表を書かない（402 は `ClientError` → `Stop` に落ちる）。
    #[must_use]
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Api { class, .. } => retry::verdict(*class) == Verdict::Retry,
            // 接続の失敗は一過性。解釈できない応答は再送しても同じ。
            Self::Http(_) => true,
            Self::Parse(_) => false,
        }
    }
}

impl std::fmt::Display for JevError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(detail) => write!(f, "Jev の応答を解釈できない: {detail}"),
            Self::Api { status, class } => {
                write!(f, "Jev が status={status} を返した (class={})", class.as_str())
            }
            Self::Http(detail) => write!(f, "Jev への接続に失敗: {detail}"),
        }
    }
}

/// Jev を叩く [`ParagraphScorer`]。
pub struct JevScorer {
    http: reqwest::Client,
    config: JevConfig,
}

impl JevScorer {
    /// 組む。
    ///
    /// # Errors
    ///
    /// HTTP クライアントを作れないとき。
    pub fn new(config: JevConfig) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .build()
            .map_err(|e| format!("HTTP クライアントを作れない: {e}"))?;
        Ok(Self { http, config })
    }

    /// サーバーへ名乗るモデル名（計器・テスト用）。
    #[must_use]
    pub fn model(&self) -> &str {
        &self.config.model
    }

    /// 1 束ねを投げる。一過性の失敗だけ再送する。
    async fn ask(&self, basis: &str, batch: &[(usize, &str)]) -> (usize, Result<JevAnswers, JevError>) {
        let mut calls = 0usize;
        let mut attempt = 0u32;
        loop {
            calls += 1;
            match self.ask_once(basis, batch).await {
                Ok(answers) => return (calls, Ok(answers)),
                Err(e) => {
                    if attempt >= MAX_RETRIES || !e.is_transient() {
                        return (calls, Err(e));
                    }
                    tokio::time::sleep(Duration::from_secs(1u64 << attempt)).await;
                    attempt += 1;
                }
            }
        }
    }

    async fn ask_once(
        &self,
        basis: &str,
        batch: &[(usize, &str)],
    ) -> Result<JevAnswers, JevError> {
        let body = encode(&self.config.model, basis, batch);
        let resp = self
            .http
            .post(self.config.endpoint())
            .bearer_auth(&self.config.api_token)
            .json(&body)
            .send()
            .await
            .map_err(|e| JevError::Http(e.to_string()))?;

        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            // 本文は**捨てる**。コードの照合には使わず、ログにも出さない
            // （Cloudflare の本文は受信ヘッダーをエコーすることがある = #71 の系譜）。
            return Err(JevError::Api {
                status,
                class: retry::classify(status, None),
            });
        }
        let raw = resp.text().await.map_err(|e| JevError::Http(e.to_string()))?;
        decode(&raw)
    }
}

#[async_trait::async_trait]
impl ParagraphScorer for JevScorer {
    /// 束ねて並列に投げ、段落の並びへ戻す。
    ///
    /// **束ね方は [`crate::prune::batch`] の 1 実装**を通す。並列は
    /// [`crate::prune::PARALLEL`] 本で、済んだものから次を立てる。
    ///
    /// 一部の束ねだけ失敗したときは**その段落を `None`（= 残す）にして続ける**。
    /// 全部失敗したときだけ [`ScoreError::Failed`] — 畳むと
    /// 「1 つも落ちなかった」と区別が付かなくなる（`failures.md` #72 の規律）。
    async fn score(&self, basis: &str, paragraphs: &[&str]) -> Result<ScoreReport, ScoreError> {
        let groups = crate::prune::batch(paragraphs);
        let mut report = ScoreReport {
            scores: vec![None; paragraphs.len()],
            calls: 0,
            tokens: 0,
        };
        if groups.is_empty() {
            return Ok(report);
        }

        let mut ok = 0usize;
        let mut failed = 0usize;
        // 済んだものから次を立てる。`chunks` は 4 本ずつの窓ではなく、
        // **常に 4 本走っている**形にしたいので手で詰める。
        let mut queue = groups.into_iter();
        let mut flight: Vec<_> = Vec::new();
        loop {
            while flight.len() < crate::prune::PARALLEL {
                let Some(group) = queue.next() else { break };
                flight.push(Box::pin(self.run_group(basis, paragraphs, group)));
            }
            if flight.is_empty() {
                break;
            }
            let (done, index, _) = futures_select(&mut flight).await;
            // 済んだ future を外す（返り値は使い終わっている）。
            drop(flight.swap_remove(index));
            let (group, calls, result) = done;
            report.calls += calls;
            match result {
                Ok(answers) => {
                    ok += 1;
                    report.tokens += answers.input_tokens;
                    for i in group {
                        if let Some(score) = answers.scores.get(&chunk_id(i)) {
                            report.scores[i] = Some(*score);
                        }
                    }
                }
                Err(_) => failed += 1,
            }
        }

        if ok == 0 && failed > 0 {
            return Err(ScoreError::Failed(format!("{failed} 本の束ねが全部失敗した")));
        }
        Ok(report)
    }

    fn threshold(&self) -> f32 {
        self.config.threshold
    }
}

impl JevScorer {
    /// 1 束ねを投げ、`(段落 index の列, 呼び出し回数, 結果)` を返す。
    async fn run_group(
        &self,
        basis: &str,
        paragraphs: &[&str],
        group: Vec<usize>,
    ) -> (Vec<usize>, usize, Result<JevAnswers, JevError>) {
        let batch: Vec<(usize, &str)> = group.iter().map(|i| (*i, paragraphs[*i])).collect();
        let (calls, result) = self.ask(basis, &batch).await;
        (group, calls, result)
    }
}

/// 飛んでいる future のうち**最初に終わったもの**を返す。
///
/// `futures` crate を足さずに済ませるための最小実装（`tokio-util` を直接化した
/// ときと同じ判断 — 依存はツリーに居ても、この 1 箇所のために表へ足さない）。
/// 返るのは `(値, その future の位置, 残り本数)`。
async fn futures_select<F, T>(flight: &mut [F]) -> (T, usize, usize)
where
    F: std::future::Future<Output = T> + Unpin,
{
    let len = flight.len();
    std::future::poll_fn(|cx| {
        for (i, fut) in flight.iter_mut().enumerate() {
            if let std::task::Poll::Ready(v) = std::pin::Pin::new(fut).poll(cx) {
                return std::task::Poll::Ready((v, i, len));
            }
        }
        std::task::Poll::Pending
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> JevConfig {
        JevConfig {
            account_id: "abc123".into(),
            api_token: "t".into(),
            base_url: format!("{DEFAULT_BASE_URL}/"),
            model: DEFAULT_MODEL.into(),
            threshold: 0.2,
        }
    }

    /// `base_url` の末尾スラッシュを重複させない。
    #[test]
    fn endpoint_is_built_from_account_id() {
        assert_eq!(
            cfg().endpoint(),
            "https://api.cloudflare.com/client/v4/accounts/abc123/ai/run"
        );
    }

    /// 送信ボディが Jev API の形になる。**質問文と criteria は逐語で留める** —
    /// 一文の増減で 0.36 動くので、変えるときは測り直す必要がある。
    #[test]
    fn encode_matches_the_measured_shape() {
        let batch = [(0usize, "本文 A"), (13usize, "本文 B")];
        let body = serde_json::to_value(encode(DEFAULT_MODEL, "依頼文", &batch)).unwrap();

        assert_eq!(body["model"], "typesafe/jev");
        assert_eq!(body["input"]["state"]["request"], "依頼文");
        assert_eq!(body["input"]["state"]["chunks"]["c0000"], "本文 A");
        assert_eq!(body["input"]["state"]["chunks"]["c0013"], "本文 B");

        let asked = &body["input"]["questions"]["c0013"];
        assert_eq!(asked["type"], "noul");
        assert_eq!(
            asked["instructions"],
            "`chunks.c0013` の文章は、`request` の依頼に答えるために必要な情報を含むか。"
        );
        assert_eq!(
            asked["criteria"]["true"],
            "依頼に答える材料 (事実・数値・主張・手順・限界) が含まれる"
        );
        assert_eq!(
            asked["criteria"]["false"],
            "ナビゲーション・広告・定型文・著者欄・無関係な話題など、依頼に答える材料を含まない"
        );
    }

    /// **送るのは基準と段落の 2 つだけ**（D10）。ボディに他の欄を増やさない。
    #[test]
    fn the_body_carries_only_the_basis_and_the_paragraphs() {
        let body = serde_json::to_value(encode(DEFAULT_MODEL, "依頼文", &[(0, "本文")])).unwrap();
        // 並びは serde の挿入順なので**揃えてから**比べる（見たいのは欄の集合）。
        let keys = |v: &serde_json::Value| {
            let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
            k.sort();
            k
        };
        assert_eq!(keys(&body), ["input", "model"]);
        assert_eq!(keys(&body["input"]), ["questions", "state"]);
        assert_eq!(keys(&body["input"]["state"]), ["chunks", "request"]);
    }

    /// Cloudflare の二重包装を剥がす。本文は 2026-09-22 に実際に受け取った形。
    #[test]
    fn decode_unwraps_the_cloudflare_envelope() {
        let raw = r#"{
          "result": {
            "state": "Completed",
            "result": {
              "model": "jev-1.13.0",
              "answers": {
                "c0000": { "type": "noul", "noul": 0.93 },
                "c0001": { "type": "noul", "noul": 0.1 }
              },
              "usage": { "input_tokens": 931, "output_tokens": 0 }
            }
          },
          "success": true, "errors": [], "messages": []
        }"#;
        let got = decode(raw).expect("decode");
        assert_eq!(got.model, "jev-1.13.0");
        assert!((got.scores["c0000"] - 0.93).abs() < 1e-6);
        assert!((got.scores["c0001"] - 0.1).abs() < 1e-6);
        assert!(
            !got.scores.contains_key("c0002"),
            "未回答は載せない（0.0 に潰さない）"
        );
        assert_eq!(got.input_tokens, 931);
    }

    /// `usage` が無い応答でも落ちない（計器の欠落で採点を失わせない）。
    #[test]
    fn decode_tolerates_a_missing_usage() {
        let raw = r#"{"result":{"result":{"model":"jev-1.13.0","answers":{"c0000":{"type":"noul","noul":0.5}}}},"success":true}"#;
        let got = decode(raw).expect("decode");
        assert!((got.scores["c0000"] - 0.5).abs() < 1e-6);
        assert_eq!(got.input_tokens, 0);
    }

    /// `noul` でない答えは載せない（呼び出し側は「返らなかった」= 残すで受ける）。
    #[test]
    fn decode_drops_answers_of_another_type() {
        let raw = r#"{"result":{"result":{"answers":{"c0000":{"type":"choice","choice":"a"},"c0001":{"type":"noul","noul":0.4}}}}}"#;
        let got = decode(raw).expect("decode");
        assert!(!got.scores.contains_key("c0000"));
        assert!(got.scores.contains_key("c0001"));
    }

    /// 非 JSON の本文は先頭ごと surface する（捨てると真因が消える）。
    #[test]
    fn decode_keeps_the_head_of_a_broken_body() {
        let err = decode("<html>502 Bad Gateway</html>").unwrap_err();
        match err {
            JevError::Parse(detail) => assert!(detail.contains("502 Bad Gateway")),
            other => panic!("Parse を期待したが {other:?}"),
        }
    }

    /// 200 なのに `result` が無い形も Parse として本文ごと出す。
    #[test]
    fn decode_reports_a_missing_result() {
        let err = decode(r#"{"success":true,"errors":[],"result":null}"#).unwrap_err();
        match err {
            JevError::Parse(detail) => assert!(detail.contains("result が無い")),
            other => panic!("Parse を期待したが {other:?}"),
        }
    }

    /// **クレジット不足（402）は一過性でない** — 残高は再送で増えない。
    /// 判定は Spec 52 の表に委ねてあるので、ここで 2 つ目の規則を持たない。
    #[test]
    fn a_payment_error_is_not_transient() {
        let paid = JevError::Api {
            status: 402,
            class: retry::classify(402, None),
        };
        assert!(!paid.is_transient(), "402 を再送してはいけない");

        // 対照: 429 と 5xx は一過性なので再送する。
        for status in [429u16, 503] {
            let e = JevError::Api {
                status,
                class: retry::classify(status, None),
            };
            assert!(e.is_transient(), "status={status} は再送する");
        }
        // 解釈できない応答は再送しても同じ。
        assert!(!JevError::Parse("x".into()).is_transient());
    }

    /// 実鍵で 1 回だけ叩く。**既定では走らない**（`--ignored` で明示したときだけ）。
    ///
    /// ```text
    /// JEV_ACCOUNT_ID=... JEV_API_TOKEN=... \
    ///   cargo test -p fuseforks-core --lib jev::tests::live -- --ignored --nocapture
    /// ```
    #[tokio::test]
    #[ignore = "実鍵と課金が要る"]
    async fn live_scores_two_paragraphs() {
        let (Ok(account_id), Ok(api_token)) = (
            std::env::var("JEV_ACCOUNT_ID"),
            std::env::var("JEV_API_TOKEN"),
        ) else {
            panic!("JEV_ACCOUNT_ID / JEV_API_TOKEN が要る");
        };
        let scorer = JevScorer::new(JevConfig {
            account_id,
            api_token,
            base_url: DEFAULT_BASE_URL.into(),
            model: DEFAULT_MODEL.into(),
            threshold: 0.2,
        })
        .expect("client");

        let basis = "Rust の `Pin` が何を保証するのか、動かせない理由まで含めて説明してほしい。";
        let paragraphs = [
            "`Pin<P>` は、指している値がメモリ上で動かされないことを型で表す。\
             自己参照を持つ future は、動かされると内部のポインタが宙に浮く。",
            "このサイトはクッキーを使用しています。設定を変更するにはこちらをクリックしてください。\
             プライバシーポリシー | 利用規約 | お問い合わせ",
        ];
        let got = scorer
            .score(basis, &paragraphs)
            .await
            .expect("score");

        println!("{got:?}");
        assert_eq!(got.scores.len(), 2);
        assert_eq!(got.calls, 1, "2 段落なら 1 呼び出しに収まる");
        assert!(got.tokens > 0, "入力トークンが返る");
        let (core, junk) = (got.scores[0].expect("核"), got.scores[1].expect("定型文"));
        assert!(core > junk, "依頼の核 {core} が定型文 {junk} より高い");
    }
}
