//! モデルテンプレートの接続テスト（Spec 68）— 純機構。
//!
//! 保存済みのテンプレートで LLM を 1 回だけ呼んだ結果を、画面が辞書の鍵を選べる型へ振り分ける。
//! 呼び出しと計器は `orchestrator/template_probe.rs`。契約は `data_contract.yaml` の
//! `template_probe_contract`。
//!
//! - **成功の基準は「接続先が 2xx を返し、選んだプロトコルの応答として解釈できた」**（D2）。本文が空でも、
//!   出力上限で切れても成功に数える（接続先は鍵・モデル名・パラメータを受け付けている）
//! - **失敗の分類は表示用の閉じた列挙 [`ProbeFailure`]**（D6）。再試行の判定のための `RetryClass` は
//!   1 値も変えない。写しは [`judge`] の網羅 match の 1 本で、`_ =>` を書かない — `LlmError` か `RetryClass`
//!   に値が足されたら、ここがコンパイルで落ちる
//! - **コアは訳した文字列を入れない**（D7）。画面が `outcome` / `failure` / `finish` から辞書の鍵を選ぶ

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, ErrorPayload};
use crate::llm::retry::RetryClass;
use crate::llm::{ChatResponse, Finish, LlmError};
use crate::world::Language;

/// 応答本文を画面へ運ぶ長さ（コードポイント）。
pub const PROBE_TEXT_CHARS: usize = 200;

/// 200 だが形が違ったとき、受け取った本文を運ぶ長さ（コードポイント）。
pub const PROBE_RAW_CHARS: usize = 2_000;

/// 接続テストで送る唯一の利用者発話（D3）。村の言語の定型文で、利用者のデータを 1 字も含まない。
pub fn probe_prompt(language: Language) -> &'static str {
    match language {
        Language::Ja => "接続の確認です。『OK』とだけ返してください。",
        Language::En => "This is a connection check. Reply with just \"OK\".",
    }
}

/// 失敗の表示用の分類（D6。10 値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeFailure {
    /// テンプレートからバックエンドを組めなかった（ファクトリの失敗 / 退避 / `Config`）。
    Config,
    /// 認証で拒否された（401 / 403）。
    Auth,
    /// モデル名か base URL のパスが接続先に無い（404）。
    NotFound,
    /// 接続先が要求の形を拒否した（404 以外の 4xx / 文脈長）。
    Rejected,
    /// レート制限。
    RateLimit,
    /// 残高かクォータの不足（`insufficient_quota`）。
    Billing,
    /// 接続先の一時的な障害（5xx / 過負荷）。
    ServerError,
    /// 時間内に応答が無かった。
    Timeout,
    /// base URL に届かなかった。
    Network,
    /// 2xx だが、選んだプロトコルの応答の形ではなかった。
    UnexpectedShape,
}

impl ProbeFailure {
    /// 計器 `template probe:` の `outcome=` に書く名前（serde の名前と同じ）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Auth => "auth",
            Self::NotFound => "not_found",
            Self::Rejected => "rejected",
            Self::RateLimit => "rate_limit",
            Self::Billing => "billing",
            Self::ServerError => "server_error",
            Self::Timeout => "timeout",
            Self::Network => "network",
            Self::UnexpectedShape => "unexpected_shape",
        }
    }
}

/// 本文が空だった応答の終了理由（canonical の [`Finish`] の写し）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeFinish {
    /// 正常終了（本文が空のまま終わった）。
    Stop,
    /// 出力上限（`OutputTruncated` にならなかった形）。
    Length,
    /// ツール呼び出しで終わった（ツールは送っていない）。
    ToolUse,
    /// 判別不能（Gemini の安全フィルタ `SAFETY` もここに畳まれる）。
    Other,
}

impl ProbeFinish {
    fn of(finish: &Finish) -> Self {
        match finish {
            Finish::Stop => Self::Stop,
            Finish::Length => Self::Length,
            Finish::ToolUse => Self::ToolUse,
            // `pause_turn` はサーバー側ツールのループが止まった印（Spec 71）。接続テストは
            // `without_provider_skills` でサーバー側ツールを外して撃つので来ないはずの形で、
            // 来ても値を増やさず「判別不能」に畳む（`ProbeFinish` は閉じた表示の語彙）。
            Finish::Paused | Finish::Other => Self::Other,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Length => "length",
            Self::ToolUse => "tool_use",
            Self::Other => "other",
        }
    }
}

/// 1 回の確認で払ったトークン（画面のその場の表示とログの 1 行に出す）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeUsage {
    /// 入力トークン（キャッシュ分を含む合計）。
    pub prompt: u64,
    /// 出力トークン（思考を含む）。
    pub completion: u64,
    /// うち思考トークン。
    pub reasoning: u64,
}

impl ProbeUsage {
    fn of(usage: &crate::llm::Usage) -> Self {
        Self {
            prompt: usage.prompt,
            completion: usage.completion,
            reasoning: usage.reasoning,
        }
    }
}

/// 接続先が応答したときの形（`outcome` で判別。欄は outcome ごとに決まる）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Reached {
    /// 本文が返った。`text` は先頭 [`PROBE_TEXT_CHARS`] 字。
    Replied {
        /// 応答本文の先頭（trim 後）。
        text: String,
        /// 払ったトークン。
        usage: ProbeUsage,
    },
    /// 出力上限で切れた（**成功に数える** — 接続先は要求を受け付けている）。`limit` は適用されていた上限。
    Truncated {
        /// 適用されていた最大出力トークン。
        limit: u32,
        /// 払ったトークン。
        usage: ProbeUsage,
    },
    /// 本文が無かった。`finish` と `usage` が `None` なのは実経路では到達しない腕
    /// （`EmptyResponse` / `Blocked`）だけ。
    NoText {
        /// 終了理由。
        finish: Option<ProbeFinish>,
        /// 払ったトークン。
        usage: Option<ProbeUsage>,
    },
}

/// 接続テストの結果（`result` で判別。IPC `test_model_template` の返り値）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum TemplateProbe {
    /// 接続先が応答した（成功）。
    #[serde(rename_all = "camelCase")]
    Reached {
        /// 押してから返るまで。
        elapsed_ms: u64,
        /// 応答の形。
        #[serde(flatten)]
        reached: Reached,
    },
    /// 繋がらなかった（失敗）。
    #[serde(rename_all = "camelCase")]
    Failed {
        /// 表示用の分類。
        failure: ProbeFailure,
        /// `Api`（と画像の拒否）のときだけ。
        status: Option<u16>,
        /// `Api` で接続先が待ち時間を明示したときだけ（60 秒を超えても値のまま）。
        retry_after_secs: Option<u64>,
        /// 原文。画面は `formatError` に通す。作り方は経路ごとに 1 つ（契約 7）。
        error: ErrorPayload,
        /// 押してから返るまで。
        elapsed_ms: u64,
    },
}

/// 先頭から `max` コードポイントで切る（文字の途中で切らない）。
fn head_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// 1 回の往復の結果を振り分ける（D2・D6）。`LlmError` と `RetryClass` の両方を網羅 match で書く。
pub fn judge(result: Result<ChatResponse, LlmError>, elapsed_ms: u64) -> TemplateProbe {
    let err = match result {
        Ok(response) => {
            let usage = ProbeUsage::of(&response.usage);
            let text = response.text.as_deref().map(str::trim).unwrap_or("");
            let reached = if text.is_empty() {
                Reached::NoText {
                    finish: Some(ProbeFinish::of(&response.finish)),
                    usage: Some(usage),
                }
            } else {
                Reached::Replied {
                    text: head_chars(text, PROBE_TEXT_CHARS),
                    usage,
                }
            };
            return TemplateProbe::Reached { elapsed_ms, reached };
        }
        Err(err) => err,
    };

    let reached = |reached| TemplateProbe::Reached { elapsed_ms, reached };
    let failed = |failure, status, retry_after_secs, error| TemplateProbe::Failed {
        failure,
        status,
        retry_after_secs,
        error,
        elapsed_ms,
    };
    match err {
        LlmError::OutputTruncated { limit, usage } => reached(Reached::Truncated {
            limit,
            usage: ProbeUsage::of(&usage),
        }),
        // 実経路では作られない腕（Spec 68 現況）。網羅のために持つ。
        LlmError::EmptyResponse | LlmError::Blocked { .. } => reached(Reached::NoText {
            finish: None,
            usage: None,
        }),
        LlmError::Api {
            status,
            class,
            retry_after,
            ..
        } => {
            let failure = api_failure(status, class);
            let retry_after_secs = retry_after.map(|wait| wait.as_secs());
            failed(failure, Some(status), retry_after_secs, payload(err))
        }
        // 接続テストは添付を送らないので到達しない。形は 400 の拒否と同じ。
        LlmError::ImagesRejected { .. } => failed(ProbeFailure::Rejected, Some(400), None, payload(err)),
        LlmError::Config(_) => failed(ProbeFailure::Config, None, None, payload(err)),
        LlmError::Http { ref source, .. } => {
            let failure = if source.is_timeout() {
                ProbeFailure::Timeout
            } else {
                ProbeFailure::Network
            };
            failed(failure, None, None, payload(err))
        }
        // 受け取った本文の先頭を運ぶ。Display の「構造化出力のパースに失敗しました」は使わない
        // （構造化出力を使わない確認で出ると誤った方向を指す）。間違った base URL が返した HTML などが
        // いちばん効く手掛かり（契約 7）。
        LlmError::Parse { source, raw } => failed(
            ProbeFailure::UnexpectedShape,
            None,
            None,
            ErrorPayload {
                code: "LLM_PARSE".to_owned(),
                message: head_chars(&raw, PROBE_RAW_CHARS),
                detail: Some(source.to_string()),
                agent_id: None,
                retryable: false,
                retry_after_secs: None,
                images_rejected: false,
            },
        ),
        LlmError::NoStructuredOutput => failed(ProbeFailure::UnexpectedShape, None, None, payload(err)),
    }
}

/// `Api` の失敗を status と class だけで写す（D6。P0 実測でプロバイダの code で補う腕は持たないと決めた）。
fn api_failure(status: u16, class: RetryClass) -> ProbeFailure {
    match class {
        RetryClass::Auth => ProbeFailure::Auth,
        RetryClass::ClientError if status == 404 => ProbeFailure::NotFound,
        RetryClass::ClientError | RetryClass::ContextLength => ProbeFailure::Rejected,
        RetryClass::RateLimit => ProbeFailure::RateLimit,
        RetryClass::Billing => ProbeFailure::Billing,
        RetryClass::ServerError | RetryClass::Overloaded => ProbeFailure::ServerError,
        // `Api.class` は `retry::classify` だけが決め、この 2 値を返さない（型の網羅のための腕）。
        RetryClass::Timeout => ProbeFailure::Timeout,
        RetryClass::Network => ProbeFailure::Network,
    }
}

fn payload(err: LlmError) -> ErrorPayload {
    ErrorPayload::from(&CoreError::Llm(err))
}

/// ファクトリがバックエンドを組めなかった（契約 7: 既存の表示と同じ原文）。
pub fn factory_failure(err: LlmError, elapsed_ms: u64) -> TemplateProbe {
    TemplateProbe::Failed {
        failure: ProbeFailure::Config,
        status: None,
        retry_after_secs: None,
        error: payload(err),
        elapsed_ms,
    }
}

/// ファクトリが設定不備をエコー応答へ退避させた（契約 8: **成功にしない**）。
///
/// `degraded_reason` は完成した表示文（`LlmError` の Display の接頭辞を既に含む）なので、
/// `LlmError::Config` に包み直さない — 包むと「LLM 設定エラー: 」が二重になる（契約 7）。
pub fn degraded_failure(reason: String, elapsed_ms: u64) -> TemplateProbe {
    TemplateProbe::Failed {
        failure: ProbeFailure::Config,
        status: None,
        retry_after_secs: None,
        error: ErrorPayload {
            code: "LLM_CONFIG".to_owned(),
            message: reason,
            detail: None,
            agent_id: None,
            retryable: false,
            retry_after_secs: None,
            images_rejected: false,
        },
        elapsed_ms,
    }
}

/// 計器の `class=` / `code=` の材料（判定が `LlmError` を消費する前に読む）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProbeFacts {
    /// `RetryClass` の名前（`Api` のときだけ）。
    pub class: Option<&'static str>,
    /// プロバイダの機械可読コードの先頭 40 字（`Api` のときだけ）。
    pub code: Option<String>,
}

impl ProbeFacts {
    /// 1 往復の結果から読む。
    pub fn of(result: &Result<ChatResponse, LlmError>) -> Self {
        match result {
            Err(LlmError::Api { class, code, .. }) => Self {
                class: Some(class.as_str()),
                code: code.as_deref().map(|c| head_chars(c, 40)),
            },
            _ => Self::default(),
        }
    }
}

/// 計器 `template probe:` の 1 行（D4。契約 9）。
///
/// **書かないもの = 送った文・応答の本文・接続先のエラー本文・受け取った本文・API キー・base URL**
/// （`failures.md` #71。P0 実測: OpenAI は不正な鍵の 401 本文に鍵の末尾 4 文字を返す）。
/// 使用量の欄の `-` は使用量を持たない結果だけ（0 を捏造しない — #72）。
pub fn log_line(template: &str, wire: &str, model: &str, probe: &TemplateProbe, facts: &ProbeFacts) -> String {
    let dash = || "-".to_owned();
    let (outcome, finish, status, elapsed_ms, usage) = match probe {
        TemplateProbe::Reached { elapsed_ms, reached } => match reached {
            Reached::Replied { usage, .. } => ("replied", None, None, *elapsed_ms, Some(*usage)),
            Reached::Truncated { usage, .. } => ("truncated", None, None, *elapsed_ms, Some(*usage)),
            Reached::NoText { finish, usage } => ("no_text", *finish, None, *elapsed_ms, *usage),
        },
        TemplateProbe::Failed {
            failure,
            status,
            elapsed_ms,
            ..
        } => (failure.as_str(), None, *status, *elapsed_ms, None),
    };
    let num = |n: Option<u64>| n.map_or_else(dash, |n| n.to_string());
    format!(
        "template probe: template={template} wire={wire} model={model} outcome={outcome} finish={} status={} \
         class={} code={} elapsed_ms={elapsed_ms} prompt={} completion={} reasoning={}",
        finish.map_or_else(dash, |f| f.as_str().to_owned()),
        status.map_or_else(dash, |s| s.to_string()),
        facts.class.unwrap_or("-"),
        facts.code.clone().unwrap_or_else(dash),
        num(usage.map(|u| u.prompt)),
        num(usage.map(|u| u.completion)),
        num(usage.map(|u| u.reasoning)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::retry::HintSource;
    use crate::llm::{Grounding, Usage};
    use std::time::Duration;

    fn usage() -> Usage {
        Usage { prompt: 20, completion: 5, cache_read: 0, cache_write: 0, cache_write_1h: 0, reasoning: 3 }
    }

    fn response(text: Option<&str>, finish: Finish) -> ChatResponse {
        ChatResponse {
            text: text.map(str::to_owned),
            tool_calls: Vec::new(),
            finish,
            usage: usage(),
            grounding: Grounding::default(),
            reasoning_summary: Vec::new(),
            replay: None,
        }
    }

    fn api(status: u16, class: RetryClass) -> LlmError {
        LlmError::Api {
            status,
            body: "boom".into(),
            class,
            code: None,
            retry_after: None,
            hint_src: HintSource::None,
        }
    }

    fn failure_of(probe: &TemplateProbe) -> ProbeFailure {
        match probe {
            TemplateProbe::Failed { failure, .. } => *failure,
            other => panic!("failed のはず: {other:?}"),
        }
    }

    // ---- 成功の腕（D2） ----

    #[test]
    fn a_reply_with_text_is_replied_with_its_head_and_usage() {
        let long = "あ".repeat(PROBE_TEXT_CHARS + 50);
        let probe = judge(Ok(response(Some(&format!("  {long}  ")), Finish::Stop)), 12);
        let TemplateProbe::Reached { elapsed_ms, reached: Reached::Replied { text, usage } } = probe else {
            panic!("replied のはず");
        };
        assert_eq!(elapsed_ms, 12);
        assert_eq!(text.chars().count(), PROBE_TEXT_CHARS, "先頭 200 字（コードポイント）");
        assert_eq!(usage, ProbeUsage { prompt: 20, completion: 5, reasoning: 3 });
    }

    #[test]
    fn an_empty_reply_is_no_text_with_finish_and_usage() {
        for (text, finish, expected) in [
            (None, Finish::Stop, ProbeFinish::Stop),
            (Some("   "), Finish::Other, ProbeFinish::Other),
        ] {
            let probe = judge(Ok(response(text, finish)), 1);
            assert_eq!(
                probe,
                TemplateProbe::Reached {
                    elapsed_ms: 1,
                    reached: Reached::NoText {
                        finish: Some(expected),
                        usage: Some(ProbeUsage { prompt: 20, completion: 5, reasoning: 3 }),
                    },
                },
                "200 の空の本文は使用量を持つ（rev2 点 3）"
            );
        }
    }

    #[test]
    fn output_truncation_is_a_success_with_the_limit() {
        let probe = judge(Err(LlmError::OutputTruncated { limit: 64, usage: usage() }), 1);
        assert!(matches!(
            probe,
            TemplateProbe::Reached { reached: Reached::Truncated { limit: 64, .. }, .. }
        ));
    }

    #[test]
    fn the_unreachable_empty_arms_are_no_text_without_usage() {
        for err in [LlmError::EmptyResponse, LlmError::Blocked { reason: "SAFETY".into() }] {
            let probe = judge(Err(err), 1);
            assert!(matches!(
                probe,
                TemplateProbe::Reached { reached: Reached::NoText { finish: None, usage: None }, .. }
            ));
        }
    }

    // ---- Api の腕（D6。到達しうる 7 分類 + 404 の分け = 8 行で、写し先は 6 値） ----

    #[test]
    fn api_failures_map_by_status_and_class() {
        let rows = [
            (401, RetryClass::Auth, ProbeFailure::Auth),
            (404, RetryClass::ClientError, ProbeFailure::NotFound),
            (400, RetryClass::ClientError, ProbeFailure::Rejected),
            (400, RetryClass::ContextLength, ProbeFailure::Rejected),
            (429, RetryClass::RateLimit, ProbeFailure::RateLimit),
            (429, RetryClass::Billing, ProbeFailure::Billing),
            (500, RetryClass::ServerError, ProbeFailure::ServerError),
            (529, RetryClass::Overloaded, ProbeFailure::ServerError),
        ];
        for (status, class, expected) in rows {
            let probe = judge(Err(api(status, class)), 1);
            assert_eq!(failure_of(&probe), expected, "{status} {class:?}");
            let TemplateProbe::Failed { status: got, error, .. } = probe else { unreachable!() };
            assert_eq!(got, Some(status));
            assert_eq!(error.code, "LLM_API");
            assert!(error.message.contains("boom"), "接続先の本文を原文で運ぶ");
        }
    }

    #[test]
    fn an_explicit_wait_is_carried_as_is_even_above_sixty_seconds() {
        let mut err = api(429, RetryClass::RateLimit);
        if let LlmError::Api { retry_after, .. } = &mut err {
            *retry_after = Some(Duration::from_secs(3600));
        }
        let TemplateProbe::Failed { retry_after_secs, .. } = judge(Err(err), 1) else { panic!() };
        assert_eq!(retry_after_secs, Some(3600));
    }

    // ---- Api 以外の失敗の腕 ----

    #[test]
    fn config_parse_and_structured_output_failures() {
        assert_eq!(failure_of(&judge(Err(LlmError::Config("鍵が無い".into())), 1)), ProbeFailure::Config);
        assert_eq!(failure_of(&judge(Err(LlmError::NoStructuredOutput), 1)), ProbeFailure::UnexpectedShape);
        assert_eq!(
            failure_of(&judge(Err(LlmError::ImagesRejected { body: "x".into() }), 1)),
            ProbeFailure::Rejected
        );
    }

    #[test]
    fn a_parse_failure_carries_the_received_body_not_the_structured_output_wording() {
        let source = serde_json::from_str::<serde_json::Value>("<html").unwrap_err();
        // 多バイト文字で 2,000 字を超える本文 — 文字の途中で切っても panic しない。
        let raw = format!("<html>{}", "日".repeat(PROBE_RAW_CHARS));
        let probe = judge(Err(LlmError::Parse { source, raw }), 1);
        assert_eq!(failure_of(&probe), ProbeFailure::UnexpectedShape);
        let TemplateProbe::Failed { error, .. } = probe else { unreachable!() };
        assert_eq!(error.code, "LLM_PARSE");
        assert!(error.message.starts_with("<html>"), "受け取った本文の先頭: {}", error.message);
        assert_eq!(error.message.chars().count(), PROBE_RAW_CHARS);
        assert!(!error.message.contains("構造化出力"), "構造化出力の語を出さない");
        assert!(error.detail.is_some(), "serde の理由は detail");
    }

    #[test]
    fn a_degraded_backend_is_config_with_the_reason_once() {
        let reason = "LLM 設定エラー: モデルテンプレート `x` の API キーが未登録です".to_owned();
        let probe = degraded_failure(reason, 1);
        assert_eq!(failure_of(&probe), ProbeFailure::Config);
        let TemplateProbe::Failed { error, .. } = probe else { unreachable!() };
        assert_eq!(error.code, "LLM_CONFIG");
        assert_eq!(error.message.matches("LLM 設定エラー: ").count(), 1, "包み直して二重にしない");
    }

    #[test]
    fn a_factory_error_is_config() {
        let probe = factory_failure(LlmError::Config("鍵が無い".into()), 1);
        assert_eq!(failure_of(&probe), ProbeFailure::Config);
    }

    // ---- 計器（D4） ----

    #[test]
    fn the_log_line_carries_counts_but_no_body() {
        let replied = judge(Ok(response(Some("OK 秘密の本文"), Finish::Stop)), 42);
        let line = log_line("tpl", "OpenAiCompat", "m", &replied, &ProbeFacts::default());
        assert_eq!(
            line,
            "template probe: template=tpl wire=OpenAiCompat model=m outcome=replied finish=- status=- \
             class=- code=- elapsed_ms=42 prompt=20 completion=5 reasoning=3"
        );
        assert!(!line.contains("秘密"), "応答の本文を書かない");

        let result: Result<ChatResponse, LlmError> = Err(LlmError::Api {
            status: 401,
            body: "Incorrect API key provided: sk-****abcd".into(),
            class: RetryClass::Auth,
            code: Some("invalid_api_key".into()),
            retry_after: None,
            hint_src: HintSource::None,
        });
        let facts = ProbeFacts::of(&result);
        let failed = judge(result, 7);
        let line = log_line("tpl", "OpenAiResponses", "m", &failed, &facts);
        assert_eq!(
            line,
            "template probe: template=tpl wire=OpenAiResponses model=m outcome=auth finish=- status=401 \
             class=auth code=invalid_api_key elapsed_ms=7 prompt=- completion=- reasoning=-"
        );
        assert!(!line.contains("abcd") && !line.contains("Incorrect"), "接続先の本文（鍵の末尾）を書かない");
    }

    #[test]
    fn the_probe_serializes_as_the_frozen_wire_shape() {
        let reached = judge(Ok(response(Some("OK"), Finish::Stop)), 5);
        assert_eq!(
            serde_json::to_value(&reached).unwrap(),
            serde_json::json!({
                "result": "reached", "elapsedMs": 5, "outcome": "replied", "text": "OK",
                "usage": { "prompt": 20, "completion": 5, "reasoning": 3 }
            })
        );
        let failed = judge(Err(api(404, RetryClass::ClientError)), 6);
        let json = serde_json::to_value(&failed).unwrap();
        assert_eq!(json["result"], "failed");
        assert_eq!(json["failure"], "not_found");
        assert_eq!(json["status"], 404);
        assert!(json["retryAfterSecs"].is_null());
        assert_eq!(json["elapsedMs"], 6);
        assert_eq!(json["error"]["code"], "LLM_API");
    }

    #[test]
    fn the_prompt_is_in_the_village_language() {
        assert!(probe_prompt(Language::Ja).contains("接続の確認"));
        assert!(probe_prompt(Language::En).starts_with("This is a connection check"));
    }
}
