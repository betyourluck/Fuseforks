//! AI による下書き補助（Spec 63）— 呼び出し・判断役の検証の輪・記録。
//!
//! 純機構（指針・文脈の組み立て・振り分け・履歴の検査）は [`crate::assist`]。ここは
//! 文脈を村から集め、生成役を 1〜3 回呼び、`assist:` 行と `Record::Assist` を書く。
//!
//! **書き込みの経路は 1 本も増えない**（凍結 1）— 下書きは返すだけで、保存は既存の
//! `write_config` / `save_judge_file` を人が押す。

use crate::assist::{
    AssistContext, AssistKind, AssistOutcome, AssistReply, AssistRequest, AssistValidation,
    Classified, MAX_JUDGE_ATTEMPTS, SUBMIT_DRAFT, check_history, classify, compose_system,
    current_block, draft_rejected, draft_shown, extra_call_ignored, force_supported,
    force_utterance, submit_draft_spec,
};
use crate::error::{CoreError, CoreResult};
use crate::llm::{ChatMessage, ChatRequest, LlmError, ToolCall, ToolChoice, Usage};
use crate::model::{ConfigFileKind, ModelTemplate};
use crate::note;
use crate::session_store::{AssistRecord, Record as SessionRecord};
use crate::world::Language;

use super::Orchestrator;
use super::judging::check_judge_text;
use super::turn::is_bundled_tool_presented;

/// 1 回の呼び出しの記録に要る値（`assist:` 行と `Record::Assist` を同じ値から書く）。
struct CallNote<'a> {
    kind: AssistKind,
    template: &'a ModelTemplate,
    attempt: u32,
    forced: &'static str,
    outcome: AssistOutcome,
    usage: Option<Usage>,
    current_chars: usize,
    draft_chars: usize,
    started_ms: u64,
}

/// 輪の途中で持つ、直前の不正な下書き（失敗・空のときに返す）。
struct LastDraft {
    text: Option<String>,
    content: String,
    notes: Option<String>,
    location: String,
    message: String,
}

impl Orchestrator {
    /// AI 下書き補助の 1 往復（Spec 63 D4）。**状態を持たない** — 会話の全体を受け取り、
    /// 今回の利用者の発話とコアが作ったメッセージを `appended` として返す。
    ///
    /// # Errors
    /// - [`CoreError::InvalidAssistRequest`]: 発話が空 / 履歴の形が崩れている /
    ///   テンプレートがツールを使わない / 生成役を組めない
    /// - [`CoreError::ModelTemplateNotFound`] / [`CoreError::AgentNotFound`]
    /// - [`CoreError::Llm`]: 呼び出しが失敗し、それまでに下書きが 1 つも無い
    pub async fn assist_draft(&self, req: AssistRequest) -> CoreResult<AssistReply> {
        let language = self.language().await;
        let kind = req.target.kind;

        // 1. 送る前に拒否するもの（凍結 4 — 形だけを検査する）。
        check_history(&req.history).map_err(|reason| CoreError::InvalidAssistRequest { reason })?;
        let input = req.input.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let utterance = match (input, req.force_draft) {
            (Some(text), true) => format!("{text}\n\n{}", force_utterance(language)),
            (None, true) => force_utterance(language).to_owned(),
            (Some(text), false) => text.to_owned(),
            (None, false) => {
                return Err(CoreError::InvalidAssistRequest { reason: "発話が空です".to_owned() });
            }
        };

        // 2. 生成役（凍結 3）。**固有スキルを外した複製はキャッシュへ入れない** —
        //    id が同じなので、入れると村の個体がスキル無しのバックエンドを掴む。
        let template = {
            let world = self.shared.world.read().await;
            world.template(&req.template_id)?.clone()
        };
        if !template.use_tools {
            return Err(CoreError::InvalidAssistRequest {
                reason: format!("テンプレート `{}` はツールを使わない設定なので、下書きを受け取れません", template.name),
            });
        }
        let template = template.without_provider_skills();
        let resolution = self.shared.factory.create(&template)?;
        if let Some(reason) = resolution.degraded_reason {
            return Err(CoreError::InvalidAssistRequest { reason });
        }
        let backend = resolution.backend;
        let force = req.force_draft && force_supported(template.effective_provider());
        let forced = match (req.force_draft, force) {
            (false, _) => "no",
            (true, true) => "yes",
            (true, false) => "fallback",
        };

        // 3. 文脈（凍結 8）とメッセージ。安定部だけを `cacheable_prefix_len` に載せる。
        let context = self.assist_context(&req.target, language).await?;
        let stable = compose_system(kind, language, &context);
        let stable_len = stable.chars().count();
        let mut messages = vec![ChatMessage::system(stable)];
        if let Some(block) = current_block(kind, language, &req.current) {
            messages.push(ChatMessage::system(block));
        }
        messages.extend(req.history.iter().cloned());
        let user = ChatMessage::user(utterance);
        messages.push(user.clone());
        let mut appended = vec![user];

        let current_chars = req.current.chars().count();
        let max_attempts = if kind == AssistKind::Judge { MAX_JUDGE_ATTEMPTS } else { 1 };
        let mut last: Option<LastDraft> = None;

        // 4. 呼ぶ（判断役だけ検証の輪 — 凍結 7）。
        for attempt in 1..=max_attempts {
            let started_ms = crate::command::now_ms();
            let request = ChatRequest {
                model: template.model.clone(),
                messages: messages.clone(),
                tools: vec![submit_draft_spec(language)],
                tool_choice: if force { ToolChoice::Specific(SUBMIT_DRAFT.to_owned()) } else { ToolChoice::Auto },
                temperature: template.temperature,
                max_tokens: template.max_output_tokens,
                effort: template.effort,
                cacheable_prefix_len: stable_len,
            };
            let mut note = CallNote {
                kind,
                template: &template,
                attempt,
                forced,
                outcome: AssistOutcome::Failed,
                usage: None,
                current_chars,
                draft_chars: 0,
                started_ms,
            };

            let response = match backend.chat(request).await {
                Ok(response) => response,
                Err(err) => {
                    note.usage = err.usage().copied();
                    self.note_assist(&note);
                    return match last {
                        Some(draft) => Ok(invalid_reply(appended, draft, attempt - 1)),
                        None => Err(CoreError::Llm(err)),
                    };
                }
            };
            note.usage = Some(response.usage);

            let classified = classify(&response);
            let assistant = ChatMessage::assistant_tool_calls(
                response.text.clone().unwrap_or_default(),
                response.tool_calls.clone(),
            );
            messages.push(assistant.clone());
            appended.push(assistant);

            match classified {
                Classified::Question(text) => {
                    pair_calls(&response.tool_calls, None, language, &mut messages, &mut appended);
                    note.outcome = AssistOutcome::Question;
                    self.note_assist(&note);
                    return Ok(AssistReply::Question { appended, text });
                }
                Classified::Empty => {
                    pair_calls(&response.tool_calls, None, language, &mut messages, &mut appended);
                    note.outcome = AssistOutcome::Empty;
                    self.note_assist(&note);
                    return match last {
                        Some(draft) => Ok(invalid_reply(appended, draft, attempt - 1)),
                        None => Err(CoreError::Llm(LlmError::EmptyResponse)),
                    };
                }
                Classified::Draft { text, content, notes } => {
                    note.draft_chars = content.chars().count();
                    if kind != AssistKind::Judge {
                        pair_calls(&response.tool_calls, Some(draft_shown(language).to_owned()), language, &mut messages, &mut appended);
                        note.outcome = AssistOutcome::Draft;
                        self.note_assist(&note);
                        return Ok(draft_reply(appended, text, content, notes, None, attempt));
                    }
                    let checked = {
                        let world = self.shared.world.read().await;
                        check_judge_text(&world, &content)
                    };
                    match checked {
                        Ok(_) => {
                            pair_calls(&response.tool_calls, Some(draft_shown(language).to_owned()), language, &mut messages, &mut appended);
                            note.outcome = AssistOutcome::Draft;
                            self.note_assist(&note);
                            return Ok(draft_reply(appended, text, content, notes, Some(AssistValidation::Valid), attempt));
                        }
                        Err(CoreError::InvalidJudgeFile { location, message }) => {
                            let result = draft_rejected(language, &location, &message);
                            pair_calls(&response.tool_calls, Some(result), language, &mut messages, &mut appended);
                            let draft = LastDraft { text, content, notes, location, message };
                            if attempt >= max_attempts {
                                note.outcome = AssistOutcome::DraftInvalid;
                                self.note_assist(&note);
                                return Ok(invalid_reply(appended, draft, attempt));
                            }
                            note.outcome = AssistOutcome::Invalid;
                            self.note_assist(&note);
                            last = Some(draft);
                        }
                        Err(other) => return Err(other),
                    }
                }
            }
        }
        // 上限に達したときは輪の中で返している。
        unreachable!("the attempt loop always returns")
    }

    /// 生成役に渡す「村の事実」を集める（凍結 8）。**渡さないもの（Memory / 条例 /
    /// 会話ログ）は読みもしない。**
    async fn assist_context(
        &self,
        target: &crate::assist::AssistTarget,
        language: Language,
    ) -> CoreResult<AssistContext> {
        match target.kind {
            AssistKind::Skill | AssistKind::Construct => {
                let (name, spec, connected) = {
                    let world = self.shared.world.read().await;
                    let record = world.agent(&target.id)?;
                    let spec = record.spec.clone();
                    let connected = spec
                        .connected_agents
                        .iter()
                        .map(|id| match world.agent(id) {
                            Ok(other) => other.spec.name.clone(),
                            Err(_) => world.judge(id).map_or_else(|| id.to_string(), |j| j.name.clone()),
                        })
                        .collect::<Vec<_>>();
                    (spec.name.clone(), spec, connected)
                };
                let mut bundled_tools: Vec<String> = crate::tools::BUNDLED_TOOL_NAMES
                    .iter()
                    .filter(|name| is_bundled_tool_presented(name, &spec))
                    .map(|name| (*name).to_owned())
                    .collect();
                if spec.uses_blackboard {
                    bundled_tools.push("blackboard".to_owned());
                }
                if !spec.rag_sources.is_empty() {
                    bundled_tools.push("rag".to_owned());
                }
                let mcp = self.assist_mcp(&target.id).await;
                let pair_kind = match target.kind {
                    AssistKind::Skill => ConfigFileKind::Construct,
                    _ => ConfigFileKind::Skill,
                };
                let pair = self.shared.store.read_config(&target.id, pair_kind).await?;
                let pair = (!pair.trim().is_empty()).then_some(pair);
                let _ = language;
                Ok(AssistContext::Servant { name, bundled_tools, mcp, connected, pair })
            }
            AssistKind::Judge => {
                let world = self.shared.world.read().await;
                let judge = world
                    .judge(&target.id)
                    .ok_or_else(|| CoreError::AgentNotFound(target.id.to_string()))?;
                let mut servants: Vec<_> = world
                    .snapshots()
                    .into_iter()
                    .map(|snapshot| {
                        let role = snapshot
                            .role_id
                            .as_ref()
                            .and_then(|id| world.role(id).ok())
                            .map(|role| role.name.clone());
                        (snapshot.id.clone(), snapshot.name.clone(), role)
                    })
                    .collect();
                servants.sort_by(|a, b| a.0.cmp(&b.0));
                Ok(AssistContext::Judge { name: judge.name.clone(), servants })
            }
        }
    }

    /// 対象の個体が使える MCP。**接続中はツール名まで、未接続は名前だけ**（凍結 8）。
    ///
    /// 共通の接続（村の `mcp.json`）と個体の接続（`agents/<id>/mcp.json`。稼働中だけ）を合わせる。
    async fn assist_mcp(&self, id: &crate::model::AgentId) -> Vec<(String, Option<Vec<String>>)> {
        let mut out: Vec<(String, Option<Vec<String>>)> = Vec::new();
        let mut push = |status: &crate::mcp::McpServerStatus| {
            let tools = status.connected.then(|| status.tools.clone());
            out.push((status.name.clone(), tools));
        };
        for status in self.shared.mcp.read().await.statuses() {
            push(status);
        }
        if let Some(state) = self.shared.agent_mcp.read().await.get(id) {
            for status in state.manager.statuses() {
                push(status);
            }
        }
        out
    }

    /// `assist:` 行と `Record::Assist` を**同じ値から**書く（凍結 10・11）。本文は出さない。
    fn note_assist(&self, note: &CallNote<'_>) {
        let (prompt, cached, total, reasoning) = note.usage.map_or_else(
            || ("-".to_owned(), "-".to_owned(), "-".to_owned(), "-".to_owned()),
            |u| {
                (
                    u.prompt.to_string(),
                    u.cache_read.to_string(),
                    u.total().to_string(),
                    u.reasoning.to_string(),
                )
            },
        );
        note!(
            "assist: kind={} model={} attempt={} forced={} outcome={} prompt={prompt} cached={cached} \
             total={total} reasoning={reasoning} current_chars={} draft_chars={}",
            note.kind.label(),
            note.template.model,
            note.attempt,
            note.forced,
            note.outcome.label(),
            note.current_chars,
            note.draft_chars,
        );
        // 使用量が分からない失敗（HTTP の失敗など）は数字を捏造しない — 行だけ。
        let Some(usage) = note.usage else {
            return;
        };
        let record = AssistRecord {
            ts_ms: note.started_ms,
            draft_kind: note.kind,
            model: note.template.model.clone(),
            template_id: note.template.id.as_str().to_owned(),
            prompt: usage.prompt,
            cached: usage.cache_read,
            cache_write: usage.cache_write,
            cache_write_1h: usage.cache_write_1h,
            completion: usage.completion,
            reasoning: usage.reasoning,
            outcome: note.outcome,
        };
        if !self.shared.persist(&SessionRecord::assist(record)) {
            note!("WARN assist: 会話の保存先へ書けなかったため、この使用量は統計画面に出ません（この行だけが記録です）");
        }
    }
}

/// 応答の呼び出しすべてにツール結果を対で付ける（凍結 5）。最初の `submit_draft` には
/// `first` を、それ以外（2 本目以降・下書きにならなかった呼び出し）には定型文を返す。
fn pair_calls(
    calls: &[ToolCall],
    first: Option<String>,
    language: Language,
    messages: &mut Vec<ChatMessage>,
    appended: &mut Vec<ChatMessage>,
) {
    let mut first = first;
    for call in calls {
        let content = match (call.name == SUBMIT_DRAFT, first.take()) {
            (true, Some(text)) => text,
            (_, taken) => {
                first = taken;
                extra_call_ignored(language).to_owned()
            }
        };
        let result = ChatMessage::tool_result(call.id.clone(), call.name.clone(), content);
        messages.push(result.clone());
        appended.push(result);
    }
}

fn draft_reply(
    appended: Vec<ChatMessage>,
    text: Option<String>,
    content: String,
    notes: Option<String>,
    validation: Option<AssistValidation>,
    attempts: u32,
) -> AssistReply {
    let draft_chars = u32::try_from(content.chars().count()).unwrap_or(u32::MAX);
    AssistReply::Draft { appended, text, content, notes, draft_chars, validation, attempts }
}

fn invalid_reply(appended: Vec<ChatMessage>, draft: LastDraft, attempts: u32) -> AssistReply {
    draft_reply(
        appended,
        draft.text,
        draft.content,
        draft.notes,
        Some(AssistValidation::Invalid { location: draft.location, message: draft.message }),
        attempts,
    )
}
