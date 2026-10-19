//! モデルテンプレートの接続テスト（Spec 68）— 呼び出しと計器。
//!
//! 純機構（定型文・判定・結果の型・ログ行）は [`crate::template_probe`]。ここは保存済みのテンプレートを読み、
//! 固有スキルを外した複製でバックエンドを組み、**再試行の輪を通らない 1 往復**（`chat_once`）を撃って、
//! `template probe:` の 1 行を書く。
//!
//! - **押したときだけ外へ出る**（契約 11）— 呼び出し元は IPC `test_model_template` だけで、起動・画面遷移・
//!   タイマーから呼ばない
//! - **複製をキャッシュ（`backend_for`）へ入れない**（契約 8）— id が同じなので、入れると村の個体がスキル無しの
//!   バックエンドを掴む。`factory.create` を直接呼ぶ（Spec 63 の `assist_draft` と同じ）
//! - **払いは村の予算・個体の累計・統計（`Record`）に入れない**（契約 10）。見える場所はログの 1 行と画面

use std::time::Instant;

use crate::error::CoreResult;
use crate::llm::{ChatMessage, ChatRequest, ToolChoice};
use crate::model::ModelTemplateId;
use crate::note;
use crate::template_probe::{
    ProbeFacts, TemplateProbe, degraded_failure, factory_failure, judge, log_line, probe_prompt,
};

use super::Orchestrator;

impl Orchestrator {
    /// 保存済みのテンプレートで LLM を 1 回だけ呼び、繋がったかどうかを返す（Spec 68）。
    ///
    /// **LLM の失敗は `Err` ではなく `TemplateProbe::Failed` で返す**（契約 6 — `CoreError` だと画面に
    /// `LLM_API` しか届かず、表示用の分類が落ちる）。
    ///
    /// # Errors
    /// テンプレートが保存されていない（確認を始められない）ときだけ
    /// [`crate::CoreError::ModelTemplateNotFound`] を返す。
    pub async fn probe_template(&self, id: &ModelTemplateId) -> CoreResult<TemplateProbe> {
        let template = {
            let world = self.shared.world.read().await;
            world.template(id)?.clone()
        };
        let language = self.language().await;
        // 固有スキル（検索・pro モードなど）は外す（契約 3）。思考段階・最大出力・温度はテンプレートのまま —
        // それらが拒否される形を捕まえるのが確認の目的。
        let template = template.without_provider_skills();
        let wire = format!("{:?}", template.effective_provider());

        let started = Instant::now();
        let elapsed = || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let (probe, facts) = match self.shared.factory.create(&template) {
            Err(err) => (factory_failure(err, elapsed()), ProbeFacts::default()),
            Ok(resolution) => match resolution.degraded_reason {
                // GUI のファクトリは設定不備をエコー応答へ退避させる。**ここで止めないと、鍵の無い
                // テンプレートの確認がエコー応答で「成功」する**（契約 8）。
                Some(reason) => (degraded_failure(reason, elapsed()), ProbeFacts::default()),
                None => {
                    let request = ChatRequest {
                        model: template.model.clone(),
                        messages: vec![ChatMessage::user(probe_prompt(language))],
                        tools: Vec::new(),
                        tool_choice: ToolChoice::None,
                        temperature: template.temperature,
                        max_tokens: template.max_output_tokens,
                        effort: template.effort,
                        cacheable_prefix_len: 0,
                    };
                    let result = resolution.backend.chat_once(request).await;
                    let facts = ProbeFacts::of(&result);
                    (judge(result, elapsed()), facts)
                }
            },
        };

        note!(
            "{}",
            log_line(template.id.as_str(), &wire, &template.model, &probe, &facts)
        );
        Ok(probe)
    }
}
