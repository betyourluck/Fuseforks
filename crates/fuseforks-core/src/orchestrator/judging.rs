//! 判断役の実行時（Spec 62・`judge_contract`）— ツールの合成・評価・中継・計器・有効の述語・
//! 村への登録とファイルの読み書き。
//!
//! 判断役は**受信箱もターンも持たない関数**で、呼び出し元（サーヴァント）のツール呼び出しの中で
//! 同期的に評価される。中継は `deliver_and_wait(from = 呼び出し元)` なので答えは構造上呼び出し元へ
//! 戻り、待ちの輪の検出（Spec 44）・hop・予算・打ち切りは委譲と同じものが効く。

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use super::delegation::{bundle_answers, fanout_and_wait};
use super::{Orchestrator, Participants, Shared, deliver_and_wait, note_dropped_attachment};
use crate::budget::BudgetPool;
use crate::error::{CoreError, CoreResult};
use crate::event::CoreEvent;
use crate::judge::{
    JudgeError, JudgeFile, JudgeFileError, JudgeStatus, JudgeView, Picked, Resolved, Target,
    answers_line, judgment_line, starter_template, status_of,
};
use crate::llm::{ToolCall, ToolSpec};
use crate::model::{AgentId, Endpoint, JudgeSpec};
use crate::note;
use crate::world::Language;

/// 判断役のツール名の接頭辞（`ask_*` とは別の族 — 答えるのは判断役ではなく、判断役が選んだ相手か
/// 判定結果なので、同じ族にするとモデルが「判断役に質問した」と読む）。
pub(super) const JUDGE_TOOL_PREFIX: &str = "judge_";
/// 判断モデルの締め切り（Spec 59 の圧縮と同じ 20 秒）。**口の中では待たない** — ここで切る。
const JUDGE_DEADLINE: Duration = Duration::from_secs(20);
/// 関数名の長さの上限（`HandoffTools` と同じ）。
const MAX_TOOL_NAME: usize = 64;

/// 読み込みと検査の結果（`None` のときは鍵ごと無い = ファイルが無い）。
pub(super) type JudgeFiles = BTreeMap<AgentId, Result<JudgeFile, JudgeFileError>>;

/// 呼び出し元に生やす 1 本（有効な判断役だけ）。
#[derive(Debug, Clone)]
pub(super) struct JudgeEntry {
    /// ツール名（`judge_<id>`。長すぎれば `judge_<連番>`）。
    pub(super) tool: String,
    pub(super) id: AgentId,
    pub(super) name: String,
    pub(super) file: Arc<JudgeFile>,
    /// 行き先になりうる相手（ID, 表示名）。説明文と束ねの見出しに使う。
    pub(super) destinations: Vec<(AgentId, String)>,
}

impl JudgeEntry {
    /// 呼び出し元のモデルへ見せる定義。**問いの文面と規則は書かない** — 毎ターンのトークンに
    /// なるうえ、モデルが規則を先読みして材料を寄せる。
    pub(super) fn spec(&self, language: Language) -> ToolSpec {
        let names: Vec<&str> = self.destinations.iter().map(|(_, n)| n.as_str()).collect();
        let description = match language {
            Language::Ja => format!(
                "**{}** に判定させる（判断役。文章を書かず、人が決めた問いと規則で行き先を決める）。\
                 `message` に判定の材料になる依頼の本文を書くこと。結果は次のどれか: \
                 {} のどれかへ `message` がそのまま渡り、その答えが戻る / 複数へ撒かれて束ねた答えが戻る / \
                 誰にも渡さず判定の結果だけが戻る / 判定できなかったと戻る（そのときは宛先を自分で選ぶ）。",
                self.name,
                if names.is_empty() { "（行き先なし）".to_owned() } else { names.join("・") },
            ),
            Language::En => format!(
                "Have **{}** judge the request (a judge: it writes no text and picks a destination by \
                 questions and rules a human wrote). Put the request text to judge in `message`. The \
                 result is one of: `message` is delivered as-is to one of {} and their answer comes back / \
                 it fans out to several and the bundled answers come back / nothing is delivered and only \
                 the judgment comes back / it could not judge (then choose the destination yourself).",
                self.name,
                if names.is_empty() { "(no destinations)".to_owned() } else { names.join(", ") },
            ),
        };
        ToolSpec {
            name: self.tool.clone(),
            description,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "message": {
                        "type": "string",
                        "description": language.pick("判定の材料（依頼の本文）", "What to judge (the request text)")
                    }
                },
                "required": ["message"],
                "additionalProperties": false
            }),
        }
    }
}

/// `connected_agents` のうち**有効な判断役**だけを、呼び出し元に生やすツールへ直す。
///
/// 判断役でない ID（サーヴァント・消えた相手）は飛ばす — 振り分けの片側（`HandoffTools` へ
/// 流すサーヴァント）は呼び手が持つ。
pub(super) async fn judge_entries(shared: &Arc<Shared>, connected: &[AgentId]) -> Vec<JudgeEntry> {
    let world = shared.world.read().await;
    let files = shared.judge_files.read().await;
    let has_model = shared.judge.read().await.is_some();
    let mut out: Vec<JudgeEntry> = Vec::new();
    for (index, id) in connected.iter().enumerate() {
        let Some(judge) = world.judge(id) else { continue };
        let file = files.get(id);
        if status_of(judge.enabled, file, |t| world.is_servant(t), has_model) != JudgeStatus::Active {
            continue;
        }
        let Some(Ok(parsed)) = file else { continue };
        let natural = format!("{JUDGE_TOOL_PREFIX}{id}");
        let tool = if natural.len() <= MAX_TOOL_NAME && !out.iter().any(|e| e.tool == natural) {
            natural
        } else {
            format!("{JUDGE_TOOL_PREFIX}{index}")
        };
        let destinations = parsed
            .targets()
            .into_iter()
            .map(|t| {
                let name = world.agent(t).map(|r| r.spec.name.clone()).unwrap_or_else(|_| t.to_string());
                (t.clone(), name)
            })
            .collect();
        out.push(JudgeEntry {
            tool,
            id: id.clone(),
            name: judge.name.clone(),
            file: Arc::new(parsed.clone()),
            destinations,
        });
    }
    out
}

/// 判定できなかった理由（計器の `reason=` と本文）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Undecided {
    JevError,
    Timeout,
    TooLarge,
    Unanswered(String),
}

impl Undecided {
    fn label(&self) -> &'static str {
        match self {
            Self::JevError => "jev_error",
            Self::Timeout => "timeout",
            Self::TooLarge => "too_large",
            Self::Unanswered(_) => "unanswered",
        }
    }

    fn text(&self, language: Language) -> String {
        let why = match (self, language) {
            (Self::JevError, Language::Ja) => "判断モデルに問い合わせられなかった".to_owned(),
            (Self::JevError, Language::En) => "the judge model could not be reached".to_owned(),
            (Self::Timeout, Language::Ja) => "判断モデルが時間内に答えなかった".to_owned(),
            (Self::Timeout, Language::En) => "the judge model did not answer in time".to_owned(),
            (Self::TooLarge, Language::Ja) => "材料が判断モデルの入力の上限を超えた".to_owned(),
            (Self::TooLarge, Language::En) => "the request is over the judge model's input limit".to_owned(),
            (Self::Unanswered(q), Language::Ja) => format!("問い「{q}」の答えが返らなかった"),
            (Self::Unanswered(q), Language::En) => format!("no answer came back for question \"{q}\""),
        };
        match language {
            Language::Ja => format!("判定できませんでした（{why}）。誰にも渡していません。宛先は自分で選んでください。"),
            Language::En => format!("Could not judge ({why}). Nothing was delivered. Choose the destination yourself."),
        }
    }
}

/// 判断役を 1 回走らせる（呼び出し元の `on_call` から）。
///
/// 戻りはツール結果の本文。**失敗も本文で返す**（ツールの失敗で会話を止めない、の既存の規律）。
#[allow(
    clippy::too_many_arguments,
    reason = "ask_agent と同じ因果の付随物（予算・打ち切り・参加者・待ちの連鎖）を運ぶ"
)]
pub(super) async fn run_judge(
    shared: &Arc<Shared>,
    caller: &AgentId,
    entry: &JudgeEntry,
    call: &ToolCall,
    hop: u8,
    parent: &tokio_util::sync::CancellationToken,
    budget: Option<&Arc<BudgetPool>>,
    participants: Option<&Participants>,
    waiting: &[AgentId],
    drops_attachment: Option<crate::attachment::AttachmentKind>,
    auto_approve_plans: bool,
) -> CoreResult<String> {
    let language = shared.world.read().await.language().unwrap_or(Language::Ja);
    let message = call
        .args
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_owned();
    let judge_id = &entry.id;
    let judge = shared.judge.read().await.clone();

    // 判断モデルへ 1 回。締め切りと打ち切りはここで切る（口の中で待ちを抱えない）。
    let asked = match judge {
        None => Err(Undecided::JevError),
        Some(judge) => {
            tokio::select! {
                biased;
                () = parent.cancelled() => {
                    note!("judge: caller={caller} judge={judge_id} rule=none outcome=interrupted");
                    return Ok(language
                        .pick("判定は打ち切られました。", "The judgment was interrupted.")
                        .to_owned());
                }
                r = tokio::time::timeout(JUDGE_DEADLINE, judge.judge(&message, &entry.file.questions)) => match r {
                    Err(_) => Err(Undecided::Timeout),
                    Ok(Err(JudgeError::TooLarge)) => Err(Undecided::TooLarge),
                    Ok(Err(JudgeError::Failed(_))) => Err(Undecided::JevError),
                    Ok(Ok(report)) => Ok(report),
                },
            }
        }
    };
    let report = match asked {
        Ok(report) => report,
        Err(why) => {
            note!(
                "judge: caller={caller} judge={judge_id} rule=none outcome=undecided reason={} \
                 input_tokens=0 output_tokens=0",
                why.label()
            );
            return Ok(why.text(language));
        }
    };
    let tokens = format!("input_tokens={} output_tokens={}", report.input_tokens, report.output_tokens);

    // 規則を上から評価する。未回答が 1 つでもあれば規則を評価しない（otherwise にも行かない）。
    let decision = match entry.file.evaluate(&report.answers) {
        Ok(d) => d,
        Err(question) => {
            let why = Undecided::Unanswered(question);
            note!(
                "judge: caller={caller} judge={judge_id} rule=none outcome=undecided reason={} {tokens}",
                why.label()
            );
            return Ok(why.text(language));
        }
    };
    let rule = match decision.picked {
        Picked::Rule(n) => n.to_string(),
        Picked::Otherwise => "otherwise".to_owned(),
    };
    let answers = answers_line(&decision.values);
    // 判定の行と note は人が書いた自由文（鍵・note）を含むので、封筒の構文を寄せる（Spec 26）。
    let line = crate::sender_envelope::defuse(&judgment_line(&entry.name, &decision.values, language)).0;
    let note_line = decision.action.note.as_deref().map(|n| {
        let n = crate::sender_envelope::defuse(n).0;
        match language {
            Language::Ja => format!("［判断の注記: {n}］"),
            Language::En => format!("[Judgment note: {n}]"),
        }
    });

    match &decision.action.target {
        Target::Return => {
            note!("judge: caller={caller} judge={judge_id} rule={rule} outcome=returned answers={answers} {tokens}");
            let mut out = vec![line];
            out.extend(note_line);
            out.push(
                language
                    .pick(
                        "判定の結果だけを返しました（誰にも渡していません）。",
                        "Returned the judgment only (nothing was delivered).",
                    )
                    .to_owned(),
            );
            Ok(out.join("\n"))
        }
        Target::To(targets) => {
            let next_hop = hop.saturating_add(1);
            if next_hop >= shared.config.max_hops {
                shared.emit(CoreEvent::HopLimitReached {
                    agent_id: caller.clone(),
                    max_hops: shared.config.max_hops,
                });
                note!("judge: caller={caller} judge={judge_id} rule={rule} outcome=hop_limit answers={answers} {tokens}");
                return Ok("転送の上限に達したため、これ以上は尋ねられません。".to_owned());
            }
            // 本文は呼び出し元の message をそのまま + 判定 1 行（+ 注記）。判断役は文章を書かない。
            let mut body = note_dropped_attachment(&message, drops_attachment, language);
            body.push_str("\n\n");
            body.push_str(&line);
            if let Some(n) = &note_line {
                body.push('\n');
                body.push_str(n);
            }
            let from = Endpoint::Agent { id: caller.clone() };
            let to_list = targets.iter().map(AgentId::as_str).collect::<Vec<_>>().join(",");
            if let [target] = targets.as_slice() {
                note!("judge: caller={caller} judge={judge_id} rule={rule} outcome=routed answers={answers} to={to_list} {tokens}");
                return Ok(deliver_and_wait(
                    shared,
                    &from,
                    target,
                    &body,
                    next_hop,
                    parent,
                    budget,
                    participants,
                    waiting,
                    "judge",
                    auto_approve_plans,
                )
                .await
                .0);
            }
            // 2 体以上 = 撒いて束ねる。**execute_wave を通らない**（波の記録・イベント・
            // 検証役が付かない）。解決ごとの hook は何もしない。
            let tasks: Vec<(AgentId, String)> = targets.iter().map(|t| (t.clone(), body.clone())).collect();
            let (interrupted, replies) = fanout_and_wait(
                shared,
                &from,
                &tasks,
                next_hop,
                parent,
                budget,
                participants,
                waiting,
                "judge",
                auto_approve_plans,
                |_, _, _| async {},
            )
            .await;
            if interrupted {
                note!("judge: caller={caller} judge={judge_id} rule={rule} outcome=interrupted answers={answers} to={to_list} {tokens}");
                return Ok(language
                    .pick("判定のあとの配送は打ち切られました。", "The delivery after the judgment was interrupted.")
                    .to_owned());
            }
            let display_of: HashMap<AgentId, String> = entry.destinations.iter().cloned().collect();
            let bundle = bundle_answers(&tasks, replies, &display_of);
            note!(
                "judge: caller={caller} judge={judge_id} rule={rule} outcome=fanned answers={answers} to={to_list} \
                 bundle_chars={} {tokens}",
                bundle.chars().count()
            );
            Ok(bundle)
        }
    }
}

/// `judges/` にある全判断役のファイルを読み、検査する（起動時）。
pub(super) async fn load_judge_files(store: &crate::config_store::ConfigStore, judges: &[JudgeSpec]) -> JudgeFiles {
    let mut files = JudgeFiles::new();
    for judge in judges {
        match store.read_judge_file(&judge.id).await {
            Ok(Some(text)) => {
                let parsed = JudgeFile::parse(&text);
                if let Err(e) = &parsed {
                    note!("WARN judge: judge={} は検査に落ちています（{}）— 無効のまま起動します", judge.id, e.location);
                }
                files.insert(judge.id.clone(), parsed);
            }
            Ok(None) => {}
            Err(err) => note!("WARN judge: judge={} のファイルを読めません — {err}", judge.id),
        }
    }
    files
}

/// 「試す」の結果（配送はしない）。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JudgeTrial {
    /// 判断モデルの版。
    pub model: String,
    /// 規則が読んだ値（問いの名前 → 表示用の文字列）。未回答の問いは載らない。
    pub values: BTreeMap<String, String>,
    /// 当たった規則（1 始まり）。`None` = otherwise または判定できなかった。
    pub rule: Option<usize>,
    /// `otherwise` に当たったか。
    pub otherwise: bool,
    /// 判定できなかった理由（`jev_error` / `timeout` / `too_large` / `unanswered`）。
    pub undecided: Option<String>,
    /// 当たった規則の行き先（`None` = return か判定できなかった）。
    pub to: Option<Vec<AgentId>>,
    /// 入力トークン。
    pub input_tokens: u64,
}

fn value_text(v: &Resolved) -> String {
    match v {
        Resolved::Choice { key, p, margin } => format!("{key}  p={p:.2}  margin={margin:.2}"),
        Resolved::Score { level, p, margin, mean } => format!("{level}  p={p:.2}  margin={margin:.2}  mean={mean:.2}"),
        Resolved::Noul { p } => format!("{p:.2}"),
    }
}

impl Orchestrator {
    /// 判断モデルを差し込む（`None` で外す）。**外すと判断役は全部無効**（ツールが生えない）。
    pub async fn set_judge(&self, judge: Option<Arc<dyn crate::judge::Judge>>) {
        *self.shared.judge.write().await = judge;
        self.shared.emit(CoreEvent::TopologyChanged);
    }

    /// 判断役の一覧と有効かどうか。
    pub async fn judges(&self) -> Vec<JudgeView> {
        let world = self.shared.world.read().await;
        let files = self.shared.judge_files.read().await;
        let has_model = self.shared.judge.read().await.is_some();
        world
            .judges()
            .into_iter()
            .map(|j| {
                let file = files.get(&j.id);
                let status = status_of(j.enabled, file, |t| world.is_servant(t), has_model);
                let (targets, outline) = match file {
                    Some(Ok(parsed)) => (parsed.targets().into_iter().cloned().collect(), Some(parsed.outline())),
                    _ => (Vec::new(), None),
                };
                JudgeView { id: j.id, name: j.name, order: j.order, enabled: j.enabled, status, targets, outline }
            })
            .collect()
    }

    /// 判断役を作る。`judge.toml` が無ければ雛形を書く（有効になるのは Jev の設定があるとき）。
    ///
    /// # Errors
    /// ID・表示名の衝突・不正（`World::register_judge`）とファイルの書き込み。
    pub async fn create_judge(&self, spec: JudgeSpec) -> CoreResult<()> {
        let id = spec.id.clone();
        let language = {
            let mut world = self.shared.world.write().await;
            world.register_judge(spec)?;
            world.language().unwrap_or(Language::Ja)
        };
        let text = match self.shared.store.read_judge_file(&id).await? {
            Some(existing) => existing,
            None => {
                let starter = starter_template(language).to_owned();
                self.shared.store.write_judge_file(&id, &starter).await?;
                starter
            }
        };
        self.shared.judge_files.write().await.insert(id, JudgeFile::parse(&text));
        self.persist().await?;
        self.shared.emit(CoreEvent::TopologyChanged);
        Ok(())
    }

    /// 判断役の表示名と並びを差し替える。
    ///
    /// # Errors
    /// 判断役が無い・表示名の衝突。
    pub async fn update_judge(&self, spec: JudgeSpec) -> CoreResult<()> {
        self.shared.world.write().await.update_judge(spec)?;
        self.persist().await?;
        self.shared.emit(CoreEvent::TopologyChanged);
        Ok(())
    }

    /// 判断役を消す（線・座標・ファイルも）。
    ///
    /// # Errors
    /// 判断役が無い・ディレクトリを消せない。
    pub async fn delete_judge(&self, id: &AgentId) -> CoreResult<()> {
        self.shared.world.write().await.remove_judge(id)?;
        self.shared.judge_files.write().await.remove(id);
        self.shared.store.remove_judge_dir(id).await?;
        self.persist().await?;
        self.shared.emit(CoreEvent::TopologyChanged);
        Ok(())
    }

    /// `judge.toml` の本文（無ければ空文字）。
    ///
    /// # Errors
    /// 判断役が無い・読めない。
    pub async fn read_judge_file(&self, id: &AgentId) -> CoreResult<String> {
        if self.shared.world.read().await.judge(id).is_none() {
            return Err(CoreError::AgentNotFound(id.to_string()));
        }
        Ok(self.shared.store.read_judge_file(id).await?.unwrap_or_default())
    }

    /// `judge.toml` を保存する。**検査に落ちたら保存しない**（形・文法・型・定義域・`to` の実在）。
    ///
    /// # Errors
    /// 判断役が無い / [`CoreError::InvalidJudgeFile`] / 書き込み。
    pub async fn save_judge_file(&self, id: &AgentId, text: &str) -> CoreResult<()> {
        let parsed = {
            let world = self.shared.world.read().await;
            if world.judge(id).is_none() {
                return Err(CoreError::AgentNotFound(id.to_string()));
            }
            check_judge_text(&world, text)?
        };
        self.shared.store.write_judge_file(id, text).await?;
        self.shared.judge_files.write().await.insert(id.clone(), Ok(parsed));
        self.shared.emit(CoreEvent::TopologyChanged);
        Ok(())
    }

    /// 「試す」— 編集中の本文とサンプルの文で判断モデルを呼び、どの規則に当たるかを返す。
    /// **配送はしない。** 押したときだけ外へ出る（開いただけでは出ない）。
    ///
    /// # Errors
    /// 本文が検査に落ちる（[`CoreError::InvalidJudgeFile`]）。判断モデルの失敗は `undecided` で返す。
    pub async fn try_judge(&self, text: &str, message: &str) -> CoreResult<JudgeTrial> {
        let parsed = JudgeFile::parse(text).map_err(|e| CoreError::InvalidJudgeFile {
            location: e.location,
            message: e.message,
        })?;
        let undecided = |reason: &str| JudgeTrial {
            model: String::new(),
            values: BTreeMap::new(),
            rule: None,
            otherwise: false,
            undecided: Some(reason.to_owned()),
            to: None,
            input_tokens: 0,
        };
        let Some(judge) = self.shared.judge.read().await.clone() else {
            return Ok(undecided("jev_error"));
        };
        let report = match tokio::time::timeout(JUDGE_DEADLINE, judge.judge(message, &parsed.questions)).await {
            Err(_) => return Ok(undecided("timeout")),
            Ok(Err(JudgeError::TooLarge)) => return Ok(undecided("too_large")),
            Ok(Err(JudgeError::Failed(_))) => return Ok(undecided("jev_error")),
            Ok(Ok(r)) => r,
        };
        let mut trial = JudgeTrial {
            model: report.model.clone(),
            values: BTreeMap::new(),
            rule: None,
            otherwise: false,
            undecided: None,
            to: None,
            input_tokens: report.input_tokens,
        };
        match parsed.evaluate(&report.answers) {
            Err(_) => trial.undecided = Some("unanswered".to_owned()),
            Ok(d) => {
                trial.values = d.values.iter().map(|(k, v)| (k.clone(), value_text(v))).collect();
                match d.picked {
                    Picked::Rule(n) => trial.rule = Some(n),
                    Picked::Otherwise => trial.otherwise = true,
                }
                if let Target::To(ids) = &d.action.target {
                    trial.to = Some(ids.clone());
                }
            }
        }
        Ok(trial)
    }
}

/// サーヴァントを消したとき、そのサーヴァントを `to` に書いている判断役を**名指しで**知らせる
/// （黙って無効にしない）。判断役は有効の述語で自動的に無効になる。知らせ方は役職の変更と同じ
/// System 行（from: System / to: User。記録のみで配送しない）。
pub(super) async fn note_judges_losing(shared: &Arc<Shared>, removed: &AgentId) {
    let (language, hits): (Language, Vec<(AgentId, String)>) = {
        let world = shared.world.read().await;
        let files = shared.judge_files.read().await;
        let hits = world
            .judges()
            .into_iter()
            .filter(|j| matches!(files.get(&j.id), Some(Ok(p)) if p.targets().contains(&removed)))
            .map(|j| (j.id, j.name))
            .collect();
        (world.language().unwrap_or(Language::Ja), hits)
    };
    for (id, name) in hits {
        note!("judge disabled: judge={id} reason=missing_targets targets={removed}");
        let text = match language {
            Language::Ja => format!(
                "判断役 {id}（{name}）は行き先の {removed} が消えたため無効になりました。judge.toml を直してください。"
            ),
            Language::En => format!(
                "Judge {id} ({name}) is disabled because its destination {removed} was removed. Fix its judge.toml."
            ),
        };
        shared
            .record(crate::model::AgentMessage::new(Endpoint::System, Endpoint::User, text, 0))
            .await;
    }
}

/// `judge.toml` の本文の検査（形・文法・型・定義域・`to` の実在）。
///
/// **保存（[`Orchestrator::save_judge_file`]）と AI 下書き補助（Spec 63 D8）の 1 実装。**
/// 2 箇所に書くと、下書きでは通るのに保存で落ちる形が生まれる。
///
/// # Errors
/// [`CoreError::InvalidJudgeFile`]（落ちた場所と理由）。
pub(super) fn check_judge_text(world: &crate::world::World, text: &str) -> CoreResult<JudgeFile> {
    let parsed = JudgeFile::parse(text).map_err(|e| CoreError::InvalidJudgeFile {
        location: e.location,
        message: e.message,
    })?;
    let missing = parsed.missing_targets(|t| world.is_servant(t));
    if !missing.is_empty() {
        let names = missing.iter().map(AgentId::as_str).collect::<Vec<_>>().join(", ");
        return Err(CoreError::InvalidJudgeFile {
            location: "to".to_owned(),
            message: format!("サーヴァントではない ID があります: {names}"),
        });
    }
    Ok(parsed)
}
