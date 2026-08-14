//! 委譲と転送（Spec 04 / 20）。`ask_*` / `plan` / `transfer_to_*` の
//! 提示・判定・配送。
//!
//! **1 ファイルに集めたのは、変更履歴がここで繋がっているから** — 95 コミットの
//! ペア集計で handle_message と HandoffTools が 9 回、run_plan が 9 回、
//! ask_agent が 7 回、deliver_and_wait が 5 回、同時に変わっている。
//! 因果が輪になっている（`ask` が `deliver_and_wait` を呼び、それが新しい
//! ターンを起こす）ためで、提示・判定・配送を別ファイルへ分けると
//! 1 つの変更が散る。
//!
//! **答えの行き先が逆**なのがこの層の核心。`ask_*` は依頼主へ戻り（戻り先は
//! `reply_to` が決めておりモデルは触れない）、`transfer_to_*` は会話ごと渡す。
//! 取り違えると 1 つの依頼が 2 本に分裂する（`failures.md` #96）。

use super::*;

use crate::world::Language;

/// 他のエージェントへ質問し、**答えを待って**返す（委譲）。
///
/// 転送との違いは行き先だけ。転送は制御ごと渡してユーザーへ返るが、委譲は
/// 答えが呼び出し元へ戻り、ツール結果として会話が続く。
///
/// **必ず有限時間で戻る。** 相手が応答しない・相互に委譲し合う配置では
/// 待ち合わせが起きうるので、上限で打ち切って理由を文字列で返す
/// （ツールの失敗は会話を止めない、という既存の規律に合わせる）。
#[allow(clippy::too_many_arguments)]
pub(super) async fn ask_agent(
    shared: &Arc<Shared>,
    from: &AgentId,
    to: &AgentId,
    call: &crate::llm::ToolCall,
    hop: u8,
    parent: &tokio_util::sync::CancellationToken,
    budget: Option<&Arc<BudgetPool>>,
    participants: Option<&Participants>,
    drops_attachment: Option<crate::attachment::AttachmentKind>,
) -> CoreResult<String> {
    // 依頼元のターンに添付が付いていたら、届かないことを本文で断る（D6）。
    // System 行と同じく**記録時の言語**で書く（Spec 35 D6）。
    let language = shared
        .world
        .read()
        .await
        .language()
        .unwrap_or(crate::world::Language::Ja);
    let question = note_dropped_attachment(
        call.args
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or_default(),
        drops_attachment,
        language,
    );

    let next_hop = hop.saturating_add(1);
    if next_hop >= shared.config.max_hops {
        shared.emit(CoreEvent::HopLimitReached {
            agent_id: from.clone(),
            max_hops: shared.config.max_hops,
        });
        return Ok("転送の上限に達したため、これ以上は尋ねられません。".to_owned());
    }

    // ask は分類を捨てる（.0）。分類は波ペインの素材で、ask の関心ではない。
    Ok(deliver_and_wait(
        shared,
        &Endpoint::Agent { id: from.clone() },
        to,
        &question,
        next_hop,
        parent,
        budget,
        participants,
    )
    .await
    .0)
}

/// 1 件の依頼を配送し、答えを待つ（`ask` と `plan` の共通部分）。
///
/// **切り出してあるのは、2 つの経路で失敗の文言と境界を揃えるため。**
/// 別々に書くと、同じ配置で ask は通り plan は止まる、という説明できない差が
/// いずれ生まれる。`hop` の判定は呼び出し側に置く — plan では波全体で
/// 一様に決まる制約なので、タスクごとに判定すると同じ文字列が人数分並ぶ。
///
/// 戻り値は**必ず文字列と分類の組**。相手が停止中でも無応答でも例外にしない
/// （ツールの失敗で会話を止めない、という既存の規律）。分類（Spec 08）は
/// 波ペインのセル色の素材で、`ask` 側は捨てるだけ — 計時も同じ理由で
/// ここに入れない（plan の観測の関心を ask に背負わせない）。
#[allow(
    clippy::too_many_arguments,
    reason = "因果の付随物（予算・打ち切り・参加者）は 1 つの構造体へ畳めるが、\
              畳むと『どの配送が何を運ぶか』が呼び出し側から読めなくなる。\
              ask / plan の 2 つのハンドラも同じ理由で許容済み"
)]
pub(super) async fn deliver_and_wait(
    shared: &Arc<Shared>,
    from: &Endpoint,
    to: &AgentId,
    question: &str,
    next_hop: u8,
    parent: &tokio_util::sync::CancellationToken,
    budget: Option<&Arc<BudgetPool>>,
    participants: Option<&Participants>,
) -> (String, PlanTaskState) {
    // 予算が尽きていたら配送そのものを始めない（token_budget の exhaustion —
    // 「新しい配送を始めない」の実装点。波の並列配送でも、兄弟タスクの消費で
    // 先に尽きたらここで止まる）。
    if let Some(pool) = budget
        && !pool.has_remaining()
    {
        return (
            "トークン予算の上限に達したため、配送していません。".to_owned(),
            PlanTaskState::BudgetExhausted,
        );
    }

    // 送り手は `Endpoint` で受ける（Spec 25）。**ここに `Endpoint::Agent` を
    // 焼き込んでいたので、外部依頼が「サーヴァント発」に化けていた** —
    // 待ち方・失敗の分類・予算の検査は 1 字も変えずに、送り手だけを広げる。
    let mut outgoing =
        AgentMessage::new(from.clone(), Endpoint::Agent { id: to.clone() }, question, next_hop);
    // 質問自体のトークンは呼び出し元のターンに計上済み。二重計上しない。
    outgoing.tokens = 0;
    shared.record(outgoing.clone()).await;

    let (tx, rx) = tokio::sync::oneshot::channel();
    let envelope = Envelope {
        incoming: outgoing,
        reply_to: Some(tx),
        // 依頼元ターンの子（Spec 10 Phase 2）。依頼元が切られたら、この封筒が
        // 生んだ仕事（未着手の畳み・飛行中の検知）だけが連鎖して止まる。
        // 受信側の別の依頼は別トークンなので巻き添えにならない。
        cancel: Some(parent.child_token()),
        // 依頼元と同じ因果 = 同一の Arc（新しいプールを作らない）。
        budget: budget.cloned(),
        // 参加者も同じ因果を運ぶ。**入れるのは答えが返った後**（下）。
        participants: participants.cloned(),
    };

    if let Err(err) = deliver_envelope(shared, to, envelope).await {
        // 相手が停止中・受信箱が飽和。会話は止めず、モデルに事実を返す。
        return (
            format!("相手に尋ねられませんでした: {err}"),
            PlanTaskState::Undeliverable,
        );
    }

    match tokio::time::timeout(shared.config.ask_timeout, rx).await {
        // 答え（Answered）か転送の事実（HandedOff）。刻み手は handle_message。
        Ok(Ok(reply)) => {
            // **ここが「待って完了した」の唯一の観測点**（Spec 28 D7）。
            // `handoff` はこの関数を通らないので、渡した先は入らない。
            //
            // **転送の事実（HandedOff）では数えない。** 返ってきたのは
            // 「別の人へ回した」という事実であって、この個体が仕事を
            // 終えた証拠ではない — 履歴が伸びていないので要約する物が無い。
            if reply.kind == PlanTaskState::Answered
                && let Some(set) = participants
                && let Ok(mut set) = set.lock()
            {
                set.insert(to.clone());
            }
            (reply.text, reply.kind)
        }
        // 相手が答えずにタスクを終えた（停止・失敗）。転送で応じた場合は
        // handle_message が事実を送るので、ここへは来ない。
        Ok(Err(_)) => (
            "相手から答えが返りませんでした。".to_owned(),
            PlanTaskState::NoAnswer,
        ),
        Err(_) => (
            "相手からの答えが時間内に返りませんでした。".to_owned(),
            PlanTaskState::TimedOut,
        ),
    }
}

/// 並列委譲（`plan`）を 1 波ぶん実行する（Spec 04）。
///
/// # 失敗の 3 分類
///
/// 処方が分かれる根拠は「**その値がいつ確定するか**」の 1 点だけ:
///
/// - **静的な不正**（波の中で不変・事前に確かめられる）→ **何も配送せず差し戻す**
/// - **波全体で一様な制約**（波の中で不変・全タスクが同値）→ **1 つの結果文字列**
/// - **動的な失敗**（配送の瞬間まで確定しない）→ **そのタスクの結果文字列**
///
/// 部分実行を避けるのは、「どこまで走ったか」の追跡を利用者に強いるから。
/// ただし稼働状態は**確かめても配送時には別の値でありうる**ので検証に含めない。
/// 確かめられないものを検証に含めると、嘘の保証になる。
///
/// 戻り値は 3 分類のいずれも `String`。エラーチャネルを使わないのは、
/// `Err` を返すと実行ループが「ツールの実行に失敗しました」で包み、
/// モデルが読むべき「なぜ配送されなかったか」が一段深い所へ埋まるため。
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_plan(
    shared: &Arc<Shared>,
    from: &AgentId,
    handoffs: &HandoffTools,
    call: &crate::llm::ToolCall,
    hop: u8,
    wave: u32,
    parent: &tokio_util::sync::CancellationToken,
    budget: Option<&Arc<BudgetPool>>,
    participants: Option<&Participants>,
    drops_attachment: Option<crate::attachment::AttachmentKind>,
) -> String {
    // 断り書きは記録時の言語で書く（Spec 35 D6）。波の全タスクで同じ値なので
    // ループの外で 1 回だけ引く。
    let language = shared
        .world
        .read()
        .await
        .language()
        .unwrap_or(crate::world::Language::Ja);

    // 1. 静的な不正を全件見る。1 件でも不正なら何も配送しない。
    let Some(tasks) = call.args.get("tasks").and_then(serde_json::Value::as_array) else {
        return "plan には tasks（依頼の配列）が必要です。何も配送していません。".to_owned();
    };
    if tasks.is_empty() {
        return "plan の tasks が空です。誰にも頼まずに終わりました。\
                頼む相手が居ないなら、plan を呼ばずに自分で答えてください。"
            .to_owned();
    }

    let mut wave_tasks: Vec<(AgentId, String)> = Vec::with_capacity(tasks.len());
    for (index, task) in tasks.iter().enumerate() {
        let position = index + 1;
        let (Some(to), Some(message)) = (
            task.get("to").and_then(serde_json::Value::as_str),
            task.get("message").and_then(serde_json::Value::as_str),
        ) else {
            return format!(
                "{position} 件目の依頼に to と message の両方が必要です。何も配送していません。"
            );
        };

        let target = AgentId::from(to);
        // 提示はターンの開始時、検証は今。この間に繋ぎ替えは起こりうる。
        if !handoffs.is_target(&target) {
            return format!(
                "{position} 件目の宛先「{to}」は、あなたの接続先ではありません。\
                 頼めるのは {} です。何も配送していません。",
                handoffs.roster().join("、")
            );
        }
        if wave_tasks.iter().any(|(existing, _)| *existing == target) {
            return format!(
                "宛先「{to}」が同じ波に 2 回あります。1 回の plan で同じ相手へ頼めるのは 1 件です。\
                 2 件目は次の波で頼んでください。何も配送していません。"
            );
        }
        // 依頼元のターンに添付が付いていたら、届かないことを各依頼の本文で断る（D6）。
        wave_tasks.push((
            target,
            note_dropped_attachment(message, drops_attachment, language),
        ));
    }

    // 2. 波全体で一様に決まる制約。1 回だけ確かめ、1 つの文字列で返す
    //    （タスク数ぶん同じ文字列を並べない）。判定式は ask_agent と同一。
    let next_hop = hop.saturating_add(1);
    if next_hop >= shared.config.max_hops {
        shared.emit(CoreEvent::HopLimitReached {
            agent_id: from.clone(),
            max_hops: shared.config.max_hops,
        });
        return "転送の上限に達したため、これ以上は頼めません。何も配送していません。".to_owned();
    }

    // 観測用の波の開始行。宛先と依頼サイズを記録し、後から
    // 「この波は前の波の結果を読まずに書けたか」を追えるようにする
    // （Spec 04 Notes 2 の depends_on トリガー判定の材料）。
    note!(
        "plan wave: agent={from} wave={wave} tasks={} to=[{}] msg_chars={}",
        wave_tasks.len(),
        wave_tasks
            .iter()
            .map(|(target, _)| target.as_str())
            .collect::<Vec<_>>()
            .join(","),
        wave_tasks
            .iter()
            .map(|(_, message)| message.chars().count())
            .sum::<usize>(),
    );
    let dispatched_at = std::time::Instant::now();

    // 波の記録と告知（Spec 08）。配送ゼロの plan はここへ到達しない（上の
    // 早期 return）ので、記録と stderr の数え方は構造的に一致する。
    // 開始時刻だけが壁時計（epoch ms）、所要はすべて単調時計（Instant）。
    let started_at_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
    let announced: Vec<(AgentId, u32)> = wave_tasks
        .iter()
        .map(|(target, message)| (target.clone(), message.chars().count() as u32))
        .collect();
    let plan_id = shared
        .plan_waves
        .write()
        .await
        .begin_wave(from.clone(), wave, &announced, started_at_ms);
    shared.emit(CoreEvent::PlanWaveStarted {
        plan_id,
        agent_id: from.clone(),
        wave,
        tasks: announced
            .iter()
            .map(|(to, msg_chars)| PlanTaskAnnounced {
                to: to.clone(),
                msg_chars: *msg_chars,
            })
            .collect(),
        started_at_ms,
    });

    // 3. 並列配送。JoinSet で各タスクを実行時へ載せる — ここが `ask_*` の
    //    直列委譲との唯一の構造的な差で、壁時計が人数倍にならない理由。
    //    並列なのは**配送**であって実行ではない。各エージェントの受信箱は
    //    1 本なので、ワーカーが別の仕事で塞がっていればその分だけ待つ。
    //    タスクの所要はここで測る — deliver_and_wait に計時を入れない
    //    （ask に plan の観測の関心を背負わせない）。
    let mut set = tokio::task::JoinSet::new();
    for (index, (target, message)) in wave_tasks.iter().enumerate() {
        let shared = Arc::clone(shared);
        let from = Endpoint::Agent { id: from.clone() };
        let target = target.clone();
        let message = message.clone();
        let parent = parent.clone();
        // 波の全タスクが**同一の**プールを指す（clone は Arc の複製であって
        // プールの複製ではない）。タスクごとに新しいプールを作ると天井が
        // 人数倍に化ける — delegation-fanout race（token_budget の pool）。
        let budget = budget.cloned();
        // 参加者の集合も波の全タスクが同一の Arc を指す。**答えを返した
        // タスクだけが自分を書き込む**ので、波の中で誰が答えたかがそのまま残る。
        let participants = participants.cloned();
        set.spawn(async move {
            let task_started = std::time::Instant::now();
            let (answer, state) = deliver_and_wait(
                &shared,
                &from,
                &target,
                &message,
                next_hop,
                &parent,
                budget.as_ref(),
                participants.as_ref(),
            )
            .await;
            (index, answer, state, task_started.elapsed().as_millis() as u64)
        });
    }

    // 進行役のターンが切られたら、波の待ちもここで畳む（Spec 10 — U2）。
    // 周回境界の検査だけでは、最悪 ask_timeout（既定 180 秒）が割り込み不能の
    // まま残る。ワーカー側は封筒の子トークンが同じ cancel で連鎖して止まるので、
    // ここで待ち続けても新しい答えは（打ち切りの報告以外）もう来ない。
    let mut wave_interrupted = false;
    let mut answers: Vec<Option<String>> = vec![None; wave_tasks.len()];
    loop {
        tokio::select! {
            biased;

            () = parent.cancelled() => {
                wave_interrupted = true;
                break;
            }
            joined = set.join_next() => {
                let Some(joined) = joined else { break };
                match joined {
                    Ok((index, answer, state, elapsed_ms)) => {
                        // 解決した順に記録と event を刻む。セルは波の完了を待たず
                        // 個別に色が変わる（全滅まで灰色、にしない）。
                        let to = wave_tasks[index].0.clone();
                        shared
                            .plan_waves
                            .write()
                            .await
                            .resolve_task(plan_id, &to, state, elapsed_ms);
                        shared.emit(CoreEvent::PlanTaskResolved {
                            plan_id,
                            to,
                            state,
                            elapsed_ms,
                        });
                        answers[index] = Some(answer);
                    }
                    // タスク自体が落ちた（パニック）。1 件の異常で波ごと落とさない。
                    // 記録上は finish_wave が Running を NoAnswer に倒す。
                    Err(err) => tracing_note(&err),
                }
            }
        }
    }

    if wave_interrupted {
        // set の drop で残りの待ちを畳む。配送済みの封筒はそのまま — ワーカーは
        // 子トークンで自分の周回境界（または着手時）に止まる。答えは受け取らない
        // （部分的な束ねを作らない — 束ねると次のターンの進行役が「全員から
        // 答えが揃った」と誤読する）。
        drop(set);

        // 未解決のタスクを interrupted で確定させ、波を閉じる。倒し先が
        // no_answer でないのは、答えなかったのではなく止めさせたから。
        // frontend は planWaveFinished で残った running を no_answer に倒すので、
        // その前に 1 件ずつ resolve を流して running を残さない。
        let folded_at = dispatched_at.elapsed().as_millis() as u64;
        for (index, (to, _)) in wave_tasks.iter().enumerate() {
            if answers[index].is_none() {
                shared
                    .plan_waves
                    .write()
                    .await
                    .resolve_task(plan_id, to, PlanTaskState::Interrupted, folded_at);
                shared.emit(CoreEvent::PlanTaskResolved {
                    plan_id,
                    to: to.clone(),
                    state: PlanTaskState::Interrupted,
                    elapsed_ms: folded_at,
                });
            }
        }
        // 束ねは作らなかったので 0 文字（「何も束ねていない」の正直な大きさ）。
        note!(
            "plan wave interrupted: agent={from} wave={wave} resolved={}/{}",
            answers.iter().filter(|a| a.is_some()).count(),
            wave_tasks.len(),
        );
        shared
            .plan_waves
            .write()
            .await
            .finish_wave(plan_id, 0, folded_at);
        shared.emit(CoreEvent::PlanWaveFinished {
            plan_id,
            bundle_chars: 0,
            elapsed_ms: folded_at,
        });

        // この文字列は進行役の周回に返るが、直後の周回境界で本人も止まるので
        // モデルは読まない。読まれる前提の文言にしない（人がログで読む行）。
        return "plan はユーザーの指示で打ち切られました。".to_owned();
    }

    // 4. 束ねる。見出しは `agent_id（表示名）` — 表示名だけにしないのは、
    //    表示名の一意性がどこも保証されていないから（同名が 2 体いると
    //    どちらの答えか判別できなくなる）。順序は入力順に戻す。
    let bundle = wave_tasks
        .iter()
        .zip(answers)
        .map(|((target, _), answer)| {
            let display = handoffs.display_of(target).unwrap_or_else(|| target.as_str());
            let body = answer.unwrap_or_else(|| "答えの取得中に問題が起きました。".to_owned());
            format!("## {target}（{display}）\n{body}")
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    // 束ねの大きさを記録する（Spec 04 Notes 7 の「実測してから決める」の実測側）。
    // 束ねは進行役の履歴に積まれ、以後の波のたびに入力として運ばれる —
    // 波数 × N 体で膨らむ構造なので、上限や要約を入れるかの判断材料をここで取る。
    // 機構は入れない。測らずに入れると「効いているか分からない機構」が増えるだけ。
    let bundle_chars = bundle.chars().count() as u64;
    let elapsed_ms = dispatched_at.elapsed().as_millis() as u64;
    note!(
        "plan bundle: agent={from} wave={wave} tasks={} chars={bundle_chars} \
         elapsed_ms={elapsed_ms}",
        wave_tasks.len(),
    );

    // 波の完了（Spec 08）。Running のまま残ったタスク（JoinSet パニックの経路
    // のみ）は finish_wave が NoAnswer に倒す — 完了した波に永遠の「実行中」を
    // 残さない。
    shared
        .plan_waves
        .write()
        .await
        .finish_wave(plan_id, bundle_chars, elapsed_ms);
    shared.emit(CoreEvent::PlanWaveFinished {
        plan_id,
        bundle_chars,
        elapsed_ms,
    });
    bundle
}

/// `JoinSet` のタスク異常を握り潰さずに記録する。
///
/// このクレートはログ基盤を持たない（GUI 層に一切依存しない制約）ので、
/// 標準エラーへ 1 行出すに留める。**黙って捨てない**ことだけが目的。
fn tracing_note(err: &tokio::task::JoinError) {
    note!("plan のタスクが異常終了しました: {err}");
}


/// 1 回の応答の行き先。
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Outcome {
    /// 会話終了。ユーザーへ返す。
    Finish {
        /// 本文。
        content: String,
    },
    /// 転送して会話を続ける。
    ///
    /// 宛先は複数持てる（fan-out）。かつて単一宛先の型だったときは、
    /// モデルが並列ツール呼び出しで複数へ渡そうとしても 2 本目以降が
    /// 黙って捨てられ、「みんなに挨拶して」が原理的に成立しなかった。
    Handoff {
        /// 宛先と、それぞれへ伝える内容。空にはならない（`decide` が保証）。
        deliveries: Vec<(AgentId, String)>,
    },
}

impl Outcome {
    /// このターンで実際に発した言葉。履歴へ積むのはこちら。
    ///
    /// 複数宛先のときは宛先を添えて結合する。履歴を読むのは本人（モデル）なので、
    /// 「誰に何を言ったか」が残らないと、次のターンで自分の発言を再構成できない。
    pub(super) fn spoken(&self) -> String {
        match self {
            Self::Finish { content } => content.clone(),
            Self::Handoff { deliveries } => match deliveries.as_slice() {
                [(_, message)] => message.clone(),
                many => many
                    .iter()
                    .map(|(to, message)| format!("[→ {to}] {message}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            },
        }
    }
}

/// 転送先ごとのツール定義と、その逆引き。
pub(super) struct HandoffTools {
    /// `(ツール名, 転送先, 表示名)`。名前からの逆引きと、説明文の生成に使う。
    entries: Vec<(String, AgentId, String)>,
}

impl HandoffTools {
    /// 接続先からツール名を導く。
    ///
    /// 名前は OpenAI Agents SDK の慣習に倣って `transfer_to_<agent>`。
    /// 関数名の長さ制限（64 文字）を超える場合と、切り詰めで衝突する場合は
    /// 連番へ退避する。名前が壊れるとモデルが呼べなくなるため、
    /// 「たぶん大丈夫」で通さない。
    ///
    /// **ツール名は ID、説明は表示名**という組み合わせを採る。関数名に使えるのは
    /// `[a-zA-Z0-9_-]` だけで、日本語の表示名は潰れて識別できなくなる。一方で
    /// 会話は表示名で流れるので、説明に名前が無いとモデルは
    /// 「ザリ・ロブステル」と `agent_2` を結び付けられない。実際にそうなっており、
    /// 宛先の取り違えと「自分で全員のセリフを書く」の原因になっていた。
    pub(super) fn build(targets: &[(AgentId, String)]) -> Self {
        const MAX_TOOL_NAME: usize = 64;
        const PREFIX: &str = "transfer_to_";

        let mut entries: Vec<(String, AgentId, String)> = Vec::with_capacity(targets.len());
        for (index, (target, display)) in targets.iter().enumerate() {
            let natural = format!("{PREFIX}{target}");
            let name = if natural.len() <= MAX_TOOL_NAME
                && !entries.iter().any(|(existing, _, _)| *existing == natural)
            {
                natural
            } else {
                format!("{PREFIX}agent_{index}")
            };
            entries.push((name, target.clone(), display.clone()));
        }
        Self { entries }
    }

    /// 転送先が 1 つも無いか。
    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// この場に居る相手の一覧（表示名）。手順の説明で名簿として出す。
    fn roster(&self) -> Vec<&str> {
        self.entries
            .iter()
            .map(|(_, _, display)| display.as_str())
            .collect()
    }

    /// 委譲ツールの名前。転送ツールの `transfer_to_` を `ask_` に replace した形。
    fn ask_name(transfer_name: &str) -> String {
        transfer_name.replacen("transfer_to_", "ask_", 1)
    }

    /// 委譲（`ask_*`）のツール定義。
    ///
    /// 転送との違いは**答えの行き先**だけ。転送は制御ごと渡してユーザーへ返るが、
    /// 委譲は答えが自分に戻ってきて、自分の話を続けられる。
    pub(super) fn ask_specs(&self, language: Language) -> Vec<ToolSpec> {
        self.entries
            .iter()
            .map(|(name, _, display)| ToolSpec {
                name: Self::ask_name(name),
                description: match language {
                    Language::Ja => format!(
                        "**{display}** に質問し、**その答えを受け取る**。\
                         答えは自分に戻ってくるので、それを踏まえて話を続けられる。\
                         相手に話を引き継いで自分は退く場合は、これではなく \
                         `transfer_to_*` を使うこと。"
                    ),
                    Language::En => format!(
                        "Ask **{display}** a question and **receive their answer**. \
                         The answer comes back to you, so you can continue with it. \
                         To hand the conversation over and step aside, use \
                         `transfer_to_*` instead."
                    ),
                },
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "message": {
                            "type": "string",
                            "description": language.pick("相手に尋ねる内容", "What to ask them")
                        }
                    },
                    "required": ["message"],
                    "additionalProperties": false
                }),
            })
            .collect()
    }

    /// 委譲ツール名から相手を逆引きする。
    pub(super) fn resolve_ask(&self, name: &str) -> Option<&AgentId> {
        self.entries
            .iter()
            .find(|(tool, _, _)| Self::ask_name(tool) == name)
            .map(|(_, target, _)| target)
    }

    /// wire へ載せるツール定義。
    pub(super) fn specs(&self, language: Language) -> Vec<ToolSpec> {
        self.entries
            .iter()
            .map(|(name, _, display)| ToolSpec {
                name: name.clone(),
                description: match language {
                    Language::Ja => format!(
                        "**{display}** へメッセージを渡して、会話を続ける。\
                         相手は自分で考えて返事をするので、返事を代筆しないこと。\
                         自分の応答で用が足りるなら、このツールを呼ばずに本文だけを返すこと。"
                    ),
                    Language::En => format!(
                        "Pass the conversation to **{display}** with a message. \
                         They think and reply for themselves — never write their \
                         reply for them. If your own response is enough, return \
                         plain text without calling this tool."
                    ),
                },
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "message": {
                            "type": "string",
                            "description": language.pick("相手に伝える内容", "What to tell them")
                        }
                    },
                    "required": ["message"],
                    "additionalProperties": false
                }),
            })
            .collect()
    }

    /// 手順の説明。
    ///
    /// OpenAI Agents SDK が `RECOMMENDED_PROMPT_PREFIX` で同種の説明を
    /// プロンプトへ足すのと同じ意図。ツールを渡すだけでは、
    /// 「呼ばない」という選択が終了を意味することがモデルに伝わらない。
    pub(super) fn protocol_note(
        &self,
        tools_available: bool,
        offer_transfer: bool,
        awaiting_reply: bool,
        language: Language,
    ) -> String {
        // **答えがどこへ返るか。** 委譲（`ask` / `plan`）で来たターンは依頼主へ
        // 戻り、それ以外は利用者へ流れる（`Outcome::Finish` の `destination` が
        // `reply_to` の有無で決まるのと**同じ 1 つの事実**）。
        //
        // 分けるのは、**起きないことを起きると書かない**ため。委譲で呼ばれた
        // ターンに「結果が人間へ返ります」と書くと、この村で実際に起きた
        // 取り違え（答えの行き先）を手順の文の側から助長する。
        //
        // **文言は保証ではない**（#84）。取り違えを構造で塞ぐのは
        // `offer_transfer` の側で、ここは嘘を書かないためだけに分けている。
        //
        // **`tools_available == false` の枝ではこれを使わない。** あちらは
        // ツール非対応モデルの旧経路で、委譲で呼ばれても終了マーカーが無ければ
        // 最初の相手へ渡る — **挙動が違うので同じ文は当てられない**
        // （当てると「頼んだ相手へ戻ります」が新しい嘘になる）。
        let ending = match (awaiting_reply, language) {
            (true, Language::Ja) => {
                "その時点であなたの仕事は終わり、**あなたに頼んだ相手へ答えが戻ります**。"
            }
            (false, Language::Ja) => "その時点で会話は終わり、結果が人間へ返ります。",
            (true, Language::En) => {
                "At that point your job is done and **your answer goes back to whoever asked you**."
            }
            (false, Language::En) => {
                "At that point the conversation ends and the result goes to the human."
            }
        };
        if tools_available {
            // **提示していない道具を手順で名指ししない。** 転送を落とした個体に
            // 「`transfer_to_*` を呼んでください」と書くと、存在しないツールを
            // 探させることになる（提示集合と手順が食い違う）。
            let delegation = match (offer_transfer, language) {
                (true, Language::Ja) => {
                    "他のエージェントの助けが要るときだけ `transfer_to_*` ツールを呼んでください。\
                     **複数の相手へ渡すときは、それぞれの `transfer_to_*` を同じ応答の中で同時に呼んでください**。\
                     全員へ並行して届きます。\n\
                     相手の答えを受け取って自分の話を続けたいときは、`transfer_to_*` ではなく \
                     `ask_*` を使ってください — **答えが自分に戻ります**。"
                }
                (false, Language::Ja) => {
                    "他のエージェントの助けが要るときは `ask_*` ツールを呼んでください。\
                     **答えは自分に戻ってくる**ので、それを踏まえて自分の言葉でまとめてください。\
                     **複数の相手へ同時に訊くときは、それぞれの `ask_*` を同じ応答の中で呼んでください**。\n\
                     **あなたは会話を他のエージェントへ引き渡せません。**\
                     最後にまとめて答えるのはあなたです。"
                }
                (true, Language::En) => {
                    "Call a `transfer_to_*` tool only when you need another agent's help. \
                     **To hand off to several agents, call each `transfer_to_*` in the \
                     same response** — they all receive it in parallel.\n\
                     If you want their answer back so you can continue yourself, use \
                     `ask_*` instead of `transfer_to_*` — **the answer returns to you**."
                }
                (false, Language::En) => {
                    "When you need another agent's help, call an `ask_*` tool. \
                     **The answer comes back to you**; weave it into your own words. \
                     **To ask several agents at once, call each `ask_*` in the same \
                     response.**\n\
                     **You cannot hand the conversation over to another agent.** \
                     The final answer is yours to give."
                }
            };
            let peers = self
                .roster()
                .iter()
                .map(|name| format!("- {name}"))
                .collect::<Vec<_>>()
                .join("\n");
            match language {
                Language::Ja => format!(
                    "## この場に居る相手\n\
                     {peers}\n\
                     いずれも**自分で考えて発言する別のエージェント**です。あなたが\
                     彼らの発言を書くことはありません。\n\n\
                     ## 会話の進め方\n\
                     まず、届いた発話の送り手を見てください。**あなたに話しかけてきた相手へ、\
                     あなた自身の言葉で答えるのが基本です。**\n\
                     {delegation}\n\
                     自分の応答で用が足りる場合、または相手の発言に返すべきことが残っていない場合は、\
                     **ツールを呼ばずに本文だけを返してください**。{ending}\
                     同じ内容を繰り返すくらいなら、会話を終えてください。\n\
                     委譲が失敗したときは、その**理由**（相手が停止中・時間切れ など）が\
                     結果の文字列で返ります。事前の点呼は不要です。"
                ),
                Language::En => format!(
                    "## Who is here\n\
                     {peers}\n\
                     Each of them is **a separate agent who thinks and speaks for \
                     themselves**. You never write their lines.\n\n\
                     ## How the conversation works\n\
                     First, look at who sent the incoming message. **Answering the one \
                     who spoke to you, in your own words, is the default.**\n\
                     {delegation}\n\
                     If your own response is enough, or there is nothing left to answer, \
                     **return plain text without calling any tool**. {ending} \
                     Rather than repeat yourself, end the conversation.\n\
                     When a delegation fails, the **reason** (the agent is stopped, it \
                     timed out, and so on) comes back in the result string. No roll \
                     call is needed beforehand."
                ),
            }
        } else {
            match language {
                Language::Ja => format!(
                    "## 会話の進め方\n\
                     応答は次のエージェントへ渡されます。会話を終えてよいと判断したら、\
                     本文の末尾に {TERMINATION_MARKER} と書いてください。その時点で会話は終わり、\
                     結果が人間へ返ります。同じ内容を繰り返すくらいなら、会話を終えてください。"
                ),
                Language::En => format!(
                    "## How the conversation works\n\
                     Your response is passed to the next agent. When you judge the \
                     conversation can end, write {TERMINATION_MARKER} at the end of \
                     your message. At that point the conversation ends and the result \
                     goes to the human. Rather than repeat yourself, end the \
                     conversation."
                ),
            }
        }
    }

    /// 並列委譲ツールの名前（Spec 04）。
    pub(super) const PLAN: &'static str = "plan";

    /// `plan` を提示するか。
    ///
    /// **接続先 2 体以上のときだけ。** 「進行役フラグ」のような設定は足さない —
    /// トポロジーがそのまま「進行役かどうか」を決める。1 体しか繋がっていない
    /// エージェントには `ask_*` で足りるので、使えない選択肢のスキーマを
    /// 毎ターンの固定費として払わせない。
    pub(super) fn offers_plan(&self) -> bool {
        self.entries.len() >= 2
    }

    /// 並列委譲（`plan`）のツール定義。
    ///
    /// `ask_*` との違いは**並列性と合流**だけ。`ask_*` は 1 体ずつ待つので
    /// 壁時計が人数倍になり、`transfer_to_*` の fan-out は並列だが答えが
    /// ユーザーへ散って戻ってこない。その中間が無かった。
    ///
    /// **宛先は `enum` で閉じる。** 自由文字列にすると、`build()` が
    /// 「ツール名は ID、説明は表示名」で解いた問題を作り直すことになる。
    /// 表示名の一意性はどこも保証していない（`World::register_agent` が
    /// 拒否するのは ID の重複だけ）ので、名前で指させると同名の 2 体を
    /// 区別できない。
    pub(super) fn plan_specs(&self, language: Language) -> Vec<ToolSpec> {
        if !self.offers_plan() {
            return Vec::new();
        }

        let ids: Vec<&str> = self
            .entries
            .iter()
            .map(|(_, target, _)| target.as_str())
            .collect();
        // ID と表示名の対応表。会話は表示名で流れるので、これが無いと
        // モデルは「ザリ・ロブステル」と `agent_2` を結び付けられない。
        let roster = self
            .entries
            .iter()
            .map(|(_, target, display)| format!("{target} = {display}"))
            .collect::<Vec<_>>()
            .join(" / ");

        vec![ToolSpec {
            name: Self::PLAN.to_owned(),
            description: match language {
                Language::Ja => format!(
                    "複数の相手へ**並列に**頼んで、全員の答えを束ねて受け取る。\
                     相手ごとに依頼内容を変えられる。\
                     1 体ずつ順に尋ねる `ask_*` と違い、全員が同時に動くので速い。\
                     独立した調べもの・作業を配るときはこれを使うこと。\
                     次の波を出す前に、前の束ねは**自分の言葉で要約**してから頼むこと\
                     （束ね全文を引きずると入力が波のたびに膨らむ）。\
                     「会話を渡した」と返ったタスクは**リトライしないこと** — \
                     仕事は別の経路で続いており、頼み直すと同じ仕事が二重に走る。\
                     依頼先: {roster}"
                ),
                Language::En => format!(
                    "Ask several agents **in parallel** and receive all their answers \
                     bundled together. Each agent can get a different request. Unlike \
                     `ask_*`, which waits for one agent at a time, everyone works at \
                     once, so it is fast. Use this to spread independent research or \
                     tasks. Before sending the next wave, **summarize the previous \
                     bundle in your own words** (dragging the full bundle along makes \
                     the input grow with every wave). If a task returns \"the \
                     conversation was handed over\", **do not retry it** — the work \
                     continues on another path, and asking again runs the same job \
                     twice. Recipients: {roster}"
                ),
            },
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "tasks": {
                        "type": "array",
                        "description": language.pick(
                            "同時に頼む依頼の一覧。同じ相手を 2 回入れないこと",
                            "Requests to send at the same time. Never list the same agent twice"),
                        "items": {
                            "type": "object",
                            "properties": {
                                "to": {
                                    "type": "string",
                                    "enum": ids,
                                    "description": match language {
                                        Language::Ja => format!("依頼先。{roster}"),
                                        Language::En => format!("Recipient. {roster}"),
                                    },
                                },
                                "message": {
                                    "type": "string",
                                    "description": language.pick(
                                        "その相手への依頼内容",
                                        "The request for that agent")
                                }
                            },
                            "required": ["to", "message"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["tasks"],
                "additionalProperties": false
            }),
        }]
    }

    /// この波の宛先として妥当か（実行時のトポロジーで見る）。
    ///
    /// 提示はターンの開始時、検証は実行時。`set_connections` は稼働中に
    /// 呼べるので、この 2 点の間に繋ぎ替えが起こりうる。
    fn is_target(&self, id: &AgentId) -> bool {
        self.entries.iter().any(|(_, target, _)| target == id)
    }

    /// 宛先の表示名。束ねの見出しに使う。
    fn display_of(&self, id: &AgentId) -> Option<&str> {
        self.entries
            .iter()
            .find(|(_, target, _)| target == id)
            .map(|(_, _, display)| display.as_str())
    }

    /// 名前からツールを逆引きする。
    fn resolve(&self, name: &str) -> Option<&AgentId> {
        self.entries
            .iter()
            .find(|(tool, _, _)| tool == name)
            .map(|(_, target, _)| target)
    }

    /// 最初の転送先。ツールを使えない経路の退避先。
    fn first(&self) -> Option<&AgentId> {
        self.entries.first().map(|(_, target, _)| target)
    }

    /// 応答から行き先を決める。
    ///
    /// 規則は OpenAI Agents SDK と同じ:
    /// **ツール呼び出しの無いテキスト出力が最終出力**。
    ///
    /// 転送要求は**全部**拾う（fan-out）。Claude / Gemini は 1 応答で複数の
    /// tool call を普通に返すので、最初の 1 本で打ち切ると残りが黙って消える。
    /// 同じ宛先への重複は先勝ちで 1 通に畳む——モデルは同じツールを
    /// 同じ引数で 2 回呼ぶことがあり、素通しにすると受け手の履歴が汚れる。
    /// `tools_available` は**そもそもツールを使える個体か**、
    /// `offer_transfer` は**転送を提示しているか**。**2 つは別の概念**で、
    /// 1 つの引数に畳んではいけない。
    ///
    /// 畳むと、転送を切った**ツール対応の個体**が下の旧経路
    /// （ツール非対応モデル向けの、終了マーカーが無ければ最初の相手へ渡す形）へ
    /// 落ちる。実機では `ask_*` を呼んだ周の空の本文が最初の接続先へ配送され、
    /// **依頼した相手ではない個体が空の依頼を受け取った**（2026-08-11）。
    pub(super) fn decide(&self, response: &ChatResponse, tools_available: bool, offer_transfer: bool) -> Outcome {
        let text = response.text.clone().unwrap_or_default();

        if tools_available {
            // 転送を出していない個体は**転送で抜けない**。委譲（`ask_*`）と
            // `plan` はツール実行の側で回るので、ここは常に終了でよい。
            if !offer_transfer {
                return Outcome::Finish { content: text };
            }
            let mut deliveries: Vec<(AgentId, String)> = Vec::new();
            for call in &response.tool_calls {
                let Some(target) = self.resolve(&call.name) else {
                    continue;
                };
                if deliveries.iter().any(|(to, _)| to == target) {
                    continue;
                }
                // 引数が欠けていても転送自体は成立させる。本文を代わりに渡す。
                let message = call
                    .args
                    .get("message")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
                    .unwrap_or_else(|| text.clone());
                deliveries.push((target.clone(), message));
            }
            if !deliveries.is_empty() {
                return Outcome::Handoff { deliveries };
            }
            return Outcome::Finish { content: text };
        }

        // ツールを使えない経路: 終了マーカーが無ければ最初の相手へ渡す。
        // 宛先を選ぶ手段が本文しか無いこの経路では、fan-out は表現できない。
        match (self.first(), text.contains(TERMINATION_MARKER)) {
            (Some(target), false) => Outcome::Handoff {
                deliveries: vec![(target.clone(), text)],
            },
            _ => Outcome::Finish {
                content: text.replace(TERMINATION_MARKER, "").trim_end().to_owned(),
            },
        }
    }
}
