//! 予定の発火（Spec 07 / 28）。ticker・前判定・配送・完了後の要約。
//!
//! **前判定つきの予定だけ経路が別。** schedule_tick はティッカーの select! の
//! 中で await されているので、前判定をそこで待つと timeoutSecs（上限 3600 秒）の
//! あいだティッカーごと止まり、同じループが担う AgentTyping の排出まで止まる —
//! 1 件の監視スクリプトが村中の予定を止める形になる（Spec 28 P2a）。

use super::*;

/// 予定の発火判定を回すタスクを起こす（Spec 07）。
///
/// [`spawn_stats_ticker`] と同じく `Weak` を握る。加えてイベント購読を持ち、
/// [`CoreEvent::AgentTyping`] の `active: false` で二重発火ガードの集合から
/// 相手を外す — tick を待たずに処理するので、ガードの解除が最大 30 秒
/// 遅れることはない。
///
/// 最初の tick は 1 間隔ぶん待ってから。`tokio::time::interval` の既定は
/// 即時発火で、起動の瞬間（フロントの覆いがまだ出ている間）に予定が走るのは
/// 誰も見ていない発火になる。
pub(super) fn spawn_schedule_ticker(
    shared: Weak<Shared>,
    runtime: Arc<ScheduleRuntime>,
    mut events: broadcast::Receiver<CoreEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let interval = match shared.upgrade() {
            Some(s) => s.config.schedule_interval,
            None => return,
        };
        let mut ticker =
            tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
        // 眠っていた PC が起きた直後に溜まった tick を連射しない。
        // 発火規則は「now 以前の直近の予定時刻」を毎回求めるので、
        // tick を密に打ち直しても同じ判定を繰り返すだけになる。
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    let Some(shared) = shared.upgrade() else { break };
                    schedule_tick(&shared, &runtime, chrono::Local::now()).await;
                }
                event = events.recv() => match event {
                    Ok(CoreEvent::AgentTyping { agent_id, active: false }) => {
                        runtime.in_flight.lock().await.remove(&agent_id);
                        // 根のターンが終わった = この因果で待った相手は全員
                        // 答え終わっている（`ask` / `plan` は答えを待って初めて
                        // 呼び出し元のターンが進むため）。ここが自動要約の起点。
                        let pending = runtime.pending_summaries.lock().await.remove(&agent_id);
                        if let Some(participants) = pending
                            && let Some(shared) = shared.upgrade()
                        {
                            // **切り離す。** ここで待つと、要約の LLM 呼び出しの間
                            // ティッカーが止まる（前判定と同じ理由）。
                            tokio::spawn(async move {
                                summarize_causality(&shared, &agent_id, &participants).await;
                            });
                        }
                    }
                    Ok(_) => {}
                    // 取りこぼしたら fail open（集合を空にする）。塞がったままに
                    // すると予定が二度と発火しない静かな停止になり、
                    // 稀な二重発火より悪い（Spec 07 Notes 5）。
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        runtime.in_flight.lock().await.clear();
                        // **要約待ちも一緒に捨てる。** 完了の合図を取りこぼした以上、
                        // 待ち続けても起点は二度と来ない。要約は次の発火でやり直せる。
                        runtime.pending_summaries.lock().await.clear();
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
            }
        }
    })
}

/// 予定 1 巡ぶんの判定と実行（Spec 07 の配線層 + Spec 28 の前判定）。
///
/// 判定そのものは [`ScheduledTask::decide`]（純関数）に委ね、ここは
/// **副作用の順序**だけを持つ: 配送 → 記録 → 消化 → 保存。
/// 消化の書き込みが配送成功の後にあるので、途中で落ちても
/// 「消化したのに発火していない」は起きない（逆の「発火したのに消化が
/// 残っていない」は再発火として現れ、既知の制限に含まれる）。
///
/// ## 前判定つきの予定だけ経路が別なのはなぜか（Spec 28 P2）
///
/// **この関数はティッカーの `select!` の中で await されている。** 前判定を
/// ここで待つと、`timeoutSecs`（上限 3600 秒）のあいだ**ティッカーごと止まり**、
/// 同じループが担っている [`CoreEvent::AgentTyping`] の排出まで止まる
/// （`in_flight` が掃除されず、受信が lag して fail open を誘発する）。
/// ゆえに前判定つきの予定は**切り離したタスクへ出す**。
///
/// 帰結として:
/// - **前判定つきは、決めた時点で消化する。** どの結末でも消化するので
///   （Spec 28 D1）、結末を待たずに書いてよい
/// - **束ねる配送は前判定なしの予定だけ**。前判定つきは結末が出た順に
///   独立して配送する — 非同期に散る結末を「同じ tick」として束ねられない
pub(super) async fn schedule_tick<Tz: chrono::TimeZone>(
    shared: &Arc<Shared>,
    runtime: &Arc<ScheduleRuntime>,
    now: chrono::DateTime<Tz>,
) {
    let tasks: Vec<ScheduledTask> = shared.schedules.read().await.clone();
    let mut consumed: Vec<(String, u64)> = Vec::new();
    // 前判定なしの発火。**判定を全件済ませてから**セッションを 1 回だけ
    // 切り替えて一括配送する（Spec 28 D6）。
    //
    // **消化する予定時刻は判定から運ぶ。** 配送の時点で引き直すと、
    // 判定に使った時刻と消化に書く時刻が別物になる（tick の `now` は
    // 引数で受け取っており、実時計とは限らない）。
    let mut candidates: Vec<(ScheduledTask, u64)> = Vec::new();

    for task in tasks {
        match task.decide(&now) {
            Tick::Idle => {}
            Tick::Consume { due_ms } => {
                // 猶予超過。debug ログのみ — 「閉じていた」の事後報告は直す手が
                // 無く、数日分まとめて会話ログへ流すと本物の通知が埋まる。
                note!(
                    "schedule: {}（{}）の予定時刻を猶予超過で消化（発火せず）",
                    task.id,
                    task.recurrence.label_ja()
                );
                consumed.push((task.id.clone(), due_ms));
            }
            Tick::Fire { due_ms } => {
                let running = shared.mailboxes.read().await.contains_key(&task.to);
                if !running {
                    // 停止中へは撒かない。消化して、会話ログへ 1 行だけ残す
                    // （消化するのでログも 1 回だけになる）。
                    let name = {
                        let world = shared.world.read().await;
                        world
                            .agent(&task.to)
                            .map(|record| record.spec.name.clone())
                            // 宛先が削除済みでも通知は成立させる（ID で示す）。
                            .unwrap_or_else(|_| task.to.to_string())
                    };
                    // System 行は記録時の言語で書く（Spec 35 D6）。
                    let language = shared
                        .world
                        .read()
                        .await
                        .language()
                        .unwrap_or(crate::world::Language::Ja);
                    let text = match language {
                        crate::world::Language::Ja => format!(
                            "{}（{name}）への予定「{}」を飛ばしました（停止中）",
                            task.to,
                            task.recurrence.label_ja()
                        ),
                        crate::world::Language::En => format!(
                            "Skipped the schedule \"{}\" for {} ({name}) — the agent is stopped",
                            task.recurrence.label_en(),
                            task.to,
                        ),
                    };
                    shared
                        .record(AgentMessage::new(Endpoint::System, Endpoint::User, text, 0))
                        .await;
                    consumed.push((task.id.clone(), due_ms));
                    continue;
                }

                // まだ働いている相手に積み増さない（二重発火の軽い護り）。
                // 消化しないので次の tick で再判定される — 壁時計系は待つうちに
                // 猶予を超えれば Consume へ倒れる。それで正しい。
                if runtime.in_flight.lock().await.contains(&task.to) {
                    continue;
                }

                // 前判定が走っている予定へ 2 本目を出さない（Spec 28）。
                // **消化しない**のは上と同じ理由 — 走り終われば次の tick が拾う。
                // ここを塞がないと、間隔より長い前判定でプロセスが積み上がる。
                if task.probe.is_some() && runtime.probing.lock().await.contains(&task.id) {
                    continue;
                }

                match task.probe {
                    // 前判定なし。従来どおり束ねる側へ回す。
                    None => candidates.push((task, due_ms)),
                    // 前判定あり。**結末を待たずに消化して**切り離す
                    // （どの結末でも消化するので、待つ理由が無い）。
                    Some(_) => {
                        consumed.push((task.id.clone(), due_ms));
                        runtime.probing.lock().await.insert(task.id.clone());
                        let shared = Arc::clone(shared);
                        let runtime = Arc::clone(runtime);
                        tokio::spawn(async move {
                            probe_and_deliver(&shared, &runtime, task).await;
                        });
                    }
                }
            }
        }
    }

    deliver_batch(shared, runtime, candidates, &mut consumed).await;

    if consumed.is_empty() {
        return;
    }

    // 消化をまとめて書き込む。書き込みロックを持ったまま保存するのは
    // CRUD 側（create/delete/set_enabled）と同じ理由 — 書き手が 2 系統ある。
    let mut schedules = shared.schedules.write().await;
    for (id, due_ms) in consumed {
        if let Some(task) = schedules.iter_mut().find(|task| task.id == id) {
            task.last_consumed_due_ms = Some(due_ms);
        }
    }
    if shared.schedules_blocked.is_none() {
        if let Err(err) = shared.store.save_schedules(&schedules).await {
            // 保存失敗は発火を止める理由にならない。in-memory は既に消化済みで
            // 二重発火は起きず、次の消化で再度保存を試みる。
            note!("schedule: schedules.json の保存に失敗しました: {err}");
        }
    }
}

/// 前判定なしの発火をまとめて配送する（Spec 28 D6）。
///
/// **セッションの切り替えは全件ぶん 1 回だけ。** 1 件ずつ切り替えると、
/// 同じ時刻に複数の予定が発火した村で画面が続けて 2 回切り替わる。
/// `fresh` が 1 件でもあれば新しい会話を 1 つ作り、**この tick の配送は
/// `continue` のぶんも含めて全部そこへ積む** — セッションは村全体の単位なので、
/// 同時刻の発火が同じ会話に載るほうが一貫している。
async fn deliver_batch(
    shared: &Arc<Shared>,
    runtime: &ScheduleRuntime,
    candidates: Vec<(ScheduledTask, u64)>,
    consumed: &mut Vec<(String, u64)>,
) {
    if candidates.is_empty() {
        return;
    }
    if candidates
        .iter()
        .any(|(task, _)| task.session_mode == SessionMode::Fresh)
    {
        start_fresh_session(shared).await;
    }
    for (task, due_ms) in candidates {
        // **消化するのは配送できたときだけ**（従来の規律を維持）。
        // 背圧で見送った回は消化せず、次の tick が拾い直す。
        if deliver_scheduled(shared, runtime, &task, String::new()).await {
            consumed.push((task.id.clone(), due_ms));
        }
    }
}

/// 発火 1 件を配送する。配送できたら真。
///
/// **配送してから記録する。** 逆にすると、受信箱が飽和していた場合に
/// 「配られていない発話」が会話ペインへ残る。
async fn deliver_scheduled(
    shared: &Arc<Shared>,
    runtime: &ScheduleRuntime,
    task: &ScheduledTask,
    appendix: String,
) -> bool {
    // 参加者を数えるのは `summarizeAfter` の予定だけ（Spec 28 D7）。
    // **数えない因果では `None` を運ぶ** — 使わない集合を全因果で作ると、
    // 「この欄は何のためにあるのか」が読めなくなる。
    let participants: Option<Participants> = task
        .summarize_after
        .then(|| Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())));
    // 本文の先頭に由来を書く。封筒（【送り手: Fuseforks】）だけでは
    // モデルが人の発話と区別できない。会話ペインにもそのまま出るので
    // 利用者も定期発火だと分かる。
    let language = shared
        .world
        .read()
        .await
        .language()
        .unwrap_or(crate::world::Language::Ja);
    let base = match language {
        crate::world::Language::Ja => format!(
            "【定期実行: {}】\n{}",
            task.recurrence.label_ja(),
            task.message
        ),
        // 【】は封筒と同じく両言語で共通（構造の印。Spec 35 D5 と同じ判断）。
        crate::world::Language::En => format!(
            "【Scheduled run: {}】\n{}",
            task.recurrence.label_en(),
            task.message
        ),
    };
    // 前判定の出力を添える。**添えないとサーヴァントが同じ情報を取りに行く
    // 周回が発生し、トークン節約という目的と逆行する**（Spec 28 D3）。
    let content = compose_body(&base, &appendix);

    // 予定発火はユーザー発話と同格の新しい起点なので hop は 0 —
    // そこから先の転送・委譲に満額の燃料を渡す。
    let message = AgentMessage::new(
        Endpoint::System,
        Endpoint::Agent {
            id: task.to.clone(),
        },
        content,
        0,
    );

    // 因果の根 — 予定の発火 1 回ごとに独立した予算が付く（Spec 11 S4。
    // 人が見ていない時間の安全の本体）。配送を見送った tick では
    // プールも捨てられ、次の tick が新しく作る（消費ゼロなので等価）。
    let budget = new_root_budget(shared).await;

    match deliver(shared, &task.to, message.clone(), budget, participants.clone()).await {
        Ok(()) => {
            shared.record(message).await;
            runtime.in_flight.lock().await.insert(task.to.clone());
            // 根のターンが終わったら要約する（Spec 28 D7）。**ここでは待たない** —
            // 待つと予定のループが 1 件の依頼に塞がれる。完了の合図は
            // `AgentTyping { active: false }` で、ティッカーが既に聴いている。
            if let Some(participants) = participants {
                runtime
                    .pending_summaries
                    .lock()
                    .await
                    .insert(task.to.clone(), participants);
            }
            true
        }
        Err(err) => {
            // MailboxFull（背圧）: 消化せず次の tick で再試行する。
            // NotRunning: 上の running 判定との間で停止された競合。
            // どちらも次の tick が正しく拾い直す。
            note!("schedule: {} への配送を見送りました: {err}", task.to);
            false
        }
    }
}

/// 新しい会話を起こす（`sessionMode: fresh`）。
///
/// **失敗しても発火は止めない。** 会話が切り替わらないだけで、依頼は届く —
/// 「新しい会話で始められなかったから今日の監視を落とす」は交換として悪い。
async fn start_fresh_session(shared: &Arc<Shared>) {
    let Some(store) = shared.sessions.as_ref() else {
        // 保存先の無い村では会話の単位そのものが無い。黙って続ける。
        return;
    };
    match store.create_session(None) {
        Ok(session_id) => {
            if let Err(err) = shared.open_session(&session_id).await {
                note!("WARN schedule: 新しい会話へ切り替えられませんでした: {err}");
            }
        }
        Err(err) => note!("WARN schedule: 新しい会話を起こせませんでした: {err}"),
    }
}

/// 前判定を走らせ、一致したら配送する（Spec 28）。**切り離したタスクで動く。**
///
/// 消化は呼び出し元が済ませてある — ここでの結末は「配送するかどうか」だけを
/// 決める。**走り終えたら必ず `probing` から外す**（外し忘れるとその予定は
/// 二度と発火しない静かな停止になる）。
async fn probe_and_deliver(
    shared: &Arc<Shared>,
    runtime: &Arc<ScheduleRuntime>,
    task: ScheduledTask,
) {
    let Some(probe) = task.probe.clone() else {
        return;
    };
    let (outcome, appendix) = run_probe(shared, runtime, &task.id, &probe).await;
    runtime.probing.lock().await.remove(&task.id);

    if !outcome.delivers() {
        return;
    }
    if task.session_mode == SessionMode::Fresh {
        start_fresh_session(shared).await;
    }
    deliver_scheduled(shared, runtime, &task, appendix).await;
}

/// 前判定 1 本を走らせて結末と付記を返す。
///
/// **計器は必ず 1 行出す**（`outcome` と字数だけ。stdout の中身は 1 文字も
/// 出さない — `failures.md` #71: 計器は秘密の転送経路になる）。
async fn run_probe(
    shared: &Shared,
    runtime: &ScheduleRuntime,
    task_id: &str,
    probe: &ScheduleProbe,
) -> (ProbeOutcome, String) {
    let (outcome, appendix, exit, resolved, chars) = probe_once(shared, probe).await;
    note!(
        "schedule probe: id={task_id} outcome={} exit={exit} reason={} resolved={resolved} stdout_chars={chars}",
        outcome.as_str(),
        outcome.reason(),
    );
    // 画面へ出す用に直近の結末を残す（Spec 28 D8）。**ログとは別の器**で、
    // 「いま何が起きているか」は画面、「何が起きてきたか」はログが持つ。
    runtime.last_probe.lock().await.insert(
        task_id.to_owned(),
        crate::schedule_probe::ProbeReport {
            outcome: outcome.as_str().to_owned(),
            reason: outcome.reason().to_owned(),
            at_ms: crate::model::now_ms(),
        },
    );
    (outcome, appendix)
}

/// 前判定の本体。計器へ出す値も一緒に返す。
async fn probe_once(
    shared: &Shared,
    probe: &ScheduleProbe,
) -> (ProbeOutcome, String, String, String, usize) {
    // 承認の確認が先。**未承認ならプロセスを 1 つも起こさない** —
    // 起こしてから捨てる形にすると、承認の意味が「結果を使わない」に落ちる。
    let approved = {
        let key = probe.approval_key(&shared.village_id.read().await.clone());
        match shared.probe_approvals.read().await.as_ref() {
            Some(store) => store.is_approved(&key),
            // 注入されていない = 承認を確かめる手段が無い。**走らせない側へ倒す。**
            None => false,
        }
    };
    if !approved {
        return (
            ProbeOutcome::Unapproved,
            String::new(),
            "-".to_owned(),
            "-".to_owned(),
            0,
        );
    }

    // 作業フォルダ。**検査は実行時だけ**（読み込みで弾くと、村を配った先で
    // 存在しないパスを指しているだけでスケジュール全体が読めなくなる）。
    let cwd = probe
        .cwd
        .as_ref()
        .map_or_else(|| shared.store.root().to_path_buf(), std::path::PathBuf::from);
    if !cwd.is_dir() {
        return (
            ProbeOutcome::Error(ProbeError::CwdMissing),
            String::new(),
            "-".to_owned(),
            "-".to_owned(),
            0,
        );
    }

    // **PATH は実行のたびに引き直す**（直したら次の実行から変わってほしい）。
    let Some(program) = crate::process::resolve_program(&probe.command) else {
        return (
            ProbeOutcome::Error(ProbeError::NotFound),
            String::new(),
            "-".to_owned(),
            "-".to_owned(),
            0,
        );
    };
    let resolved = crate::process::display_path(&program);

    // 打ち切りは probe 側のトークンを持たない — 人の「全ターン停止」は
    // サーヴァントのターンを止めるもので、前判定は因果の外にある。
    let ran =
        crate::process::spawn_and_wait(&program, &probe.args, &cwd, probe.timeout_secs, None).await;

    match ran {
        crate::process::Ran::Finished { code, stdout, .. } => {
            let chars = stdout.chars().count();
            let exit = code.map_or_else(|| "-".to_owned(), |c| c.to_string());
            match judge(&stdout, &probe.expect) {
                Judgement::Match { appendix } => {
                    (ProbeOutcome::Match, appendix, exit, resolved, chars)
                }
                Judgement::NoMatch => {
                    (ProbeOutcome::NoMatch, String::new(), exit, resolved, chars)
                }
                Judgement::SignalTruncated => (
                    ProbeOutcome::Error(ProbeError::SignalTruncated),
                    String::new(),
                    exit,
                    resolved,
                    chars,
                ),
            }
        }
        crate::process::Ran::TimedOut => (
            ProbeOutcome::Timeout,
            String::new(),
            "-".to_owned(),
            resolved,
            0,
        ),
        crate::process::Ran::SpawnFailed(_) | crate::process::Ran::WaitFailed(_) => (
            ProbeOutcome::Error(ProbeError::SpawnFailed),
            String::new(),
            "-".to_owned(),
            resolved,
            0,
        ),
        // 前っ判定にトークンを渡していないので、この腕は構造上通らない。
        crate::process::Ran::Cancelled => (
            ProbeOutcome::Error(ProbeError::SpawnFailed),
            String::new(),
            "-".to_owned(),
            resolved,
            0,
        ),
    }
}

/// 予定の因果が終わったので、参加した個体を要約する（Spec 28 の `summarizeAfter`）。
///
/// **対象は「根 + 待って答えを返し終えた個体」。** `handoff` で渡した先は入らない —
/// 渡した側は待っていないので、終わったことを観測する点が無い。
///
/// **人が押す「要約して続ける」との違いは対象だけ**で、要約そのものの規律
/// （本人のモデルが書く / ツールは提示しない / 保存が済んでから畳む / 空なら
/// 畳まない）は 1 つも変えていない。
async fn summarize_causality(shared: &Shared, root: &AgentId, participants: &Participants) {
    let mut targets = match participants.lock() {
        Ok(set) => set.clone(),
        // 毒されたロックで要約を諦める理由が無い（中身は名前の集合だけ）。
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    // **根は必ず対象。** 依頼を受けて答えた本人で、履歴が最も伸びている。
    targets.insert(root.clone());

    match shared.summarize_agents(Some(&targets)).await {
        Ok(0) => {}
        Ok(done) => note!("schedule summarize: root={root} agents={done}"),
        // 要約できないことは、次の発火を止める理由にならない。
        Err(err) => note!("WARN schedule summarize: root={root} に失敗しました: {err}"),
    }
}

impl Orchestrator {
    /// 登録済みの予定（登録順）。
    pub async fn schedules(&self) -> Vec<ScheduledTask> {
        self.shared.schedules.read().await.clone()
    }

    /// 予定を登録する。
    ///
    /// # Errors
    /// - 再現規則が不正な場合 [`CoreError::InvalidSchedule`]
    /// - 宛先が未登録の場合 [`CoreError::AgentNotFound`]
    /// - `schedules.json` が壊れていて書き込みが保護されている場合
    ///   [`CoreError::ScheduleStoreBlocked`]
    pub async fn create_schedule(
        &self,
        to: AgentId,
        message: String,
        recurrence: Recurrence,
        options: ScheduleOptions,
    ) -> CoreResult<ScheduledTask> {
        self.ensure_schedules_writable()?;
        // 宛先の存在確認。停止中は許す（発火時に飛ばす規則が受け止める）が、
        // 未登録は登録の時点で弾く — 発火するまで誰も気づかない予定を作らせない。
        self.shared.world.read().await.agent(&to)?;

        let task = ScheduledTask {
            id: uuid::Uuid::new_v4().to_string(),
            to,
            message,
            recurrence,
            created_at_ms: crate::model::now_ms(),
            last_consumed_due_ms: None,
            enabled: true,
            probe: options.probe,
            session_mode: options.session_mode,
            summarize_after: options.summarize_after,
        };
        // **組み立てた 1 件をまとめて検証する。** 欄ごとに検証を書くと、
        // 欄が増えたときにここを直す仕事が生える（読み込み側と同じ述語を通す）。
        task.validate().map_err(|err| CoreError::InvalidSchedule {
            reason: err.to_string(),
        })?;

        let mut schedules = self.shared.schedules.write().await;
        schedules.push(task.clone());
        // 書き込みロックを持ったまま保存する。保存を外に出すと、並んだ 2 つの
        // 変更が互いの内容を tmp ファイルで踏み合う（world.json には無い事情 —
        // あちらの書き手は UI だけだが、こちらは ticker と UI の 2 系統ある）。
        self.shared.store.save_schedules(&schedules).await?;
        Ok(task)
    }

    /// 前判定の承認を答える口を差し込む（Spec 28）。
    ///
    /// **差し込むまで前判定は 1 本も走らない**（承認を確かめる手段が無いので
    /// `unapproved` へ倒れる）。実体は GUI 層が `{app_data_dir}` に持つ —
    /// workspace に置くと承認ごと配布され、防御にならない。
    pub async fn set_probe_approvals(&self, approvals: Arc<dyn ProbeApprovals>) {
        *self.shared.probe_approvals.write().await = Some(approvals);
    }

    /// 予定ごとの直近 1 回の判定の結末（Spec 28 D8）。プロセス寿命。
    ///
    /// **不一致・失敗は会話ログへ流さない**（監視の頻度で本物の通知が埋まる）
    /// が、画面には出す — `error` / `timeout` は人が直せるので、
    /// どこにも出さないと「動かないが理由が分からない」になる。
    pub async fn probe_reports(&self) -> HashMap<String, crate::schedule_probe::ProbeReport> {
        self.schedule_runtime.last_probe.lock().await.clone()
    }

    /// この村の識別子（`{workspace}/village_id`）。承認鍵の salt。
    ///
    /// GUI は承認を書くときにこの値で鍵を組む。**コアが持っている値をそのまま
    /// 使わせる** — 2 箇所で読むと、片方がファイルを読み直して食い違う。
    pub async fn village_id(&self) -> String {
        self.shared.village_id.read().await.clone()
    }

    /// 予定を削除する。
    ///
    /// # Errors
    /// - 該当 ID が無い場合 [`CoreError::ScheduleNotFound`]
    pub async fn delete_schedule(&self, id: &str) -> CoreResult<()> {
        self.ensure_schedules_writable()?;
        let mut schedules = self.shared.schedules.write().await;
        let before = schedules.len();
        schedules.retain(|task| task.id != id);
        if schedules.len() == before {
            return Err(CoreError::ScheduleNotFound(id.to_owned()));
        }
        self.shared.store.save_schedules(&schedules).await
    }

    /// 予定の一時停止・再開（Spec 07 の `enabled`）。
    ///
    /// # Errors
    /// - 該当 ID が無い場合 [`CoreError::ScheduleNotFound`]
    pub async fn set_schedule_enabled(&self, id: &str, enabled: bool) -> CoreResult<()> {
        self.ensure_schedules_writable()?;
        let mut schedules = self.shared.schedules.write().await;
        let task = schedules
            .iter_mut()
            .find(|task| task.id == id)
            .ok_or_else(|| CoreError::ScheduleNotFound(id.to_owned()))?;
        task.enabled = enabled;
        self.shared.store.save_schedules(&schedules).await
    }

    /// 予定の発火判定を 1 回実行する。
    ///
    /// 通常はティッカーが `Local::now()` で呼ぶ。**時刻を引数に取るのは
    /// テストのため**（壁時計に依存するテストを書かない — Spec 04 の規律）。
    pub async fn run_schedule_tick<Tz: chrono::TimeZone>(&self, now: chrono::DateTime<Tz>) {
        schedule_tick(&self.shared, &self.schedule_runtime, now).await;
    }

    /// `schedules.json` が読めない状態での書き込みを拒否する。
    fn ensure_schedules_writable(&self) -> CoreResult<()> {
        match &self.shared.schedules_blocked {
            Some(reason) => Err(CoreError::ScheduleStoreBlocked {
                reason: reason.clone(),
            }),
            None => Ok(()),
        }
    }
}
