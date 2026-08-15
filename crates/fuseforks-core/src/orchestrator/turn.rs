//! ターンの実行（段 1-8）。プロンプトを組み、ツールを回し、答えを配る。
//!
//! **段ごとの関数に割ってある**（2026-08-11）。1,206 行の 1 関数だった頃は、
//! git の hunk context がほぼ全部 `async fn handle_message` になり、
//! **差分から「どの段を変えたか」が読めなかった**。95 コミット中 55 が
//! この関数に入っていたので、そこが最も効く分割点だった。
//!
//!   handle_message   段の並びだけ（骨格）
//!   build_prompt     段 4 — モデルへ何を見せるか
//!   present_tools    段 5 — 何を選べるか
//!   run_turn         段 6-7 — 実行ループと統計
//!   dispatch_outcome 段 8 — どこへ返すか
//!
//! 委譲・転送そのもの（`ask_*` / `plan` / `transfer_to_*` の提示と判定）は
//! [`super::delegation`] にある。

use super::*;

/// 失敗したターンの**受信側だけ**を履歴へ残す。
///
/// # なぜ要るか（実機で観測、2026-07-31）
///
/// 履歴への書き込みは [`handle_message`] の終盤（統計と同じ節）にあり、
/// 途中の `?` で抜けると**受け取った依頼ごと履歴に残らない**。一方で
/// 広場ログはユーザー発の発話を対象外にし、自分宛も `is_mine` で除外する
/// （それらは履歴にある、という前提で組まれている）。両者の前提が噛み合わず、
/// **失敗したターンの依頼はどのプロンプト経路にも載らない**状態になっていた。
///
/// 実害: 出力上限で 1 ターン落ちた直後、進行役が「直前に何を頼まれたか」を
/// 完全に失い、他のエージェントへ聞いて回った（相手も知らない）。会話ログには
/// 残って画面には見えているので、利用者からは「なぜ忘れたのか」が分からない。
///
/// この repo には既に対になる原則がある — hop 打ち切りの「記録してから打ち切る」と
/// `reset_rule` の「発話は起きた事実でありログに残す」。失敗経路だけが外れていた。
///
/// # 何を積むか
///
/// 受信側は成功時と**同じ封筒**（[`attribute_sender`]）で積む。応答側は実際に
/// 何も言えていないので、失敗した事実を目印として置く — 往復の対を崩すと
/// 役割の交互性が壊れ、プロバイダによっては 400 で拒否される（failures.md #29）。
/// ツール結果は積まない。依頼文さえ残れば「何を頼まれたか」は復元でき、
/// 途中経過まで抱えるのは別の判断（履歴の肥大と引き換えになる）。
pub(super) async fn record_failed_turn(
    shared: &Arc<Shared>,
    agent_id: &AgentId,
    incoming: &AgentMessage,
    error: &CoreError,
) {
    let attributed = attribute_sender(shared, incoming).await;
    let note = format!(
        "（このターンは失敗し、返答できませんでした: {error}。\
         依頼は未処理のまま残っています）"
    );

    let mut world = shared.world.write().await;
    shared.push_exchange(&mut world, agent_id, &attributed, &note);
}

/// ターンの**飛行中台帳** — 始まった時点で作り、**どの出口でも**清算する。
///
/// 元は打ち切りの 2 出口へ消費量を束ねて渡すだけの型だったが、`failures.md` #103
/// で**4 つ目の出口（`Err`）だけが清算されていない**ことが分かり、ターンの
/// 頭から終わりまで生きる台帳へ広げた。形は otari（mozilla-ai）の
/// `inflight` 登録簿と同じ発想 — 「使用量の記録は過去しか語れない」ので、
/// 要求が始まった瞬間に登録し、**終わり方に関わらず**精算する。ここでは
/// 台帳が `run_turn` の外側（[`run_turn`]）に住み、内側（[`run_turn_inner`]）が
/// `&mut` で加算するので、`?` で内側から抜けても台帳は外側の手元に残る。
///
/// 出口は 4 つで、**それぞれ 1 行の使用量の行を書く**:
///
/// | 出口 | 行 |
/// |---|---|
/// | 完走 | `turn: … stop=-|repeat:x|tool_limit` |
/// | 失敗（`Err`） | `turn: … stop=failed:{CODE}`（**この行が #103 で無かった**） |
/// | 割り込み | `turn interrupted: … prompt= cached= total=` |
/// | 予算 | `turn budget exhausted: … prompt= cached= total=` |
///
/// 個別引数にしないのは、u64 が 3 つ並ぶと呼び出し側の取り違えが
/// コンパイルを通ってしまうため。`Copy` なのは、打ち切りの出口が値で受け取る
/// 一方で台帳そのものは外側に残す必要があるから（写しを渡し、原本を持ち続ける）。
#[derive(Clone, Copy, Default)]
struct TurnSpend {
    /// 累計トークン（入力 + 出力）。
    tokens: u64,
    /// キャッシュから読んだ入力トークン。
    cached: u64,
    /// 入力トークン。
    prompt: u64,
    /// 出力のうち思考に使われたぶん。**`tokens` の内数**（Spec 32 D2）。
    reasoning: u64,
    /// 実行ループの LLM 呼び出しの周回数（上限 `max_tool_iterations` との比較に
    /// 使うので、**まとめ呼び出しは数えない** — 元の `llm_rounds` と同じ）。
    /// **払ったと分かる失敗の周も数える** — 切れた応答も 1 往復として課金されている。
    rounds: u32,
    /// 受信した発話の hop。
    hop: u8,
}

impl TurnSpend {
    /// 1 呼び出しぶんの使用量を台帳へ足す。成功した応答も、払ったと分かる失敗
    /// （[`crate::llm::LlmError::usage`]）も**同じ入口**を通る — 経路を分けると
    /// 片方だけが既定値のまま化ける（#103 の形そのもの）。周回数は呼び出し側が
    /// 別に進める（まとめ呼び出しは使用量だけ足して周回数を進めないため）。
    fn absorb(&mut self, usage: &crate::llm::Usage) {
        self.tokens += usage.total();
        self.cached += usage.cache_read;
        self.prompt += usage.prompt;
        self.reasoning += usage.reasoning;
    }
}

/// 転送・委譲の提示条件（Spec 20 / 2026-08-11）。
///
/// **3 つを束ねたのは、5 箇所が同じ規律を読むから** — 手順の文（段 4）・
/// ツール提示（段 5）・応答の判定（段 6）。ここがずれると
/// 「出していないのに効く」か「出したのに効かない」になる。
/// 実機ではその両方を踏んでいる（`failures.md` #95 / #96）。
///
/// **`awaiting_reply` だけは個体の設定と独立**で、`reply_to` の有無という
/// 構造で決まる。設定で切れる `offer_transfer` と同じ型に住まわせているのは、
/// **読む側にとっては 1 つの答え**（転送を出すか）に畳まれるため。
#[derive(Clone, Copy)]
struct HandoffGates {
    /// 委譲（`ask_*`）と並列委譲（`plan`）を出すか。ツール非対応モデルと
    /// 接続先ゼロで偽になる。
    use_handoff_tools: bool,
    /// 転送（`transfer_to_*`）を出すか。
    offer_transfer: bool,
    /// 委譲で呼ばれたターンか。**手順の文で「答えがどこへ返るか」を分ける**
    /// のに要る（`offer_transfer` が偽になる理由が 2 つあり、書くべき文が違う）。
    awaiting_reply: bool,
}

/// ターンが生んだもの。実行ループの出口から [`dispatch_outcome`] へ渡す。
///
/// **4 つとも「1 ターンぶんの事実」**という一点で寿命が揃っている — 周ごとに
/// 上書きせず溜め、配送では**先頭の 1 通にだけ**載せる（fan-out で複製すると
/// 宛先数ぶん二重に数える）。引数で並べると 4 つが同じ寿命であることが
/// 呼び出し側からしか読めないので、束ねて型で示す。
struct TurnProduct {
    /// ループの抜け方。`Finish` なら本文、`Handoff` なら宛先ごとの配送。
    outcome: Outcome,
    /// このターンが使ったトークン（統計と発話に載る）。
    tokens: u64,
    /// 接地の来歴（Spec 05 / 31）。表示層へ渡すだけでプロンプトへは戻らない。
    grounding: crate::llm::Grounding,
    /// 思考の要約（Spec 33）。**履歴へは載らない** — 積む先の
    /// [`ChatMessage`] にこの欄が無く、型で閉じている。
    reasoning_summary: Vec<String>,
}

/// 割り込みで打ち切られたターンの出口（Spec 10 — 契約の出口 2a）。
///
/// 3 点セット: (a) 会話ログへ System の 1 行（要求から検知までの elapsed を
/// 含む — LLM 呼び出し中の切断を別 Spec で入れるかの判断材料）
/// (b) 履歴へ [`record_interrupted_turn`] の注記 (c) 依頼主が居れば
/// `Reply { kind: Interrupted }`。まとめの LLM 呼び出しは**しない**
/// （打ち切りの直後にもう 1 回課金しない — RepeatGuard の打ち切りと同じ判断）。
///
/// 打ち切りは失敗ではない（不変条件 4） — `AgentFailed` を出さず、
/// `last_error` にも書かず、ステータスは Running のまま。だからこの関数は
/// `Ok(())` を返す。ここまでに使ったトークンは実際に消費したので統計へ積む。
async fn finish_interrupted(
    shared: &Arc<Shared>,
    agent_id: &AgentId,
    reply_to: Option<tokio::sync::oneshot::Sender<Reply>>,
    turn: &TurnHandle,
    sent_user_turn: &str,
    spend: TurnSpend,
) -> CoreResult<()> {
    // None = 自分への interrupt_turn ではなく、依頼元の打ち切りが子トークン
    // 経由で連鎖した（Phase 2）。そのとき「要求から 0.0 秒」と書くと、
    // 検知が一瞬だったという嘘の計測値になる — 計測が無いことを言葉で言う。
    let elapsed = turn
        .requested_at
        .lock()
        .expect("await を跨がない")
        .map(|at| at.elapsed());
    // System 行は記録時の言語で書く（Spec 35 D6）。cause は行の中へ埋まるので
    // ここで一緒に分岐する（言語をまたいで埋め込むと語順が壊れる — Spec 13 P3b）。
    let language = shared
        .world
        .read()
        .await
        .language()
        .unwrap_or(crate::world::Language::Ja);
    let cause = match (elapsed, language) {
        (Some(elapsed), crate::world::Language::Ja) => {
            format!("要求から {:.1} 秒", elapsed.as_secs_f64())
        }
        (None, crate::world::Language::Ja) => "依頼元の打ち切りに連鎖".to_owned(),
        (Some(elapsed), crate::world::Language::En) => {
            format!("{:.1}s after the request", elapsed.as_secs_f64())
        }
        (None, crate::world::Language::En) => "cascaded from the requester's interrupt".to_owned(),
    };

    // (b) 履歴 + 統計。1 回の world ロックで済ませる。
    record_interrupted_turn(shared, agent_id, sent_user_turn, &spend).await;

    // (a) 会話ログへ System の 1 行。表示名は「切られた本人」— System 行は
    // 全員の会話ペインに出るので、誰のターンかを名指ししないと読めない。
    let display = {
        let world = shared.world.read().await;
        world
            .agent(agent_id)
            .map(|record| record.spec.name.clone())
            .unwrap_or_else(|_| agent_id.to_string())
    };
    let interrupt_text = match language {
        crate::world::Language::Ja => {
            format!("{agent_id}（{display}）のターンをユーザーの指示で打ち切りました（{cause}）")
        }
        crate::world::Language::En => {
            format!("Interrupted {agent_id} ({display})'s turn at the user's request ({cause})")
        }
    };
    shared
        .record(AgentMessage::new(
            Endpoint::System,
            Endpoint::User,
            interrupt_text,
            0,
        ))
        .await;

    // 出口の行はここ（切られたターン自身）だけが書く。割り込んだ側は書かない —
    // 二重割り込み・interrupt_all・親トークン経由が重なっても 1 本になる。
    shared.emit(CoreEvent::TurnInterrupted {
        agent_id: agent_id.clone(),
        turn_seq: turn.seq,
    });

    // (c) 依頼主への返信。文言は契約（P3）の固定文。受け取り手が既に
    // 諦めている（タイムアウト・親も打ち切り済み）ことはあるので送信の失敗は
    // 無視する — 「drop は実装バグ」の射程はワーカーが送らないことであって、
    // 確定済みの親が受け取らないことではない。
    if let Some(reply_to) = reply_to {
        let _ = reply_to.send(Reply {
            text: "この依頼はユーザーの指示で打ち切られました。答えはありません。".to_owned(),
            kind: PlanTaskState::Interrupted,
        });
    }

    note!(
        "turn interrupted: agent={agent_id} seq={} hop={} rounds={} elapsed_ms={} \
         prompt={} cached={} total={}",
        turn.seq,
        spend.hop,
        spend.rounds,
        // 連鎖（None）は -1。0 と区別する — 0 は「即検知」という実測値。
        elapsed.map_or(-1, |e| i128::try_from(e.as_millis()).unwrap_or(i128::MAX)),
        spend.prompt,
        spend.cached,
        spend.tokens,
    );
    Ok(())
}

/// 打ち切られたターンの受信側を履歴へ残す（Spec 10 — 出口 2a の (b)）。
///
/// [`record_failed_turn`] と**文言を共有しない** — 失敗の文言を使い回すと、
/// 次のターンの自分が「エラーが起きた」と誤読する。起きたのは指示による
/// 打ち切りで、依頼そのものは健在。
///
/// 受信側は `sent_user_turn`（実際に送った形）をそのまま積む。`attributed`
/// だけに縮めると送信と保存が食い違い、その位置で前方一致が切れる
/// （failures.md #45 — 打ち切りの検知点では組み立てが済んでいるので、
/// 失敗経路と違って送った形が手元にある。縮める理由が無い）。
async fn record_interrupted_turn(
    shared: &Arc<Shared>,
    agent_id: &AgentId,
    sent_user_turn: &str,
    spend: &TurnSpend,
) {
    let note = "（このターンはユーザーの指示で打ち切られました。\
                依頼は未処理のまま残っています）";

    let mut world = shared.world.write().await;
    if let Ok(record) = world.agent_mut(agent_id) {
        record.total_tokens += spend.tokens;
        record.cached_tokens += spend.cached;
        record.prompt_tokens += spend.prompt;
    }
    shared.push_exchange(&mut world, agent_id, sent_user_turn, note);
}

/// トークン予算で打ち切られたターンの出口（Spec 11 — `token_budget` の
/// exhaustion）。[`finish_interrupted`]（Spec 10 の出口 2a）と同じ 3 点セットの
/// 形だが、資源の事実なので文言と分類が違う:
/// (a) 会話ログへ System の 1 行 — ただし**因果全体で 1 回だけ**
/// （`note_exhausted` の CAS が初回観測を決める。波の 6 体が同時に尽きても
/// 通知は 6 行にならない） (b) 履歴へ注記 (c) 依頼主が居れば
/// `Reply { kind: BudgetExhausted }`。まとめの LLM 呼び出しはしない —
/// 尽きたら新しい呼び出しを始めない、が契約そのもの。
/// 稼働は降ろさない（閉じるのはターンだけ）。次の依頼は新しい予算で普通に走る。
async fn finish_budget_exhausted(
    shared: &Arc<Shared>,
    agent_id: &AgentId,
    reply_to: Option<tokio::sync::oneshot::Sender<Reply>>,
    pool: &Arc<BudgetPool>,
    sent_user_turn: &str,
    spend: TurnSpend,
) -> CoreResult<()> {
    // (b) 履歴 + 統計。ここまでに使ったトークンは実際に消費したので積む。
    {
        let note = "（このターンはトークン予算の上限で打ち切られました。\
                    依頼は未処理のまま残っています）";
        let mut world = shared.world.write().await;
        if let Ok(record) = world.agent_mut(agent_id) {
            record.total_tokens += spend.tokens;
            record.cached_tokens += spend.cached;
            record.prompt_tokens += spend.prompt;
        }
        shared.push_exchange(&mut world, agent_id, sent_user_turn, note);
    }

    // (a) System の 1 行。文言は契約の固定文（事実 + 次の道 — #44 の規律）。
    // 書くのは因果で最初に尽きを観測したターンだけ。
    if pool.note_exhausted() {
        let display = {
            let world = shared.world.read().await;
            world
                .agent(agent_id)
                .map(|record| record.spec.name.clone())
                .unwrap_or_else(|_| agent_id.to_string())
        };
        // System 行は記録時の言語で書く（Spec 35 D6）。
        let language = shared
            .world
            .read()
            .await
            .language()
            .unwrap_or(crate::world::Language::Ja);
        // **「使い切った」とは言わない**（Spec 38 D3）。予約は次の 1 呼び出しぶんを
        // 先に確保する形なので、**実費を使い切る前に止まる**ことがある
        // （過大予約による早止まり）。起きた事実は「次のぶんを確保できなかった」で、
        // どちらの場合も正しい。残額の内訳は `budget stop:` の計器が持つ。
        let budget_text = match language {
            crate::world::Language::Ja => format!(
                "次の呼び出しぶんの予算（上限は実効 {} トークン）を確保できなかったため、\
                 {agent_id}（{display}）のターンを打ち切りました。\
                 続きが要るなら改めて依頼してください\
                 （予算は依頼ごとに新しく付きます）",
                pool.ceiling_effective()
            ),
            crate::world::Language::En => format!(
                "Could not reserve the budget for the next call (the ceiling is {} \
                 effective tokens), so {agent_id} ({display})'s turn was cut off. \
                 If you need more, send a new request — each request gets a fresh \
                 budget",
                pool.ceiling_effective()
            ),
        };
        shared
            .record(AgentMessage::new(
                Endpoint::System,
                Endpoint::User,
                budget_text,
                0,
            ))
            .await;
    }

    // (c) 依頼主への返信。送信の失敗は無視する（相手が先に確定していることは
    // ある — race の勝敗は分類を変えない）。
    if let Some(reply_to) = reply_to {
        let _ = reply_to.send(Reply {
            text: "この依頼はトークン予算の上限で打ち切られました。答えはありません。"
                .to_owned(),
            kind: PlanTaskState::BudgetExhausted,
        });
    }

    note!(
        "turn budget exhausted: agent={agent_id} hop={} rounds={} ceiling={} spent={} \
         prompt={} cached={} total={}",
        spend.hop,
        spend.rounds,
        pool.ceiling_effective(),
        pool.spent_effective(),
        spend.prompt,
        spend.cached,
        spend.tokens,
    );
    Ok(())
}

/// プロンプトキャッシュの診断行を 1 周ごとに残す。
///
/// # なぜ率ではなく生の数字が要るか
///
/// カードの「入力の N% をキャッシュ」だけでは、**0% の理由が三つ巴**になる —
/// (a) プロバイダの最小長を下回った (b) 前方一致が壊れた (c) プロバイダが
/// 値を返していない。この 3 つは処方が全部違うのに、画面上は同じ 0% に見える。
///
/// # 累積では判別できない
///
/// カードが持つ `promptTokens` は**ターンをまたいだ累積**なので、閾値との
/// 比較に使えない — 1 周 1,000 トークンのエージェントでも 5 周喋れば 5,000 に
/// なり、「閾値を超えているのに 0%」と誤読される。判定には**その周の値**が要る。
///
/// # ハッシュは system プロンプト全文に掛ける
///
/// 安定部分（`stable_len` まで）だけに掛けると、顔ぶれや Memory が変わって
/// 前方一致が切れた場合 (b) を「変わっていない」と表示してしまう。会話が
/// キャッシュに載るには **systemInstruction 全体**がバイト一致している必要がある。
///
/// ハッシュ値はプロセスをまたいで比較しない（`DefaultHasher` の値は Rust の
/// 版に依存する）。見るのは**同じセッション内で周ごとに変わったかどうか**だけ。
fn note_cache_diag(
    agent_id: &AgentId,
    model: &str,
    round: u32,
    usage: &crate::llm::Usage,
    system: SystemDigest,
    history: HistoryDepth,
) {
    note!(
        "cache: agent={agent_id} model={model} round={round} \
         prompt={} cached={} system_chars={} stable_chars={} system_blocks={} \
         history_msgs={}/{} system_digest={:016x}",
        usage.prompt,
        usage.cache_read,
        system.chars,
        system.stable_chars,
        system.blocks,
        history.msgs,
        history.limit,
        system.digest,
    );
}

/// 履歴の通数と上限。**`history_msgs` が `limit` に張り付いていたら窓が滑っている**
/// = 毎ターン先頭の 1 往復が落ち、前方一致は system の直後で切れる。
#[derive(Debug, Clone, Copy)]
struct HistoryDepth {
    msgs: usize,
    limit: usize,
}

/// プロバイダへ実際に渡る system 面の指紋。
///
/// # 数えるのは「連結後」でなければならない
///
/// adapter は `Role::System` のメッセージを**配列のどこにあっても全部引き抜いて**
/// 1 つの `system` / `systemInstruction` へ連結する（`gemini.rs` / `anthropic.rs` の
/// `encode`）。したがって「system プロンプト 1 本」だけを数えても、実際に前方一致の
/// 先頭を占める文字列とは別物になる。
///
/// 初版はまさにそれを数えており、可変ブロック（参照資料・広場ログ・入退室）が
/// 毎ターン変わっていても digest は動かなかった。**その計装で「前方一致は壊れて
/// いない」と読んだのは誤診**で、検出不能だっただけ（failures.md #45）。
#[derive(Debug, Clone, Copy)]
struct SystemDigest {
    /// 連結後の文字数。
    chars: usize,
    /// 安定部分の文字数（`cacheable_prefix_len` と同じ値）。
    stable_chars: usize,
    /// system ブロックの本数。2 本を超えていたら可変ブロックが混ざっている。
    blocks: usize,
    /// 連結後の全文のハッシュ。
    digest: u64,
}

impl SystemDigest {
    /// adapter と**同じ畳み方**で数える。ここがズレると指紋の意味が消える。
    fn of(messages: &[ChatMessage], stable_len: usize) -> Self {
        use std::hash::{Hash, Hasher};

        let blocks: Vec<&str> = messages
            .iter()
            .filter(|m| m.role == Role::System)
            .map(|m| m.content.as_str())
            .collect();
        let joined = blocks.join("\n\n");

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        joined.hash(&mut hasher);
        Self {
            chars: joined.chars().count(),
            stable_chars: stable_len,
            blocks: blocks.len(),
            digest: hasher.finish(),
        }
    }
}

/// 同じ結果が何回返ったら次を実行しないか（failures.md #41 の処方 1）。
///
/// 2 = 「同じ呼び出しに同じ結果が 2 回返ったら、3 回目は実行しない」。
const REPEAT_BLOCK_AFTER: u32 = 2;

/// 1 つの呼び出し（ツール名 + 引数）について、ターン内で最後に見た結果。
#[derive(Debug)]
struct SeenCall {
    /// ツール名。
    name: String,
    /// 引数。等価判定は `serde_json::Value` の中身で行う（キーの並びに依存しない）。
    args: serde_json::Value,
    /// 直近にこの呼び出しが返した本文。
    body: String,
    /// `body` が**変わらないまま**返ってきた回数。
    count: u32,
}

/// 同一のツール呼び出しの繰り返しを検出する（failures.md #41 の処方 1）。
///
/// # 判定材料に結果**本文**を使う理由
///
/// 台帳の処方は「ツール名 + 引数 + エラー文言」だが、同梱ツールは失敗を
/// `Err` ではなく **`Ok(<エラー文の本文>)`** で返す（「ツールの失敗は会話を
/// 止めない」という既存規律の帰結。`file` / `fd` / `grep` / `sd` すべてこの形）。
/// したがって `Result::is_err` で失敗を数えると、実機で燃えた経路
/// （`sd` の失敗を 12 周繰り返した failures.md #39）は 1 件も検出できない。
/// 失敗が型に載っていない以上、**モデルへ返る本文の完全一致**が失敗の
/// 一致を表す唯一の実体になる。文言を parse して失敗かどうかを推定するのは
/// やらない（Spec 08 で「分類は文言 parse でなく型で運ぶ」と決めた側の話）。
///
/// 成功の繰り返しも同じ扱いで止まる。同じ入力に同じ出力が返っている以上、
/// 3 回目に新しい情報は無い。
///
/// # 数えるのは「呼び出しごと」であって「隣接」ではない
///
/// 当初は直前の 1 件とだけ比べていた（隣接する 2 回で判定）。**実機では 1 件も
/// 発火しなかった** — モデルは 1 周に 2〜3 本を並列で呼ぶので、同じ読み直しは
/// 周をまたいで現れ、間に別の呼び出しが挟まって数えが切れる。実測（2026-07-31）:
///
/// ```text
/// round 24 file(A) → 12054 字   round 2 file(B) → 12045 字
/// round 25 file(A) → 12054 字   round 3 file(B) → 12045 字 + file(C) + file(D)
/// round 26 grep    → 別物       round 5 file(B) → 12045 字
/// round 28 file(A) → 12054 字（3 回目。隣接判定では素通し）
/// ```
///
/// そこで **(ツール名 + 引数) ごとに独立して数える**。一致の条件は
/// 完全一致のままで、「隣り合っているか」の要求だけを外した。
/// 同じ呼び出しが**違う結果**を返したら、そこで数え直す（追記が進む・待っていた
/// 状態が変わる、のように同じ操作が実を結んでいる場合は繰り返しではない）。
///
/// # 止めるのはループではなく、その 1 本
///
/// 3 回目を実行しないだけで、ターンのツールループは続ける。並列の 1 本が
/// 重複しただけで、進行中の作業まで殺さない。**その周のツールが全部
/// ブロックされたとき**（= 新しいことを何もしていない周）だけ打ち切る。
#[derive(Debug, Default)]
struct RepeatGuard {
    /// ターン内で見た呼び出し。(name, args) につき 1 件。
    ///
    /// `Vec` なのは `serde_json::Value` が `Hash` を実装しないため。
    /// 1 ターンの相異なる呼び出しは高々数十件で、線形走査で足りる。
    seen: Vec<SeenCall>,
}

impl RepeatGuard {
    /// この呼び出しを実行せずに止めるか。**実行の前**に引く。
    ///
    /// 結果はまだ無いので、ここで見られるのはツール名と引数だけ。
    /// 「同じ引数で同じ結果が [`REPEAT_BLOCK_AFTER`] 回返った呼び出しが、
    /// また同じ引数で来た」ときに真を返す。
    fn blocks(&self, name: &str, args: &serde_json::Value) -> bool {
        self.repeats(name, args) >= REPEAT_BLOCK_AFTER
    }

    /// この呼び出しに同じ結果が返った回数。打ち切りの通知に載せる。
    fn repeats(&self, name: &str, args: &serde_json::Value) -> u32 {
        self.find(name, args).map_or(0, |seen| seen.count)
    }

    /// 実行した 1 件を記録する。**実行の後**に引く。
    fn observe(&mut self, name: &str, args: &serde_json::Value, body: &str) {
        match self.position(name, args) {
            Some(index) => {
                let seen = &mut self.seen[index];
                if seen.body == body {
                    seen.count += 1;
                } else {
                    // 結果が変わった = この呼び出しは行き詰まっていない。数え直す。
                    seen.body = body.to_owned();
                    seen.count = 1;
                }
            }
            None => self.seen.push(SeenCall {
                name: name.to_owned(),
                args: args.clone(),
                body: body.to_owned(),
                count: 1,
            }),
        }
    }

    fn find(&self, name: &str, args: &serde_json::Value) -> Option<&SeenCall> {
        self.seen
            .iter()
            .find(|seen| seen.name == name && &seen.args == args)
    }

    fn position(&self, name: &str, args: &serde_json::Value) -> Option<usize> {
        self.seen
            .iter()
            .position(|seen| seen.name == name && &seen.args == args)
    }
}

/// 受信した発話を 1 件処理する。
///
/// 手順: プロンプト組み立て → RAG 付与 → LLM 呼び出し → 統計更新 → 記録 → 転送。
///
/// **途中で失敗すると履歴は書かれない。** 呼び出し側（[`agent_loop`]）が
/// [`record_failed_turn`] で受信側だけを残す責任を持つ。
/// **ログ行に出す**宛先・送り手の表記（**1 実装**）。
///
/// `turn start:` / `reply:` / `handoff:` が同じ書式を共有する。別々に書くと、
/// 同じ発話の送り手と宛先が違う綴りで出て、grep で追えなくなる。
///
/// **画面用の [`endpoint_label`] とは別物。** あちらは表示名（人が読む名前）を
/// `World` から引くが、こちらは **id のまま**出す — ログは表示名の改名を
/// またいで grep できる必要がある（`fuseforks.log` の `name=` を識別子で
/// 残した判断と同じ。CLAUDE.md「識別子は `title` に残す」）。
fn endpoint_log_label(endpoint: &Endpoint) -> String {
    match endpoint {
        Endpoint::User => "user".to_owned(),
        Endpoint::Agent { id } => id.to_string(),
        Endpoint::System => "system".to_owned(),
        Endpoint::External { client } => format!("external:{client}"),
    }
}

pub(super) async fn handle_message(
    shared: &Arc<Shared>,
    agent_id: &AgentId,
    envelope: Envelope,
    turn: &TurnHandle,
    // 直前の 1 呼び出しの実効 milli（Spec 38 D1(b) の予約見積もり）。
    // **所有者は `agent_loop`** — ターンより長く、個体の稼働と同じだけ生きる。
    // `run_turn` に置くとターンごとに新品になり、各ターンの初回ラウンドが
    // 永久に床のままになる。`Shared` の表にしなかったのは、周回ごとに
    // ロックを取らずに済ませるため（契約 reservation の「ロックは足さない」）。
    last_call_milli: &Arc<std::sync::atomic::AtomicU64>,
) -> CoreResult<()> {
    let Envelope {
        incoming,
        mut reply_to,
        // 自ターンのトークンは agent_loop が子として導出済み（`turn.token`）。
        // ここで別々に見ると 2 本の検査になる — 1 本に畳むのが Phase 2 の核。
        cancel: _,
        // 因果の予算（Spec 11）。このターンの全消費をここから引き、
        // このターンが生む全配送（ask / plan / 転送）へ同じ Arc を渡す。
        budget,
        // 因果の参加者（Spec 28）。**このターンが `ask` / `plan` で待った相手**
        // だけがここへ入る。予算と同じ経路で下流へ渡すが、書き込むのは
        // `deliver_and_wait` が答えを受け取った瞬間だけ。
        participants,
    } = envelope;
    // ターンの開始を残す。**無音の起点が分からないと、飛行中と落ちた後を
    // 区別できない** — `tool:` 行はツールを呼んだ周にしか出ないので、
    // LLM の応答を待っている間はログが止まって見える（2026-07-31 に実際に
    // 詰まった。ツール 4 周目のあと 2 分無音で、生死が判定できなかった）。
    note!(
        "turn start: agent={agent_id} hop={} from={} chars={}",
        incoming.hop,
        endpoint_log_label(&incoming.from),
        incoming.content.chars().count(),
    );
    // 1. 定義とテンプレートを取り出す。ロックはここで手放し、LLM 呼び出しは持たずに行う。
    let (spec, template) = {
        let world = shared.world.read().await;
        let record = world.agent(agent_id)?;
        let template = world.template(&record.spec.model_template_id)?.clone();
        (record.spec.clone(), template)
    };

    // 2. システムプロンプトを組む。安定部分の長さも同時に得る（キャッシュ境界）。
    //    接地の有無はテンプレート由来（エージェント個別の設定ではない）。
    //    フラグではなく grounding_active() を見る — 互換経路のまま真になっている
    //    設定（world.json の直接編集で作れる）に「検索できます」と教えないため。
    //
    //    顔ぶれ（Spec 06 P1.5）はここで組む。順序はツール提示順（=
    //    connected_agents の保存順）と同一 — 顔ぶれだけ別の整列規則を持つと、
    //    同じ相手の並びが transfer_to_* と食い違い、モデルに二重管理を強いる。
    //    形式は agent_id（表示名）: 状態。id はモデルの宛先語彙（ツール名）で、
    //    無いと表示名 → id の対応をツール説明から二段引きすることになる。
    let roster: Option<String> = {
        let world = shared.world.read().await;
        // 顔ぶれの本文もモデルへ届く面（Spec 35 P3）。組み立て時の現在言語で書く。
        // 英語の腕は ASCII の括弧 — 全角の `（）［］` は英語の文中で浮く。
        let language = world.language().unwrap_or(crate::world::Language::Ja);
        let entries: Vec<String> = spec
            .connected_agents
            .iter()
            .map(|id| {
                world
                    .agent(id)
                    .map(|record| {
                        // 役職（Spec 14）。**名前だけ**を出す — 説明はプロンプトに
                        // 入れない（role_contract 凍結 6。顔ぶれは毎ターン・全員ぶんを
                        // 素の値段で払うので、名前 3〜5 トークンに対し説明 50〜200 は
                        // 以後の全ターンに乗る固定費になる）。
                        //
                        // **引けなければ `[...]` ごと省く**（凍結 5）。`[不明]` とは
                        // 書かない — 存在しない役は判断材料にならず、毎ターンぶんの
                        // トークンを払うだけになる。バッジ側（カード・地図）と
                        // 同じ規則で、3 箇所の扱いを揃えてある。
                        match language {
                            crate::world::Language::Ja => {
                                let role = world
                                    .role_label(record.spec.role_id.as_ref())
                                    .map(|name| format!("［{name}］"))
                                    .unwrap_or_default();
                                format!(
                                    "{id}（{}）{role}: {}",
                                    record.spec.name,
                                    record.status.label()
                                )
                            }
                            crate::world::Language::En => {
                                let role = world
                                    .role_label(record.spec.role_id.as_ref())
                                    .map(|name| format!(" [{name}]"))
                                    .unwrap_or_default();
                                format!(
                                    "{id} ({}){role}: {}",
                                    record.spec.name,
                                    record.status.label_en()
                                )
                            }
                        }
                    })
                    // 接続先が消えていても行は成立させる（ID と不明で示す）。
                    .unwrap_or_else(|_| match language {
                        crate::world::Language::Ja => format!("{id}: 不明"),
                        crate::world::Language::En => format!("{id}: unknown"),
                    })
            })
            .collect();
        (!entries.is_empty()).then(|| entries.join(" / "))
    };
    // モデルへ届く面の言語（Spec 35）。初回に確定して再判定しないので、
    // ターンごとに読み直しても村の中では一定（切り替えた直後の 1 回だけ変わる）。
    let language = shared
        .world
        .read()
        .await
        .language()
        .unwrap_or(crate::world::Language::Ja);
    let (system_prompt, stable_len) = shared
        .store
        .compose_system_prompt(&spec, template.grounding_active(), roster.as_deref(), language)
        .await?;

    // 3. 転送先ごとのツールを組む。
    //    OpenAI Agents SDK は handoff を「宛先 1 つにつきツール 1 本」で表現し、
    //    `transfer_to_<agent>` という名前を使う。単一ツール + 宛先パラメータより、
    //    名前で選ばせるほうがモデルの学習分布に近い。
    //    宛先は ID だけでなく**表示名**も添えて渡す。会話は表示名で流れるので、
    //    名前と ID を結ぶ情報がプロンプトに無いと、モデルは誰に渡すか推測になる。
    let targets: Vec<(AgentId, String)> = {
        let world = shared.world.read().await;
        spec.connected_agents
            .iter()
            .map(|id| {
                let display = world
                    .agent(id)
                    .map(|record| record.spec.name.clone())
                    // 接続先が消えていても転送経路自体は壊さない。ID で示す。
                    .unwrap_or_else(|_| id.to_string());
                (id.clone(), display)
            })
            .collect()
    };
    let handoffs = HandoffTools::build(&targets);
    // **委譲（`ask` / `plan`）で呼ばれたターンか。** 真なら答えを待っている
    // 相手が居る（`reply_to` はその戻り口）。
    let awaiting_reply = reply_to.is_some();
    // 委譲（`ask_*`）と並列委譲（`plan`）を出す条件。**転送とは分ける。**
    let use_handoff_tools = template.use_tools && !handoffs.is_empty();
    // **転送（`transfer_to_*`）を提示するか。** 落ちる理由は 2 つある。
    //
    // 1. **個体の設定**（`allow_handoff`。既定は真）。分ける理由は 2 つの道具の
    //    **答えの行き先が逆**だから — 委譲は依頼主へ戻り、転送は利用者へ流れる。
    //    進行役が意図と違うほうを選ぶとオーケストレーションが成立しない。
    // 2. **委譲で呼ばれたターンでは、設定に関わらず提示しない。** 答えを待って
    //    いる口があるのに転送すると、その口には「答えは戻りません」の定型文が
    //    返り、中身は**別の因果**として宛先の受信箱へ積まれる。**1 つの依頼が
    //    2 本に分裂する。** 実機では依頼主自身へ転送された回があり
    //    （`handoff: agent=agent_3 to=agent`）、依頼主は空を読んで「答えが
    //    無かった」と報告し、その報告が済んだ 3 分後に同じ答えが新しい依頼と
    //    して届いて余分に 2 ターン走った。**モデルの意図は正しく、道具だけが
    //    違う** — 説明文では防げないので、選べなくする（#84 の一般化）。
    //
    // **`ask` と `plan` はどちらの理由でも落ちない** — 消すのは転送だけ。
    // 委譲で呼ばれた個体が、さらに別の個体へ `ask` して答えを束ねる経路は
    // 残る（囲いは `max_hops` とトークン天井が同じ因果に載っていること）。
    let gates = HandoffGates {
        use_handoff_tools,
        awaiting_reply,
        offer_transfer: use_handoff_tools && spec.allow_handoff && !awaiting_reply,
    };
    // 4. プロンプトを組む。組み立ての中身は build_prompt が持つ。
    let prompt = build_prompt(
        shared,
        agent_id,
        &spec,
        &incoming,
        system_prompt,
        stable_len,
        roster.is_some(),
        &handoffs,
        gates,
    )
    .await;

    // 5. ツールを提示する。組み立ての中身は present_tools が持つ。
    let tools = present_tools(shared, agent_id, &spec, &template, &handoffs, gates).await;
    // 6-7. 実行ループと統計。ここで打ち切り・予算切れなら後始末まで済んでいる。
    let Some(product) = run_turn(
        shared,
        agent_id,
        turn,
        &incoming,
        &mut reply_to,
        &budget,
        last_call_milli,
        &participants,
        &spec,
        &template,
        &handoffs,
        gates,
        prompt,
        tools,
        stable_len,
    )
    .await?
    else {
        return Ok(());
    };

    // 8. 記録と転送。
    dispatch_outcome(
        shared,
        agent_id,
        &incoming,
        reply_to,
        product,
        budget,
        participants,
    )
    .await
}



/// モデルへ渡すツール一式（段 5）。
///
/// **`specs` と `executable` は別物**。前者は提示集合（転送・委譲・`plan`・
/// `room_log` を含む）、後者は registry と個別 MCP で**実際に走らせられる**もの。
/// 合成側（`ask_*` / `plan` / `room_log`）は `executable` に居ないので、
/// 実行可否を `executable` だけで判定すると呼び出しが素通りして
/// **モデルが呼んだのに何も起きない**（エラーにならないので気づけない）。
/// 2 つを同じ型で返すのは、その差を読む側の目に入れるため。
struct PresentedTools {
    /// モデルへ提示する全部。
    specs: Vec<ToolSpec>,
    /// registry + 個別 MCP で実行できるもの（実行可否の判定に使う）。
    executable: Vec<ToolSpec>,
    /// ツールを送るか。空の提示集合とツール非対応モデルで偽。
    use_tools: bool,
}

/// ツールを提示する（段 5）。
///
/// **切り出したのは、ここが「何を選べるか」だけを決める段だから** —
/// 実行もしなければ応答も見ない。提示は個体別に解決する（`spec_for`）ので、
/// 名前の集合では書けない除外（`run` の許可リストが空、`rag` の宣言が空）が
/// ここに入る。
async fn present_tools(
    shared: &Arc<Shared>,
    agent_id: &AgentId,
    spec: &AgentSpec,
    template: &ModelTemplate,
    handoffs: &HandoffTools,
    gates: HandoffGates,
) -> PresentedTools {
    let HandoffGates {
        use_handoff_tools,
        offer_transfer,
        ..
    } = gates;
    // 5. ツールを提示する。転送用と実行用を 1 つの集合としてモデルへ渡す。
    //    モデルから見れば「次に何をするか」の選択肢はどちらも同じ粒度で、
    //    転送だけ別扱いにする理由が無い。区別するのはこちら側の役目。
    //    同梱ツールはエージェント個別の提示制御（enabled_tools + 作業フォルダ
    //    連動の自動除外）を通す — 使わないツールのスキーマは毎ターンの
    //    固定費になる（トークン節約は最重要課題）。
    // モデルへ届く文言の言語（Spec 35）。合成ツール・room_log・同梱ツールの
    // 提示がすべてこの 1 つの値を使う — ばらばらに読むと、切り替えの瞬間に
    // 半分だけ英語のプロンプトが組める。
    let language = shared
        .world
        .read()
        .await
        .language()
        .unwrap_or(crate::world::Language::Ja);
    let mut specs = if use_handoff_tools {
        // 転送は `offer_transfer` のときだけ。委譲と `plan` は常に載る。
        let mut both = if offer_transfer {
            handoffs.specs(language)
        } else {
            Vec::new()
        };
        both.extend(handoffs.ask_specs(language));
        // 並列委譲は接続先 2 体以上のときだけ載る（Spec 04）。
        // 1 体しか繋がっていないエージェントには使えない選択肢なので、
        // そのスキーマを毎ターンの固定費として払わせない。
        both.extend(handoffs.plan_specs(language));
        both
    } else {
        Vec::new()
    };
    // 広場ログの全文読み（Spec 22 — `room_log_pull` 契約）。提示は
    // hears_room_log ただ 1 点で決まる — 抜粋が届かない個体は ID を知る経路が
    // 無い。ログが空かどうかでは揺らさない（提示は静的・状態は動的）。
    // enabledTools の対象外（`ask_*` / `rag` と同じ側）。
    if spec.hears_room_log {
        specs.push(room_log_tool_spec(language));
    }
    // 提示は**個体別に解決する**。`spec_for` を持つツール（Spec 15 の `run`）は、
    // その個体から実行できる登録だけを列挙し、1 件も無ければ自分を落とす。
    // 名前の集合（`WORK_DIR_TOOL_NAMES`）では書けない除外がここに入る。
    let presentation_ctx = ToolContext {
        agent_id: agent_id.clone(),
        work_dir: spec.work_dir.clone().map(std::path::PathBuf::from),
        cancel: None,
        // 宣言フォルダ（Spec 18）。`rag` の spec_for が 2 段ゲートの 2 段目
        // （空または全滅なら提示しない）をここから判定する。
        rag_roots: spec.rag_sources.iter().map(std::path::PathBuf::from).collect(),
        language,
    };
    let shared_specs: Vec<ToolSpec> = shared
        .tools
        .read()
        .await
        .specs_for(&presentation_ctx)
        .await
        .into_iter()
        .filter(|tool| is_bundled_tool_presented(&tool.name, spec))
        .collect();
    // エージェント別 MCP のツールを重ねる（ツール収集の最終形）。
    // 同名は個別が勝つ — 共通と同じサーバーを自分専用の接続先で
    // 置き換える正当な手段（上書き可能な加算）。
    let personal_specs: Vec<ToolSpec> = {
        let map = shared.agent_mcp.read().await;
        map.get(agent_id)
            .map(|state| {
                state
                    .manager
                    .tools()
                    .iter()
                    .map(|tool| tool.spec(presentation_ctx.language))
                    .collect()
            })
            .unwrap_or_default()
    };
    let executable = merge_tool_specs(shared_specs, personal_specs);
    specs.extend(executable.iter().cloned());
    let use_tools = !specs.is_empty() && template.use_tools;
    PresentedTools {
        specs,
        executable,
        use_tools,
    }
}


/// ターンを走らせる（段 6-7）。**ここが `handle_message` の核**。
///
/// 戻り値の `None` は「打ち切り・予算切れで、後始末まで済んでいる」。
/// 出口 3 点セット（会話ログ・履歴・依頼主への `Reply`）はその中で完了して
/// いるので、呼び出し側は `Ok(())` で降りるだけでよい。
///
/// 返す [`TurnProduct`] は**そのまま段 8 の入力**。中で溜める 4 つ
/// （outcome / tokens / grounding / reasoning_summary）は 1 ターンぶんの寿命で、
/// 周ごとに上書きすると先に起きた接地や要約が消える。
///
/// # 飛行中台帳（#103）
///
/// この関数は**台帳 [`TurnSpend`] の所有者**で、実行ループは [`run_turn_inner`] に
/// 居る。分けたのは `?` のため — 内側が `Err` で抜けても台帳はここに残るので、
/// **失敗したターンの払いを成功と同じ 3 箇所（カードの累計・`turn:` 行・予算）へ
/// 入れられる**。以前は 4 つの出口のうち `Err` だけが清算されず、
/// 課金だけが増えて村のどの数字にも出なかった。
///
/// バックエンドの解決もここで済ませる。内側より前に失敗したら（設定不備）
/// LLM は 1 度も呼ばれておらず払いは無いので、台帳を作らずに `Err` を返す —
/// **使用量の行が出るのは実行ループへ入ったターンだけ**。
#[allow(clippy::too_many_arguments)]
async fn run_turn(
    shared: &Arc<Shared>,
    agent_id: &AgentId,
    turn: &TurnHandle,
    incoming: &AgentMessage,
    reply_to: &mut Option<tokio::sync::oneshot::Sender<Reply>>,
    budget: &Option<Arc<crate::budget::BudgetPool>>,
    last_call_milli: &Arc<std::sync::atomic::AtomicU64>,
    participants: &Option<Participants>,
    spec: &AgentSpec,
    template: &ModelTemplate,
    handoffs: &HandoffTools,
    gates: HandoffGates,
    prompt: TurnPrompt,
    tools: PresentedTools,
    stable_len: usize,
) -> CoreResult<Option<TurnProduct>> {
    let backend = shared.backend_for(template).await?;
    let mut spend = TurnSpend {
        hop: incoming.hop,
        ..TurnSpend::default()
    };
    let result = run_turn_inner(
        shared,
        agent_id,
        turn,
        incoming,
        reply_to,
        budget,
        last_call_milli,
        participants,
        spec,
        template,
        handoffs,
        gates,
        prompt,
        tools,
        stable_len,
        &backend,
        &mut spend,
    )
    .await;
    if let Err(err) = &result {
        settle_failed_turn(shared, agent_id, &spend, err, backend.name()).await;
    }
    result
}

/// 失敗したターンの清算（4 つ目の出口 — `failures.md` #103）。
///
/// 成功の出口が段 7 でやること（カードの累計 + `turn:` 行）を、`Err` に対しても
/// する。**予算は既に清算済み** — 各呼び出しの予約は内側の呼び出し地点で
/// commit（払ったと分かる失敗も含む）か Drop（払いが分からない失敗 = 全額返金）
/// されており、ここで二度触ると二重計上になる。
///
/// 履歴への注記（`record_failed_turn`）と `turn failed:` 行はこれまでどおり
/// `agent_loop` が書く。この関数が足すのは**数字だけ**。
async fn settle_failed_turn(
    shared: &Arc<Shared>,
    agent_id: &AgentId,
    spend: &TurnSpend,
    err: &CoreError,
    backend_name: &str,
) {
    {
        let mut world = shared.world.write().await;
        if let Ok(record) = world.agent_mut(agent_id) {
            record.total_tokens += spend.tokens;
            record.cached_tokens += spend.cached;
            record.prompt_tokens += spend.prompt;
        }
    }
    // 成功の `turn:` 行と**同じ欄を同じ順で**書く（読む側が 1 つの書式で数えられる
    // ように）。`stop=failed:{CODE}` だけが違う。`waves` は失敗経路では数えていない
    // ので 0 と書く（波を撒いた後に落ちたターンでも 0 — 波の記録は `plan wave:` の
    // 行が別に持つ）。`rounds` の分母は失敗経路では読めないので `-`。
    note!(
        "turn: agent={agent_id} hop={} rounds={}/- waves=0 stop=failed:{} \
         prompt={} cached={} total={} reasoning={} backend={backend_name}",
        spend.hop,
        spend.rounds,
        crate::error::ErrorPayload::from(err).code,
        spend.prompt,
        spend.cached,
        spend.tokens,
        spend.reasoning,
    );
}

/// 実行ループの本体（[`run_turn`] の内側）。台帳 `spend` は外側が持ち、
/// ここは加算するだけ。**`?` で抜けてよい** — 抜けた後の清算は外側の仕事。
#[allow(clippy::too_many_arguments)]
async fn run_turn_inner(
    shared: &Arc<Shared>,
    agent_id: &AgentId,
    turn: &TurnHandle,
    incoming: &AgentMessage,
    // **`&mut` で受けるのは、早期終了の 2 経路だけが消費するから。**
    // 通常終了では段 8 が使うので、ここで所有を奪うと戻せない。
    reply_to: &mut Option<tokio::sync::oneshot::Sender<Reply>>,
    budget: &Option<Arc<crate::budget::BudgetPool>>,
    // 予約の見積もり源（Spec 38 D1(b)）。所有者は `agent_loop`。
    last_call_milli: &Arc<std::sync::atomic::AtomicU64>,
    participants: &Option<Participants>,
    spec: &AgentSpec,
    template: &ModelTemplate,
    handoffs: &HandoffTools,
    gates: HandoffGates,
    prompt: TurnPrompt,
    tools: PresentedTools,
    stable_len: usize,
    backend: &Arc<dyn crate::llm::LlmBackend>,
    spend: &mut TurnSpend,
) -> CoreResult<Option<TurnProduct>> {
    let HandoffGates {
        use_handoff_tools,
        offer_transfer,
        ..
    } = gates;
    let TurnPrompt {
        // 実行ループが周ごとに呼び出しと結果を積む（#29 — 対で積まないと 400）。
        mut messages,
        sent_user_turn,
        system_digest,
        history_depth,
    } = prompt;
    let PresentedTools {
        specs,
        executable,
        use_tools,
    } = tools;
    // 6. 実行ループ。
    //    規則は OpenAI Agents SDK と同じ:
    //    ツールを呼んだら実行して結果を積み、もう一度呼ぶ。
    //    ツールを呼ばないテキスト出力が出たら、それが最終出力。
    //
    // 消費量は台帳 `spend` に積む（`TurnSpend::absorb`）。キャッシュ読み取り分と
    // 入力ぶんを合計と別に持つのは、合計だけ見ていると、キャッシュが一度も
    // 効いていない状態と完全に効いている状態が同じ数字に見えるため
    // (実際、実機で 5 体全員が無キャッシュのまま数日走っていた。failures.md #33)。
    // キャッシュ率の分母は合計ではなく入力。思考ぶんは `tokens` の内数（Spec 32 D2）。
    //
    // 最後の LLM 応答が使った出力トークン。**空本文の診断に使う。**
    //
    // 本文もツール呼び出しも無いのに出力トークンだけがある場合、モデルは
    // **本文以外のブロック**（拡張思考など）だけを返している。実機で 2 回
    // 続けて起き、「本文が返りませんでした」としか出せなかった（2026-08-06）。
    let mut last_completion = 0u64;
    // 接地の来歴は 1 周ぶんではなく**ターンぶん**で持つ。検索した周と
    // 関数を呼んだ周は別なので、周ごとに上書きすると先に起きた接地が消える。
    let mut grounding = crate::llm::Grounding::default();
    // 思考の要約も**ターンぶん**で持つ（Spec 33）。周ごとに要約が返るので、
    // 上書きすると最後の周ぶんしか残らない。接地と同じ理由・同じ寿命だが、
    // **畳み方は違う** — 接地は URL と検索語で重複を潰すが、要約は周ごとに
    // 別の文章なので**そのまま並べる**（同じ文が 2 度出るなら、それは
    // モデルが 2 周とも同じことを考えた事実）。
    let mut reasoning_summary: Vec<String> = Vec::new();
    let mut outcome = Outcome::Finish {
        content: String::new(),
    };

    // ツール実行の上限。エージェント個別の指定があれば優先する
    // （コーディング用エージェントは調査のツール往復が多く、既定では足りない）。
    let max_tool_iterations = spec
        .max_tool_iterations
        .unwrap_or(shared.config.max_tool_iterations)
        .max(1);
    let mut tool_limit_hit = false;
    // 同一失敗の検出（failures.md #41 の処方 1）。ターン内でだけ数える —
    // ターンを跨ぐ繰り返しは別の問題（依頼が同じなら同じ失敗をもう一度たどるのは
    // 正しい）で、ここで縛るとやり直しの依頼まで殺す。
    let mut repeat_guard = RepeatGuard::default();
    // 繰り返しで打ち切ったツール名。まとめ呼び出しと最終文言の分岐に使う。
    let mut repeat_stop: Option<String> = None;
    // 観測用（Spec 04 Notes 2 のトリガー判定の実測材料）。
    // `spend.rounds` は上限の較正（12 で足りているか）、plan_wave は波の因果の追跡。
    let mut plan_wave: u32 = 0;

    for iteration in 0..max_tool_iterations {
        // 割り込みの検査点（Spec 10 — 契約の不変条件 1）。周回境界 =
        // 次の LLM 呼び出しを組み立てる前。ここなら呼び出しと結果の対（#29）が
        // 必ず揃っており、送信と保存の一致（#45）も壊れない。飛行中の
        // LLM 呼び出し・実行中のツールは完走させる（rev1 の判断 — 検知の
        // 遅さは System 行の elapsed で測り、Notes 2 の判断材料にする）。
        if turn.token.is_cancelled() {
            return finish_interrupted(
                shared,
                agent_id,
                reply_to.take(),
                turn,
                &sent_user_turn,
                *spend,
            )
            .await.map(|()| None);
        }

        // 予算の予約（Spec 11 / Spec 38 P2）。cancel の**後**に見る — 同時成立の
        // 分類は優先順位 cancel > budget_exhausted（token_budget.precedence）。
        //
        // **load 観測ではなく CAS 予約**。観測だけで通すと、波の全タスクが
        // 同じプールを見て残額 1 でも N 体が同時に通り、超過が人数倍になる
        // （#105 / specs/tla/BudgetOvershootBound で反証済み）。予約は
        // 見積もりぶんを先に引くので、境界では通る本数そのものが絞られる。
        //
        // **予約に失敗した = 尽きた扱い**。実費はまだ残っていても
        // 「次の 1 呼び出しぶんを確保できない」ので新しい呼び出しを始めない
        // （過大予約による早止まりは仕様 — Spec 38 D1(b)）。
        let reservation = match &budget {
            Some(pool) => {
                let estimate = crate::budget::reserve_estimate_milli(
                    last_call_milli.load(std::sync::atomic::Ordering::Relaxed),
                    pool.ceiling_milli(),
                );
                match pool.try_reserve(estimate) {
                    Some(guard) => Some(guard),
                    None => {
                        // 打ち切りの理由を分ける（Spec 38 P4）。「尽きた」と
                        // 「次の 1 呼び出しぶんを確保できない」は利用者から
                        // 見て別の事態で、System 行の文言だけでは残額が読めない。
                        // **打ち切った周にしか出ない** — 通常運転では 1 行も増えない。
                        note!(
                            "budget stop: agent={agent_id} ceiling={} remaining={} estimate={} reason={}",
                            pool.ceiling_effective(),
                            pool.remaining_milli().div_ceil(1000),
                            estimate.div_ceil(1000),
                            if pool.remaining_milli() == 0 {
                                "exhausted"
                            } else {
                                "reserve_short"
                            },
                        );
                        return finish_budget_exhausted(
                            shared,
                            agent_id,
                            reply_to.take(),
                            pool,
                            &sent_user_turn,
                            *spend,
                        )
                        .await.map(|()| None);
                    }
                }
            }
            None => None,
        };

        let request = ChatRequest {
            model: template.model.clone(),
            messages: messages.clone(),
            tools: if use_tools { specs.clone() } else { Vec::new() },
            tool_choice: if use_tools {
                crate::llm::ToolChoice::Auto
            } else {
                crate::llm::ToolChoice::None
            },
            temperature: template.temperature,
            max_tokens: template.max_output_tokens,
            effort: template.effort,
            cacheable_prefix_len: stable_len,
        };

        let mut response = match backend.chat(request).await {
            Ok(response) => response,
            Err(err) => {
                // **払ったと分かる失敗は、成功と同じ台帳・同じ財布で清算してから抜ける**
                // （`failures.md` #103）。以前はここが `?` で、`Err` に化けた瞬間に
                // usage ごと捨てられ、予約は guard の Drop で**全額返金**されていた —
                // 課金は起きているのに予算が減らず、上限を小さくした個体が失敗し
                // 続けると予算が減らないまま課金だけが増えた。
                //
                // 払いが分からない失敗（400 / DNS / タイムアウト）は `usage()` が
                // `None` なので、これまでどおり Drop の全額返金へ落ちる — ここで
                // 見積もりを捏造して引くと、払っていない 400 の連発が予算を食う。
                if let Some(usage) = err.usage() {
                    spend.rounds += 1;
                    spend.absorb(usage);
                    if let Some(guard) = reservation {
                        let actual = crate::budget::effective_milli(usage);
                        last_call_milli.store(actual, std::sync::atomic::Ordering::Relaxed);
                        guard.commit(actual);
                    }
                }
                return Err(err.into());
            }
        };
        spend.rounds += 1;
        note_cache_diag(
            agent_id,
            &template.model,
            spend.rounds,
            &response.usage,
            system_digest,
            history_depth,
        );
        last_completion = response.usage.completion;
        spend.absorb(&response.usage);
        // 予約を実測で清算する（Spec 11 の consume 側 / Spec 38 P2）。usage が
        // 欠けた応答（テストバックエンド・異常応答）はバイト数で保守的に
        // 見積もる — 楽観の 0 を作らない（usage_fallback の三規程）。
        //
        // **ここへ到達しなかった経路では guard の Drop が予約を全額返す。**
        // 払ったと分かる失敗は上の `Err` 腕で先に清算しているので、Drop へ落ちるのは
        // 払いが分からない失敗だけ（#103 の残余 — `LlmError::usage` の doc）。
        if let Some(guard) = reservation {
            let settled = if response.usage.total() > 0 {
                response.usage
            } else {
                let sent: usize = messages.iter().map(|m| m.content.len()).sum();
                let received: usize = response.text.as_deref().map_or(0, str::len)
                    + response
                        .tool_calls
                        .iter()
                        .map(|call| call.args.to_string().len())
                        .sum::<usize>();
                crate::budget::normalized_usage(&response.usage, sent, received)
            };
            // 次の予約の見積もりは**この実測**（Spec 38 D1(b)）。書くのは
            // 清算した所だけ — 予約時に書くと、失敗して返金した呼び出しの
            // 見積もりが次へ引き継がれる。
            let actual = crate::budget::effective_milli(&settled);
            last_call_milli.store(actual, std::sync::atomic::Ordering::Relaxed);
            guard.commit(actual);
        }
        // 転送で抜ける周の接地も拾う。break の後ろに置くと、検索してから
        // 転送したターンの来歴が丸ごと落ちる。
        grounding.absorb(std::mem::take(&mut response.grounding));
        reasoning_summary.append(&mut response.reasoning_summary);

        // 転送の要求は「会話を渡す」ことなので、ここでループを抜ける。
        // 結果が返ってくる種類の操作ではない。
        // 提示していない道具の呼び出しは拾わない。**提示集合と判定集合を
        // 揃える** — ずれると「出していないのに効く」か「出したのに効かない」の
        // どちらかになる（Spec 20 で踏んだ形）。
        outcome = handoffs.decide(&response, use_handoff_tools, offer_transfer);
        if matches!(outcome, Outcome::Handoff { .. }) {
            break;
        }


        // **提示していない名前も捨てない。** 以前はここで filter して落として
        // いたため、モデルが実在しない名前を呼ぶと呼び出しはログにも
        // `tool_result` にも残らず消え、モデルは「呼んだのに何も起きない」まま
        // 本文を書いた。しかも `execute_tool` には「そのツールはありません」と
        // いう文言が既にあるのに、**捨てられた呼び出しはそこへ到達できない**
        // （到達不能な分岐だった）。結果を返せばモデルは自分で直せる。
        let calls: Vec<_> = response.tool_calls.clone();

        if calls.is_empty() {
            // ツールを呼ばなかった = 最終出力。
            //
            // ただし本文にツール呼び出しの XML が漏れているなら、それは
            // 「呼ばなかった」ではなく「呼び損ねた」— 生の XML が答えとして
            // 配信される。挙動は変えずに**気づける**ようにする（計器）。
            if let Some(text) = &response.text
                && looks_like_leaked_tool_call(text)
            {
                // **本文そのものは出さない。** 漏れた XML はモデルが書いた文章で、
                // そこには利用者が渡した秘密が入りうる — 実機で
                // `Authorization: Bearer …` を丸ごとログへ書いた（2026-08-06）。
                // 計器の目的は「漏れたか」と「どの形か」で、本文は要らない。
                note!(
                    "text tool call leaked: agent={agent_id} round={} chars={} markers={}",
                    iteration + 1,
                    text.chars().count(),
                    leaked_markers(text).join("+"),
                );
            }
            break;
        }

        // 呼び出しと結果は**対で**積む。呼び出しを残さずに結果だけ積むと、
        // プロバイダが「対応する呼び出しが無い結果」として拒否する。
        messages.push(ChatMessage::assistant_tool_calls(
            response.text.clone().unwrap_or_default(),
            calls.clone(),
        ));

        // この周で実際に走った本数と、繰り返しで止めた本数。
        // **全部止めた周だけがループの打ち切り条件**（新しいことを何もしていない周）。
        let mut executed_in_round = 0usize;
        let mut blocked_in_round: Option<String> = None;

        // 1 本ぶんの処理は CallRunner へ出した。**ここに残したのは
        // `messages` と周ごとのカウンタ**で、戻り値で受ければ借用が 1 つ減る。
        {
            let mut runner = CallRunner {
                shared,
                agent_id,
                spec,
                handoffs,
                incoming,
                turn,
                budget,
                participants,
                executable: &executable,
                use_handoff_tools,
                repeat_guard: &mut repeat_guard,
                plan_wave: &mut plan_wave,
            };
            for call in &calls {
                let (body, executed, blocked) = match runner.on_call(call, iteration + 1).await {
                    CallOutcome::Executed(body) => (body, true, None),
                    CallOutcome::Blocked { body, tool } => (body, false, Some(tool)),
                };
                if executed {
                    executed_in_round += 1;
                }
                if let Some(tool) = blocked {
                    blocked_in_round.get_or_insert(tool);
                }
                messages.push(ChatMessage::tool_result(&call.id, &call.name, body));
            }
        }

        // **この周が丸ごと空振りだったときだけ**打ち切る。1 本が重複しただけの
        // 周は続ける — 並列で呼ばれた残りは新しい仕事をしている。
        // 上限到達の通知は出さない（当たったのは上限ではない。理由を 1 つに保つ）。
        if executed_in_round == 0
            && let Some(tool) = blocked_in_round
        {
            note!(
                "turn cut: agent={agent_id} round={} reason=repeat tool={tool}",
                iteration + 1,
            );
            repeat_stop = Some(tool);
            break;
        }

        // 上限に達したら、次の周回は回さずに今ある本文で終える。
        if iteration + 1 == max_tool_iterations {
            shared.emit(CoreEvent::ToolLimitReached {
                agent_id: agent_id.clone(),
                max_iterations: max_tool_iterations,
            });
            tool_limit_hit = true;
        }
    }

    // まとめ呼び出しが失敗したときの理由。フォールバック文言に載せる（#4 の規律:
    // 退避には落ちた事実・理由・復帰条件の 3 点を出口に付ける）。
    let mut summary_error: Option<String> = None;

    // ツール上限で打ち切られてテキストが無いときは、**ツールの使用を禁じて最後に
    // 1 回だけ呼び、ここまでの結果を文章化させる**。
    //
    // 中間のツール結果はこのターンの `messages` にしか存在せず、履歴には
    // 積まれない。まとめずに捨てると、利用者が「続けて」と送るたびに
    // ゼロから調査をやり直して同じ上限に当たり、トークンだけが燃え続ける
    // （実機で 3 ターン連続 146k tok を観測）。ここで 1 回のまとめ呼び出しに
    // 変換すれば、燃えたトークンの成果がそのまま答えになる。
    //
    // 繰り返しの打ち切り（failures.md #41 の処方 1）も同じ扱いにする。理由は同じで、
    // まとめずに終えると利用者が「続けて」と送り、同じ所まで走って同じ所で止まる。
    if let Outcome::Finish { content } = &outcome
        && content.trim().is_empty()
        && (tool_limit_hit || repeat_stop.is_some())
    {
        // まとめの 1 回も同じ財布から**予約**する（Spec 38 P2）。確保できな
        // ければまとめを呼ばない — 尽きたら新しい LLM 呼び出しを始めない
        // （token_budget の exhaustion。打ち切りの直後にもう 1 回課金しない、
        // という割り込みのまとめ省略と同じ判断でもある）。
        let summary_reservation = match &budget {
            Some(pool) => {
                let estimate = crate::budget::reserve_estimate_milli(
                    last_call_milli.load(std::sync::atomic::Ordering::Relaxed),
                    pool.ceiling_milli(),
                );
                match pool.try_reserve(estimate) {
                    Some(guard) => Some(guard),
                    None => {
                        // 打ち切りの理由を分ける（Spec 38 P4）。「尽きた」と
                        // 「次の 1 呼び出しぶんを確保できない」は利用者から
                        // 見て別の事態で、System 行の文言だけでは残額が読めない。
                        // **打ち切った周にしか出ない** — 通常運転では 1 行も増えない。
                        note!(
                            "budget stop: agent={agent_id} ceiling={} remaining={} estimate={} reason={}",
                            pool.ceiling_effective(),
                            pool.remaining_milli().div_ceil(1000),
                            estimate.div_ceil(1000),
                            if pool.remaining_milli() == 0 {
                                "exhausted"
                            } else {
                                "reserve_short"
                            },
                        );
                        return finish_budget_exhausted(
                            shared,
                            agent_id,
                            reply_to.take(),
                            pool,
                            &sent_user_turn,
                            *spend,
                        )
                        .await.map(|()| None);
                    }
                }
            }
            None => None,
        };

        messages.push(ChatMessage::system(match &repeat_stop {
            Some(tool) => format!(
                "`{tool}` を同じ引数で繰り返し呼び、同じ結果が返り続けたため、\
                 ツール実行を打ち切りました。これ以上ツールは使えません。\
                 ここまでのツール結果から分かったことと、**何ができなかったのか**を\
                 最終回答としてまとめてください。同じ操作を勧める提案はしないでください。"
            ),
            None => "ツール実行の上限に達しました。これ以上ツールは使えません。\
                 ここまでのツール結果から分かったことを、最終回答としてまとめてください。\
                 調査が途中なら、どこまで分かっていて何が残っているかを書いてください。"
                .to_owned(),
        }));
        // ツールを取り上げるのは `tools` を消すことではなく `tool_choice` で縛る。
        // 履歴には直前のツール往復（tool_use / tool_result）が積まれたままなので、
        // `tools` を空にすると Anthropic が「tool ブロックを含むなら tools の定義が
        // 必須」の 400 を返し、**まとめはモデルに届く前にワイヤで死ぬ**
        // （実機で発生。failures.md #36）。定義は残し、使用だけを禁じる。
        let request = ChatRequest {
            model: template.model.clone(),
            messages: messages.clone(),
            tools: if use_tools { specs.clone() } else { Vec::new() },
            tool_choice: crate::llm::ToolChoice::None,
            temperature: template.temperature,
            max_tokens: template.max_output_tokens,
            effort: template.effort,
            cacheable_prefix_len: stable_len,
        };
        // まとめの失敗でターンごと落とさない。ただし**理由は握り潰さない** —
        // ここを `if let Ok` で書いていた間、まとめが落ちても理由はログにも
        // イベントにもフォールバック文言にも残らず、現場から診断不能だった。
        match backend.chat(request).await {
            Ok(mut response) => {
                // まとめ呼び出しの周。**ここは必ず書き込みになる** —
                // tool_choice を None へ変えると履歴層のキャッシュが落ちるため
                // （failures.md #42 の bounds）。0% でも異常ではない。
                note_cache_diag(
                    agent_id,
                    &template.model,
                    spend.rounds + 1,
                    &response.usage,
                    system_digest,
                    history_depth,
                );
                last_completion = response.usage.completion;
                spend.absorb(&response.usage);
                // まとめ呼び出しも同じ財布で清算する（Spec 11 / Spec 38 P2）。
                if let Some(guard) = summary_reservation {
                    let settled = if response.usage.total() > 0 {
                        response.usage
                    } else {
                        let sent: usize = messages.iter().map(|m| m.content.len()).sum();
                        let received: usize =
                            response.text.as_deref().map_or(0, str::len);
                        crate::budget::normalized_usage(&response.usage, sent, received)
                    };
                    let actual = crate::budget::effective_milli(&settled);
                    last_call_milli.store(actual, std::sync::atomic::Ordering::Relaxed);
                    guard.commit(actual);
                }
                grounding.absorb(std::mem::take(&mut response.grounding));
                reasoning_summary.append(&mut response.reasoning_summary);
                match response.text {
                    Some(text) if !text.trim().is_empty() => {
                        outcome = Outcome::Finish { content: text };
                    }
                    // 本文が無いのにツール呼び出しがある = プロバイダが
                    // `tool_choice: none` を無視した。「空だった」に丸めると、
                    // モデルの不調と経路の不調が同じ文言になり切り分けられない
                    // （実機の flash-lite / 互換経路で「本文が空」を観測。
                    // この分岐はその容疑を次回から名指しするための計器）。
                    None | Some(_) if !response.tool_calls.is_empty() => {
                        summary_error = Some(format!(
                            "モデルが本文ではなくツール呼び出し（{}）で応えました。\
                             この経路は tool_choice の禁止指定を無視している可能性があります",
                            response
                                .tool_calls
                                .iter()
                                .map(|call| call.name.as_str())
                                .collect::<Vec<_>>()
                                .join("、")
                        ));
                    }
                    _ => {}
                }
            }
            Err(err) => {
                // まとめの呼び出しが**払ったうえで**失敗した場合も台帳と財布へ入れる
                // （#103 と同じ穴がここにもあった — 出力上限で切れたまとめは
                // 課金されているのに、`summary_error` の文言にしかならなかった）。
                if let Some(usage) = err.usage() {
                    spend.absorb(usage);
                    if let Some(guard) = summary_reservation {
                        let actual = crate::budget::effective_milli(usage);
                        last_call_milli.store(actual, std::sync::atomic::Ordering::Relaxed);
                        guard.commit(actual);
                    }
                }
                summary_error = Some(err.to_string());
            }
        }
    }

    // それでも最終出力が空なら、正直な文言で置き換える。
    //
    // 空の発話を記録すると (1) UI に空バブルが出る (2) 履歴に空の assistant が
    // 積まれ、**次のターンの API リクエストが 400 (text content blocks must be
    // non-empty) で落ちてエージェントごと止まる**。空という値は連鎖的に
    // 毒になる（failures.md #29、実機で発生）。
    if let Outcome::Finish { content } = &mut outcome
        && content.trim().is_empty()
    {
        // 理由を必ず添える。「失敗しました」だけでは、設定を直せば済むのか
        // ワイヤの障害なのかを利用者が判別できない。
        let reason = || {
            summary_error
                .as_deref()
                .map(|err| format!("失敗の理由: {err}。"))
                .unwrap_or_else(|| "モデルは応答しましたが本文が空でした。".to_owned())
        };
        *content = if let Some(tool) = &repeat_stop {
            // 打ち切りの理由は上限ではないので、上限の直し方を案内しない
            // （直しても直らないものを勧めると、次の依頼がそのぶん燃える）。
            format!(
                "（`{tool}` を同じ引数で繰り返し呼び、同じ結果が返り続けたため\
                 ツール実行を打ち切りました。まとめの生成にも失敗しています。{}\
                 頼み方を変えるか、必要な情報を直接渡してください。）",
                reason()
            )
        } else if tool_limit_hit {
            format!(
                "（ツール実行の上限 {max_tool_iterations} 回に達し、まとめの生成にも\
                 失敗しました。{}\
                 エージェント設定で上限を上げるか、依頼を小さく分けてください。）",
                reason()
            )
        } else {
            // **観測できた事実だけを書く。** 出力トークンがあるのに本文も
            // ツール呼び出しも無いなら、モデルは**本文以外のブロック**
            // （拡張思考など）だけを返している。種別は `dropped content blocks:`
            // の 1 行がログに残す — ここで「thinking です」と断定しない
            // （見たのはトークン数であって、ブロックの中身ではない）。
            if last_completion > 0 {
                format!(
                    "（モデルは出力 {last_completion} トークンを使いましたが、\
                     本文もツール呼び出しも返しませんでした。\
                     もう一度頼んでみてください。繰り返すなら頼み方を変えてください。）"
                )
            } else {
                "（モデルから本文が返りませんでした。もう一度頼んでみてください。）".to_owned()
            }
        };
    }

    // 7. 統計と履歴を更新する。履歴には「実際に言ったこと」を積む。
    //    受信側は**送ったものをそのまま**積む — プロンプトと履歴の形を揃えないと、
    //    過去のターンだけ出所不明に戻るうえ、**その位置で前方一致が切れて
    //    キャッシュが頭打ちになる**（failures.md #45）。
    {
        let mut world = shared.world.write().await;
        if let Ok(record) = world.agent_mut(agent_id) {
            record.total_tokens += spend.tokens;
            record.cached_tokens += spend.cached;
            record.prompt_tokens += spend.prompt;
        }
        shared.push_exchange(&mut world, agent_id, &sent_user_turn, &outcome.spoken());
    }

    // 観測用のターン行（Spec 04 Notes 2 / Notes 12 のトリガー判定の実測材料）。
    // prompt の伸びは束ねの履歴肥大 (O(N²) 懸念) を、rounds は上限 12 の較正を、
    // waves は plan の利用実態を、それぞれ将来の判断のために記録する。
    // 機構は入れない — 測らずに入れると「効いているか分からない機構」が増える。
    // stop はループの抜け方。rounds が上限より小さいのに短く終わったターンを
    // 「モデルが早く答えた」と読み違えないために出す（繰り返しの打ち切りは
    // rounds を途中で止める唯一の機構）。
    let stop = match &repeat_stop {
        Some(tool) => format!("repeat:{tool}"),
        None if tool_limit_hit => "tool_limit".to_owned(),
        None => "-".to_owned(),
    };
    // 思考の要約を**受け取った回だけ** 1 行（`xai search:` と同じ形）。
    //
    // **量を数えないと、捕獲したものが存在しなかったことになる**（#72 の裏返し —
    // あちらは捨てたものを数えなかった）。実機で `kinds=reasoning:36` を観測して
    // おり、**1 応答に数十件入りうる**。件数と字数の分布は、表示を畳むかどうか・
    // 保存が膨らまないかの判断材料になる。
    if !reasoning_summary.is_empty() {
        note!(
            "reasoning summary: agent={agent_id} items={} chars={}",
            reasoning_summary.len(),
            reasoning_summary
                .iter()
                .map(|s| s.chars().count())
                .sum::<usize>(),
        );
    }

    note!(
        // `reasoning` は `total` の内数（Spec 32 D2）。**0 でも必ず出す** —
        // 思考を使わない経路で 0 が出ることが「常に 0 を出す実装」との対照になり、
        // 省くと機構が効いているか読めなくなる（D3）。
        // **欄の並びは [`settle_failed_turn`] の失敗行と揃える**（#103） — 読む側が
        // 1 つの書式で成功と失敗を数えられるように。違うのは `stop=` の値だけ。
        "turn: agent={agent_id} hop={} rounds={}/{max_tool_iterations} \
         waves={plan_wave} stop={stop} prompt={} cached={} total={} \
         reasoning={} backend={}",
        incoming.hop,
        spend.rounds,
        spend.prompt,
        spend.cached,
        spend.tokens,
        spend.reasoning,
        // どのワイヤを通ったか（Spec 34 P5 の前に追加）。**これが無いと、
        // ワイヤを足したことを実機で確かめられない** — Spec 31 は
        // `xai search:` が偶然その役をしていたが、あの行は検索を有効にした
        // 呼び出しでしか出ない。検索を使わない村ではプロトコルを切り替えても
        // **ログが 1 行も変わらなかった**（実機で 2 ターン撃って、どちらの口を
        // 通ったか読めなかった）。`LlmBackend::name()` は在ったのに、
        // テストの assert 以外で 1 度も呼ばれていなかった。
        backend.name(),
    );
    Ok(Some(TurnProduct {
        outcome,
        tokens: spend.tokens,
        grounding,
        reasoning_summary,
    }))
}


/// 1 本のツール呼び出しの結末。**本文は必ずある** — 呼び出しだけ残して結果を
/// 落とすと、次のリクエストが「対応する結果が無い呼び出し」として 400 で
/// 拒否される（`failures.md` #29）。
enum CallOutcome {
    /// 実行した。**「そんな道具は無い」を返した場合も含む** — 新しい情報を
    /// 返しているので、空振りの周には数えない。
    Executed(String),
    /// 繰り返しで止めた。結果は積むが、この周の実行数には数えない。
    Blocked {
        /// モデルへ返す本文。
        body: String,
        /// 止めたツール名（周ごとの打ち切り判定に使う）。
        tool: String,
    },
}

/// 1 本の呼び出しを走らせるのに要る借用を束ねたもの。
///
/// **`&mut` で持つのは 2 つだけ**（`repeat_guard` / `plan_wave`）。ほかは
/// 読むだけで、`messages` と周ごとのカウンタは**呼び出し側に残した** —
/// 戻り値で返せば借用が 1 つ減り、`run_turn` 側で `messages` を触り続けられる。
///
/// **ここを型で束ねたのは、`for` の中身を関数へ出すため**（引数 12 個の関数に
/// すると呼び出し側が読めなくなる）。分割の 5 箇条の「境界を型にする」。
struct CallRunner<'a> {
    shared: &'a Arc<Shared>,
    agent_id: &'a AgentId,
    spec: &'a AgentSpec,
    handoffs: &'a HandoffTools,
    incoming: &'a AgentMessage,
    turn: &'a TurnHandle,
    budget: &'a Option<Arc<crate::budget::BudgetPool>>,
    participants: &'a Option<Participants>,
    /// registry と個別 MCP で実行できるもの（実行可否の判定に使う）。
    executable: &'a [ToolSpec],
    use_handoff_tools: bool,
    /// 同一失敗の検出（#41 の処方 1）。**ターンをまたいで持ち回る**ので `&mut`。
    repeat_guard: &'a mut RepeatGuard,
    /// 波の連番（Spec 08）。`plan` を呼んだ回だけ進む。
    plan_wave: &'a mut u32,
}

impl CallRunner<'_> {
    /// 実行できる呼び出しか。
    ///
    /// **転送用の名前はここに来ない**（`Outcome::Handoff` で先に抜けている）。
    /// 委譲（`ask_*`）は**結果が返る**ので実行ツールと同じ扱い。
    fn is_runnable(&self, call: &crate::llm::ToolCall) -> bool {
        self.executable.iter().any(|spec| spec.name == call.name)
            || (self.use_handoff_tools && self.handoffs.resolve_ask(&call.name).is_some())
            // plan は executable にも resolve_ask にも該当しない。
            // ここへ足し忘れると呼び出しが素通りし、モデルが呼んだのに
            // **何も起きず本文だけ返る**（エラーにならないので気づけない）。
            || (self.use_handoff_tools
                && self.handoffs.offers_plan()
                && call.name == HandoffTools::PLAN)
            // room_log も orchestrator 合成（Spec 22）なので executable に
            // 居ない。条件は提示（spec_for 相当）と同じ式に揃える。
            || (self.spec.hears_room_log
                && call.name == crate::room_log::ROOM_LOG_TOOL_NAME)
    }

    /// 1 本走らせる。`round` は 1 始まりの周回数（ログの `round=`）。
    async fn on_call(&mut self, call: &crate::llm::ToolCall, round: u8) -> CallOutcome {
        // 読むだけの借用はここでローカルへ落とす。**`note!` のインライン展開
        // （`{agent_id}`）は式を取れない**ので、`self.` のままだと本文が書けない。
        let shared = self.shared;
        let agent_id = self.agent_id;
        let spec = self.spec;
        let handoffs = self.handoffs;
        let incoming = self.incoming;
        let turn = self.turn;
        let budget = self.budget;
        let participants = self.participants;
        let use_handoff_tools = self.use_handoff_tools;
        // 同じ呼び出しに同じ結果が返り続けているなら、この 1 本は実行しない
        // （failures.md #41 の処方 1）。**結果は必ず積む** — 呼び出しだけ
        // 残して結果を落とすと、次のリクエストが「対応する結果が無い
        // 呼び出し」として 400 で拒否される（#29）。
        // 返す本文は短くする。**ここが効きの本体** — 同じ 12,000 字を
        // もう一度積むと、以後の全周回でそれが再送される。
        if self.repeat_guard.blocks(&call.name, &call.args) {
            let repeats = self.repeat_guard.repeats(&call.name, &call.args);
            shared.emit(CoreEvent::ToolRepeatBlocked {
                agent_id: agent_id.clone(),
                tool: call.name.clone(),
                repeats,
            });
            let body = format!(
                "`{}` は同じ引数で既に {repeats} 回、同じ結果を返しています。\
                 もう一度呼んでも同じなので実行しませんでした。\
                 引数か手順を変えるか、**できなかったこと自体を答えとして**\
                 報告してください。",
                call.name
            );
            note!(
                "tool blocked: agent={agent_id} round={} name={} repeats={repeats}",
                round,
                call.name,
            );
            return CallOutcome::Blocked {
                body,
                tool: call.name.clone(),
            };
        }

        // 提示していない名前。**捨てずに「無い」ことを結果として返す。**
        //
        // 判定を RepeatGuard の**後**に置くのは、同じ実在しない名前を呼び
        // 続けたときに周回の打ち切りへ落ちるようにするため（先に置くと
        // 同じ結果を返し続けて上限まで回る）。イベントは出さない —
        // `ToolInvoked` は「ツールが走った」の意味で、走っていないものを
        // 混ぜると UI の直近ツールが実在しない名前で埋まる。
        if !self.is_runnable(call) {
            let body = format!(
                "`{}` というツールはありません。提示された名前から選んでください。",
                call.name
            );
            note!(
                "tool unknown: agent={agent_id} round={} name={}",
                round,
                call.name,
            );
            // 数えるのは**モデルへ返した本文**（RepeatGuard の規律と同じ）。
            self.repeat_guard.observe(&call.name, &call.args, &body);
            // 「無い」と伝えるのも新しい情報なので、空振りの周にはしない。
            return CallOutcome::Executed(body);
        }

        // 理由（Spec 27）の既定は `Excluded` = **合成側**（plan / room_log /
        // ask / handoff）。あれらは `AgentTool` を実装しておらず、
        // **引数が発話として会話ペインに出る**ので理由は重複になる。
        // registry へ落ちた呼び出しだけが下で上書きする。
        //
        // **条件を書き写して先に判定しない。** 分岐の条件をここでもう一度
        // 書くと、同じ規律が 2 箇所に生えて片方だけ古くなる。
        // 既定値 + 実際に通った枝での上書きなら、**枝が増えても既定へ落ちる**。
        let mut reason = (crate::tool_reason::ReasonState::Excluded, 0usize);
        // 並列委譲は 1 回の呼び出しで N 体ぶんの仕事をする。ツール実行の
        // 上限（`max_tool_iterations`）の消費も 1 回で済む。
        let result = if use_handoff_tools
            && handoffs.offers_plan()
            && call.name == HandoffTools::PLAN
        {
            *self.plan_wave += 1;
            Ok(run_plan(
                shared,
                agent_id,
                handoffs,
                call,
                incoming.hop,
                *self.plan_wave,
                &turn.token,
                budget.as_ref(),
                participants.as_ref(),
                incoming.attachments.first().map(|a| a.kind()),
            )
            .await)
        } else if spec.hears_room_log
            && call.name == crate::room_log::ROOM_LOG_TOOL_NAME
        {
            // 広場ログの全文読み（Spec 22）。名前一致でここが勝つのは
            // `transfer_to_*` / `ask_*` と同じ規則 — orchestrator 合成の
            // 名前は registry（MCP 由来の同名）より先に解決される。
            // hears_room_log = false の個体では素通りして registry 側へ
            // 落ちる（その個体にこのツールは合成されていない）。
            Ok(read_room_log(shared, agent_id, call).await)
        } else {
            match handoffs.resolve_ask(&call.name) {
                Some(target) if use_handoff_tools => {
                    ask_agent(
                        shared,
                        agent_id,
                        target,
                        call,
                        incoming.hop,
                        &turn.token,
                        budget.as_ref(),
                        participants.as_ref(),
                        incoming.attachments.first().map(|a| a.kind()),
                    )
                    .await
                }
                _ => {
                    reason = registry_reason(shared, agent_id, call).await;
                    execute_tool(shared, agent_id, call, &turn.token).await
                }
            }
        };
        let (reason, reason_chars) = reason;
        // 状態の名前を先に取る（この後 `reason` はイベントへ移る）。
        let reason_kind = crate::tool_reason::kind_label(&reason);
        shared.emit(CoreEvent::ToolInvoked {
            agent_id: agent_id.clone(),
            tool: call.name.clone(),
            ok: result.is_ok(),
            reason,
        });
        let ok = result.is_ok();
        let body = match result {
            Ok(text) => text,
            // 失敗しても会話を止めない。モデルが読んで次を決める。
            Err(err) => format!("ツールの実行に失敗しました: {err}"),
        };
        // ツール 1 本ごとの実測。**`body_chars` がこの行の主目的** — ツール結果は
        // 履歴に積まれて以後の全周回で再送されるので、1 本の大きさが
        // そのターンの入力トークンに周回数ぶん掛かって効く。ターン行の
        // `rounds` と `prompt` だけでは「何がプロンプトを太らせたか」が
        // 追えなかった（2026-07-31 の 730,406 トークンの診断で不足した欄）。
        // `ok` は `Err` だったかどうかで、同梱ツールは失敗も `Ok` の本文で
        // 返すため `ok=true` のまま失敗していることがある（CoreEvent::ToolInvoked
        // と同じ意味。判定材料にするなら本文の側を見る）。
        // `reason_chars` は**トリム後・切り詰め前**（Spec 27 D3）。本文は出さない —
        // モデルの出力を記録する計器は秘密の転送経路になる（failures.md #71）。
        // 切り詰め後を出すと「モデルが上限を超えて書くか」が全部 60 に
        // 貼り付いて測れなくなる。
        //
        // **`reason=` を併記するのは、字数だけでは 3 つの状態が 0 に畳まれるため**
        // （書かなかった / 外部なので尋ねていない / 対象外）。**畳むと後から
        // 区別できない** — 実機の初日に、尋ねていない 2 件を「短い理由」として
        // 平均へ混ぜる誤りを踏んだ（Spec 27 の P4 実装記録）。
        note!(
            "tool: agent={agent_id} round={} name={} ok={ok} args_chars={} body_chars={} reason={reason_kind} reason_chars={reason_chars}",
            round,
            call.name,
            call.args.to_string().chars().count(),
            body.chars().count(),
        );
        // 数えるのは**モデルへ返した本文**。同梱ツールの失敗は `Err` ではなく
        // この本文に乗るので、ここで数えないと実機の失敗ループは検出できない。
        self.repeat_guard.observe(&call.name, &call.args, &body);
        CallOutcome::Executed(body)
    }
}

/// 送るプロンプト一式（段 4）。
///
/// **`sent_user_turn` を一緒に返すのが要点**。可変文脈を畳んだこの文字列を
/// **そのまま履歴へ積む**規律（`failures.md` #45）があり、組み立てた側と
/// 保存する側で別々に作ると前方一致がそこで切れてキャッシュが頭打ちになる。
/// 同じ関数から出すことで、2 つが同じ文字列であることを型で保つ。
struct TurnPrompt {
    /// バックエンドへ送る messages。
    messages: Vec<ChatMessage>,
    /// 畳んだ user 発話。**履歴へ積むのはこれ**。
    sent_user_turn: String,
    /// 安定プレフィックスの指紋（計器）。
    system_digest: SystemDigest,
    /// 滑る窓の深さ（計器）。
    history_depth: HistoryDepth,
}

/// プロンプトを組む（段 4）。
///
/// **切り出したのは、ここが「モデルへ何を見せるか」だけを決める段だから** —
/// ツールの実行も応答の判定もしない。可変文脈（要約・広場ログ・入退室・
/// 同報・添付）はすべてここで 1 本の user 発話へ畳まれる（#45）。
#[allow(clippy::too_many_arguments)]
async fn build_prompt(
    shared: &Arc<Shared>,
    agent_id: &AgentId,
    spec: &AgentSpec,
    incoming: &AgentMessage,
    system_prompt: String,
    stable_len: usize,
    has_roster: bool,
    handoffs: &HandoffTools,
    gates: HandoffGates,
) -> TurnPrompt {
    let HandoffGates {
        use_handoff_tools,
        offer_transfer,
        awaiting_reply,
    } = gates;
    // 4. プロンプトを組む。順序は system → 手順 → 履歴 → 可変の文脈 + 今回の受信。
    //
    // **`Role::System` は「安定なもの」専用の枠として扱う。** adapter は
    // Role::System のメッセージを配列のどこにあっても全部引き抜いて 1 つの
    // system / systemInstruction へ連結するので（gemini.rs / anthropic.rs の
    // encode）、可変なものを System で積むと**配列上の位置に関係なく前方一致の
    // 先頭へ戻る**。位置を変えるだけでは直らない（failures.md #45）。
    let mut messages = vec![ChatMessage::system(system_prompt)];
    if !handoffs.is_empty() {
        let language = shared
            .world
            .read()
            .await
            .language()
            .unwrap_or(crate::world::Language::Ja);
        messages.push(ChatMessage::system(handoffs.protocol_note(
            use_handoff_tools,
            offer_transfer,
            awaiting_reply,
            language,
        )));
    }

    // 履歴。これが無いと毎回コールドスタートになり、同じ入力に同じ出力を返し続ける。
    //
    // 通数を控える理由は診断のため。`history_turns` は**滑る窓**で（world.rs の
    // push_exchange が先頭から drain する）、埋まると毎ターン先頭の 1 往復が落ちる。
    // 前方一致はそこで切れるので、窓が埋まった瞬間からキャッシュは system 止まりに
    // なる。窓が上限に張り付いているかは通数を見ないと分からない。
    let history_msgs = {
        let world = shared.world.read().await;
        match world.agent(agent_id) {
            Ok(record) => {
                messages.extend(record.history.iter().cloned());
                record.history.len()
            }
            Err(_) => 0,
        }
    };

    // ここから下は**毎ターン変わる文脈**。System では積まず、`context` へ溜めて
    // 最後に今回の受信と一緒に 1 本の user 発話として送る。
    //
    // こうする理由は 2 つ。(1) System で積むと adapter が先頭へ畳むので
    // 前方一致がそこで切れる。(2) user ロールで別々に積むと user が連続し、
    // ロールの交互を要求するプロバイダで壊れる。**1 本に畳めば両方避けられる。**
    //
    // 履歴には入れない（`attributed` だけを積む）— 今回だけの文脈を履歴へ
    // 焼き付けると、以後の全ターンのプレフィックスに残り続ける。
    let mut context: Vec<String> = Vec::new();

    // これまでの経緯（Spec 12 P4 の手動要約）。作られていれば毎ターン差す。
    //
    // **履歴の中に summary 専用の席は作らない。** ここ（可変文脈）へ差せば、
    // 畳んでできた `sent_user_turn` がそのまま `exchange` として保存されるので、
    // 送信と保存が食い違わない（failures.md #45）。履歴へ直接積むと、
    // 保存側は「送った文字列そのもの」を持つ規律なので二重に入る。
    //
    // 注入するのは**最新の 1 本だけ**。古い要約はレコードとして残るが、
    // 痕跡であって現役ではない。
    if let Some(summary) = shared.summaries.read().await.get(agent_id) {
        context.push(format!(
            "## これまでの経緯（要約）\n{summary}\n\n\
             （この要約より後のやり取りは、下の履歴にそのまま残っています）"
        ));
    }

    // 参照資料の push 注入は Spec 18 で廃止した（pull のみ = モデルが `rag`
    // ツールで読みに行く）。撤去自体は挙動を変えていない — 旧 RagIndex には
    // 取り込み導線が無く索引は常に空で、この位置の注入は一度も発火しなかった。

    // 居合わせた会話（広場ログ）。受信側でオプトアウトできる（Spec 03）:
    // 毎ターン最大 12 件 × 200 字の固定費であり、場の共有が要らない役には
    // 価値が無い。false でも自分の発話は他者の広場ログに載る（受信側だけの設定）。
    //
    // 元は「場の背景であって自分とのやり取りではない」から System の枠で履歴の
    // **前**に置いていた。その読みは筋が通っていたが、**他人が喋るたびに前方一致が
    // 切れる**という代償が見えていなかった — 村として使っているときにこそ
    // キャッシュが効かなくなる。
    if spec.hears_room_log
        && let Some(room) = compose_room_log(shared, agent_id, &shared.config).await
    {
        context.push(room);
    }

    // 入退室の通知（Spec 06 P1）。**広場ログの gate の外**に置く —
    // 広場ログのオプトアウトは「場の共有が要らない役から固定費を外す」機能で、
    // 入退室は場の雑談ではなく配送先の正しさに関わる情報。コストの設定が
    // 経路の正しさを黙って壊す形にしない。
    if let Some(notices) =
        compose_presence_notices(shared, &shared.config, has_roster).await
    {
        context.push(notices);
    }

    // 送り手の封筒。ユーザーの言葉もエージェントからの転送も同じ user ロールで
    // 届くため、名前を書かないと受信側は区別できない — 実際にユーザーの発話を
    // 「他のエージェントが話した言葉」と取り違えた。プロンプトと履歴の両方へ
    // 同じ形で入れる。履歴に入れないと、次のターンで再び出所不明になる。
    let attributed = attribute_sender(shared, incoming).await;

    // 同報の注記。「みんなへ」と呼びかけられたのに自分しか受け取っていないように
    // 見えると、各エージェントは律儀に接続先へ転送して反響が起きる（実機で観測）。
    // 転送を禁止するのではなく、「全員が既に受け取っている」という事実を与えて
    // 転送する理由そのものを消す。
    //
    // これも System では積まない。同報かどうかは発話ごとに変わるので、System へ
    // 入れると adapter が先頭へ畳んで前方一致を切る（failures.md #45）。
    if incoming.co_recipients.len() >= 2 {
        let world = shared.world.read().await;
        let names: Vec<String> = incoming
            .co_recipients
            .iter()
            .map(|id| {
                world
                    .agent(id)
                    .map(|record| record.spec.name.clone())
                    // 宛先が既に削除されていても注記自体は成立させる。ID で示す。
                    .unwrap_or_else(|_| id.to_string())
            })
            .collect();
        // 「転送するな」だけでは足りない。実機では、転送の代わりに
        // 「ユーザーから依頼です、自己紹介お願いします」という**新しい発話**を
        // 全員へ配って回り、同じ混乱が起きた（促しは転送ではないので注記の射程外だった）。
        // 塞ぐべきは経路ではなく、**他人の分まで面倒を見ようとする動機**のほう。
        context.push(format!(
            "【同報】この発話はあなたを含む {} 体（{}）へ同時に届いています。\
             全員が同じ内容を既に受け取っており、**それぞれが自分で答えます**。\
             したがって、この内容を他のエージェントへ転送する必要はありませんし、\
             他の参加者に発言を促す必要もありません。\
             あなたは**あなた自身の分だけ**答えてください。",
            names.len(),
            names.join("、")
        ));
    }

    // 添付画像を実体へ展開する（Spec 23）。読むのは**このターンの受信に付いた
    // 参照だけ**で、履歴の発話は `String` なので構造的に画像を持てない（D1）。
    // 読めなかった参照（GC 済み・ファイル欠損）は黙って抜かずに本文で断る —
    // モデルが「画像を見た」ふりで答える形が一番診断しにくい（#44 の同型）。
    let image_attachments = load_turn_attachments(shared, agent_id, incoming).await;
    // モデルへ届く文は二言語（Spec 35）。**日本語は 1 バイトも変えていない**
    // ので、既存の golden はそのまま緑のまま。
    let language = shared
        .world
        .read()
        .await
        .language()
        .unwrap_or(crate::world::Language::Ja);
    if incoming.attachments.len() > image_attachments.len() {
        context.push(match language {
            crate::world::Language::Ja =>
                "【添付】この発話には画像が添付されていましたが、保持期間を過ぎて\
                 削除されたため読み込めませんでした。画像は見えていない前提で\
                 答えてください。"
                    .to_owned(),
            crate::world::Language::En =>
                "[Attachment] This message had an attachment, but it could not be \
                 loaded because it passed the retention period. Answer on the \
                 assumption that you cannot see it."
                    .to_owned(),
        });
    }
    // **添付の事実を文字列として残す**（2026-08-06 利用者裁定。実機で穴が出た）。
    //
    // D1 は画像だけでなく**「画像があったという事実」ごと**履歴から消していた —
    // 履歴に積まれるのは畳んだ文字列で、そこに添付の記述が無いため、次のターンの
    // モデルは**自分が書いた説明文は覚えているのに、画像という物があったことを
    // 知らない**。実機では「画像を貼る → 話す → これを誰々に見せて」という
    // 最も自然な流れで、転送先が理由の書かれていない本文だけを受け取った
    // （D6 の断り書きは「そのターンの受信に添付があるか」で門を張っており、
    // 添付と転送依頼が別ターンになると鳴らない）。
    //
    // **この 1 行は `context` へ入るので `sent_user_turn` の一部になり、
    // #45 の規律でそのまま履歴へ残る。** 画像そのものは入らないので D1 は不変。
    if !image_attachments.is_empty() {
        // 種別語で書く（Spec 36）。D5 で 1 発話 1 件なので先頭を見れば足りる。
        let kind = image_attachments[0].kind();
        let label = super::attachment_kind_label(kind, language);
        context.push(match language {
            crate::world::Language::Ja => format!(
                "【添付】この発話には{label}が {} 件付いています。\
                 **{label}が渡るのはこのターンだけ**で、次のターン以降はあなたからは\
                 見えません（利用者の画面には残ります）。他のサーヴァントへ転送・委譲\
                 するときも{label}は渡りません。**後から「あの{label}を見せて」と頼まれたら、\
                 もう手元に無いことを伝え、宛先を指定して貼り直すよう案内してください。**",
                image_attachments.len()
            ),
            crate::world::Language::En => format!(
                "[Attachment] This message carries {} {label} attachment(s). \
                 **The {label} reaches you only on this turn** — from the next turn \
                 on you cannot see it (it stays visible to the user). It is not \
                 forwarded when you hand off or delegate to another servant either. \
                 **If you are later asked to show that {label}, say that you no longer \
                 have it and ask the user to attach it again, addressed to the \
                 servant who needs it.**",
                image_attachments.len()
            ),
        });
    }

    // 可変の文脈と今回の受信を **1 本の user 発話**に畳んで送る。
    //
    // **送った文字列をそのまま履歴へ積む**（下の push_exchange へ渡す）。
    // 当初は `attributed` だけを積んで「今回だけの文脈を履歴へ焼き付けない」
    // ようにしたが、それは**送信と保存の食い違い**を作る。次のターンでは履歴側の
    // 短い文字列がその位置に来るので、**前方一致がそこで切れる** — 以後どれだけ
    // 会話が伸びてもキャッシュは system + tools で頭打ちになる（failures.md #45）。
    //
    // 揃えるほうが記録としても正しい。エージェントは実際にその文脈込みで受け取って
    // おり、`attributed` だけを積むのは受け取った内容についての嘘になる。
    //
    // 画像は文字列ではないので畳めない — `ChatMessage.attachments` の席で運び、
    // adapter が画像ブロックへ組む（テキストより前）。履歴へ積まれるのは
    // `sent_user_turn` の文字列だけなので、**画像は履歴に残らない**（D1）。
    context.push(attributed.clone());
    let sent_user_turn = context.join("\n\n");
    messages.push(ChatMessage::user_with_attachments(
        &sent_user_turn,
        image_attachments,
    ));

    // 指紋は**組み終わってから**取る。adapter と同じ畳み方で数えないと、
    // 実際に前方一致の先頭を占める文字列とは別物を測ることになる。
    let system_digest = SystemDigest::of(&messages, stable_len);
    let history_depth = HistoryDepth {
        msgs: history_msgs,
        limit: shared.config.history_turns.saturating_mul(2),
    };
    TurnPrompt {
        messages,
        sent_user_turn,
        system_digest,
        history_depth,
    }
}

/// ターンの出口（段 8）。答えを記録し、宛先へ配送する。
///
/// **`handle_message` から切り出したのは、ここが「どこへ返すか」だけを決める
/// 独立した段だから** — 入力は [`TurnProduct`] と受信の封筒だけで、プロンプトも
/// ツールもモデルも見ない。今日の実機の事故（1 つの依頼が 2 本に分裂する）は
/// この段の宛先の決まり方そのもので、**ここだけを読めば追える形**にしてある。
async fn dispatch_outcome(
    shared: &Arc<Shared>,
    agent_id: &AgentId,
    incoming: &AgentMessage,
    reply_to: Option<tokio::sync::oneshot::Sender<Reply>>,
    product: TurnProduct,
    budget: Option<Arc<crate::budget::BudgetPool>>,
    participants: Option<Participants>,
) -> CoreResult<()> {
    let TurnProduct {
        outcome,
        tokens,
        mut grounding,
        mut reasoning_summary,
    } = product;
    let next_hop = incoming.hop.saturating_add(1);
    let from = Endpoint::Agent {
        id: agent_id.clone(),
    };

    let deliveries = match &outcome {
        Outcome::Finish { content } => {
            // 会話はここで終わり。ただし**誰へ返すか**は、頼まれ方で決まる。
            // 委譲（ask）で来た発話なら答えは依頼主へ戻る。通常配送ならユーザーへ。
            let destination = match &reply_to {
                Some(_) => incoming.from.clone(),
                None => Endpoint::User,
            };
            // **答えがどこへ行ったか**を残す。これが無いと「委譲したのに
            // ユーザーへ返った」という報告を、ログから確かめる手段が無い
            // （実際に確かめられず、診断に何往復もかかった）。
            //
            // `to=user` なら `reply_to` が無かった = 転送で来た依頼だった証拠、
            // `to=<agent_id>` なら委譲が戻っている証拠。**判定の材料は宛先だけ**で、
            // 依頼文の中身を読む必要が無い。
            note!(
                "reply: agent={agent_id} to={} hop={next_hop} chars={}",
                endpoint_log_label(&destination),
                content.chars().count(),
            );
            let mut outgoing = AgentMessage::new(from, destination, content, next_hop);
            outgoing.tokens = tokens as u32;
            // 接地の来歴は発話に添えて表示層へ渡す（`MessageSent` が運ぶ）。
            // プロンプトへは戻らない — 組み立て側は `content` しか読まない。
            outgoing.grounding = grounding;
            // 思考の要約も同じ経路で表示層へ渡す。**履歴へは載らない** —
            // 積む先の `ChatMessage` にこの欄が無い（型で閉じている）。
            outgoing.reasoning_summary = reasoning_summary;
            shared.record(outgoing).await;

            if let Some(reply_to) = reply_to {
                // 受け取り手が既に諦めている（タイムアウト）ことはあるので、
                // 送信の失敗は無視する。こちらの処理は完了している。
                let _ = reply_to.send(Reply {
                    text: content.clone(),
                    kind: PlanTaskState::Answered,
                });
            }
            return Ok(());
        }
        Outcome::Handoff { deliveries } => deliveries,
    };

    // **転送はどのログ行にも出ていなかった。**`transfer_to_*` は `tool:` 行を
    // 出す前にループを抜けるので（上の `Outcome::Handoff` の break）、
    // **`name=transfer_to_*` で grep しても構造的に 0 件しか返らない** —
    // 「使われていない」と「見えていない」が同じ 0 に畳まれていた
    // （`failures.md` #90 の再演。肯定の対照を `ask_*` で取ったが、
    // **対照を取った家族が違った**）。
    note!(
        "handoff: agent={agent_id} to={} hop={next_hop}",
        deliveries
            .iter()
            .map(|(to, _)| to.to_string())
            .collect::<Vec<_>>()
            .join(","),
    );

    // 委譲（ask / plan）で来た依頼に、答えを返さず**転送で応じた**場合。
    //
    // `reply_to` は上の `Finish` 分岐でしか使われないため、ここで何もしないと
    // 送信側が drop されるだけになり、依頼主は「相手から答えが返りませんでした。」
    // を読む。**これは嘘である** — 答えは返っており、宛先が違うだけで会話は
    // 第三者へ渡っている（そして最終的にユーザーへ流れる）。
    //
    // 転送そのものは抑制しない。ワーカーの正当な選択を握り潰すと、
    // 「呼んだのに何も起きない」という別の穴に変わる。直すのは文言だけ。
    if let Some(reply_to) = reply_to {
        let names = {
            let world = shared.world.read().await;
            deliveries
                .iter()
                .map(|(to, _)| {
                    world
                        .agent(to)
                        .map(|record| record.spec.name.clone())
                        .unwrap_or_else(|_| to.to_string())
                })
                .collect::<Vec<_>>()
                .join("、")
        };
        let _ = reply_to.send(Reply {
            text: format!(
                "相手はこの依頼に自分で答えず、{names} へ会話を渡しました。\
                 答えはこちらへ戻りません。必要なら別の相手に頼むか、自分で進めてください。"
            ),
            kind: PlanTaskState::HandedOff,
        });
    }

    // 宛先ごとに 1 通として記録する（fan-out）。トークンは 1 ターンぶんの消費なので、
    // 全通に載せると宛先数で二重計上される。先頭の 1 通にだけ載せる。
    //
    // 断り書きは記録時の言語で書く（Spec 35 D6）。全通で同じ値なのでループの外。
    let language = shared
        .world
        .read()
        .await
        .language()
        .unwrap_or(crate::world::Language::Ja);
    let dropped_kind = incoming.attachments.first().map(|a| a.kind());
    let mut queued = Vec::with_capacity(deliveries.len());
    for (index, (to, message)) in deliveries.iter().enumerate() {
        let mut outgoing = AgentMessage::new(
            from.clone(),
            Endpoint::Agent { id: to.clone() },
            // 依頼元のターンに添付が付いていたら、届かないことを本文で断る（D6）。
            note_dropped_attachment(message, dropped_kind, language),
            next_hop,
        );
        outgoing.tokens = if index == 0 { tokens as u32 } else { 0 };
        // 接地も 1 ターンぶんの事実なので、トークンと同じく先頭の 1 通にだけ載せる。
        // 全通に複製すると、表示で畳んだあとも同じ出典が宛先数ぶん並ぶ。
        if index == 0 {
            outgoing.grounding = std::mem::take(&mut grounding);
            // 要約も同じ理由で先頭の 1 通だけ（1 ターンぶんの事実）。
            outgoing.reasoning_summary = std::mem::take(&mut reasoning_summary);
        }

        // 同じ内容を複数宛先へ渡す fan-out は、受け手から見ればエージェント発の
        // 同報。宛先一覧を封筒に載せ、受け手同士が「相手はこれを知らない」と
        // 誤解して伝言し合う経路（ユーザー同報の反響と同型）を塞ぐ。
        // 内容が宛先ごとに違う配送は同報ではないので載せない —
        // 「全員が同じ内容を受け取っている」という注記が嘘になる。
        let same_content: Vec<AgentId> = deliveries
            .iter()
            .filter(|(_, m)| m == message)
            .map(|(t, _)| t.clone())
            .collect();
        if same_content.len() >= 2 {
            outgoing.co_recipients = same_content;
        }

        shared.record(outgoing.clone()).await;
        queued.push((to, outgoing));
    }

    // 燃料切れの判定は宛先共通（同じターン由来なので hop も同じ）。
    // 記録は済ませてから打ち切る——発話自体は起きたのだから、ログには残す。
    if next_hop >= shared.config.max_hops {
        shared.emit(CoreEvent::HopLimitReached {
            agent_id: agent_id.clone(),
            max_hops: shared.config.max_hops,
        });
        return Ok(());
    }

    // 転送失敗（宛先停止中・受信箱飽和）は、このエージェント自身の失敗ではない。
    // 自分を Failed に落とさず、事象として通知するに留める。
    // 1 宛先の失敗で残りを道連れにしない——枝は独立している。
    for (to, outgoing) in queued {
        // 転送・fan-out は**同じ因果の続き** — 予算は同一の Arc を引き継ぐ
        // （新しいプールを作った瞬間に天井が蒸発する。token_budget の pool）。
        // **参加者は引き継ぐが、転送先はここでは数に入らない** — 数えるのは
        // 「待って答えを返した」個体で、転送は待たない（Spec 28 D7）。
        // 引き継ぐのは、転送先がさらに `ask` で待ったときにその相手を拾うため。
        if let Err(err) = deliver(shared, to, outgoing, budget.clone(), participants.clone()).await
        {
            shared.emit(CoreEvent::AgentFailed {
                agent_id: to.clone(),
                error: ErrorPayload::from(&err),
            });
        }
    }

    Ok(())
}

/// 共有ツールと個別 MCP ツールを 1 つの集合へ畳む（純関数）。
///
/// 同名は個別が勝つ（上書き可能な加算）。順序は共有 → 個別で安定させる。
fn merge_tool_specs(shared_specs: Vec<ToolSpec>, personal: Vec<ToolSpec>) -> Vec<ToolSpec> {
    if personal.is_empty() {
        return shared_specs;
    }
    let mut merged: Vec<ToolSpec> = shared_specs
        .into_iter()
        .filter(|spec| !personal.iter().any(|p| p.name == spec.name))
        .collect();
    merged.extend(personal);
    merged
}

/// エージェント別 MCP を接続して登録する（Spec 02）。
///
/// 読み込み失敗（外部編集で壊れた mcp.json = 失敗二分類 (1')）でも
/// エージェントの起動は止めない。個別ツール 0 本で稼働し、失敗理由は
/// [`AgentMcpStatus::load_error`] として読める。
pub(super) async fn connect_agent_mcp(shared: &Shared, id: &AgentId) {
    let state = match shared.store.read_agent_mcp_config(id).await {
        Ok(config) => AgentMcpState {
            manager: crate::mcp::McpManager::connect_all(&config).await,
            load_error: None,
        },
        Err(err) => AgentMcpState {
            manager: crate::mcp::McpManager::default(),
            load_error: Some(err.to_string()),
        },
    };
    shared.agent_mcp.write().await.insert(id.clone(), state);
}

/// 本文が「テキストとして漏れたツール呼び出し」を含むか（計器用の純関数）。
///
/// モデルがネイティブの `tool_use` ブロックではなく本文へツール呼び出しの XML を
/// 書いてしまう既知の揺らぎがある。ハーネスから見ると「ツールを呼ばなかった」=
/// 最終出力なので、**生の XML がそのまま利用者へ配信され、ログには何も残らない**。
///
/// # 開始タグは既に食われている前提で見る
///
/// 実機の観測（2026-08-02、claude-opus-5 + MCP 27 本）では、届いた本文は
/// `MCP_DOCKER__fetch">\n<parameter name="url">…</parameter>\n</invoke>` の形で、
/// **先頭の `<invoke name="` が無い**（削ったのは API 側。こちらの adapter は
/// JSON のブロックを走査するだけで XML を解釈しない）。`<invoke` だけを探す
/// 検出器は実機で一度も発火しない — 閉じタグと `<parameter` を主に据える。
fn looks_like_leaked_tool_call(text: &str) -> bool {
    !leaked_markers(text).is_empty()
}

/// 漏れを検出した目印の一覧（計器に出す）。
///
/// **本文の代わりにこれを出す。** どの目印で当たったかが分かれば形は追えるし、
/// **モデルが書いた文章をログへ再放流しない** — そこには利用者が渡した秘密が
/// 入りうる（実機で `Authorization: Bearer …` を丸ごと書いた）。
///
/// `<invoke ` だけを探すと実機で一度も発火しない（届く本文は先頭の
/// `<invoke name="` が食われて途中から始まる）ので、3 つとも見る。
fn leaked_markers(text: &str) -> Vec<&'static str> {
    [
        ("</invoke>", "close"),
        ("<parameter name=", "param"),
        ("<invoke ", "open"),
    ]
    .into_iter()
    .filter(|(needle, _)| text.contains(needle))
    .map(|(_, label)| label)
    .collect()
}

/// 同梱ツールをこのエージェントへ提示するか（enabled_tools_invariant）。
///
/// - 同梱ツール以外（MCP 由来）は常に提示（このフィルタの対象外）
/// - 作業フォルダが要るツールは、未設定なら enabled_tools に関わらず
///   提示しない（自動除外が明示より優先。使えないツールを見せない）
/// - enabled_tools が None なら**既定集合**（`DEFAULT_ENABLED_TOOLS`）、
///   Some なら列挙分だけ
///
/// **`None` は「全同梱ツール」ではない**（Spec 15 の破壊的変更）。`run` だけが
/// 既定集合の外に居るので、更新しただけで実行能力が増えることはない。
fn is_bundled_tool_presented(name: &str, spec: &AgentSpec) -> bool {
    if !crate::tools::BUNDLED_TOOL_NAMES.contains(&name) {
        return true;
    }
    if crate::tools::WORK_DIR_TOOL_NAMES.contains(&name) && spec.work_dir.is_none() {
        return false;
    }
    match &spec.enabled_tools {
        None => crate::tools::DEFAULT_ENABLED_TOOLS.contains(&name),
        Some(enabled) => enabled.iter().any(|tool| tool == name),
    }
}

/// ツールを 1 本実行する。
///
/// 未知の名前でも `Err` にせず文字列を返すのは、モデルが読んで直せるようにするため。
/// ここで会話ごと落とすと、名前を打ち間違えただけでターンが終わる。
/// registry へ落ちた呼び出しの理由を決める（Spec 27）。
///
/// 返すのは（状態, **トリム後・切り詰め前**の文字数）。
///
/// **解決の順は [`execute_tool`] と同じ**（個別 MCP → 共有 registry）。
/// 順が食い違うと、**実行したツールと理由を引いたツールが別物になる**。
///
/// - 引けて `wants_reason` = 真 → 引数から読む（`Written` か `Omitted`）
/// - 引けて偽 → `Unsupported`（[`crate::mcp::McpTool`] だけ）
/// - 引けない → 呼び出し側の既定 `Excluded` のまま（ここへは来ない）
async fn registry_reason(
    shared: &Arc<Shared>,
    agent_id: &AgentId,
    call: &crate::llm::ToolCall,
) -> (crate::tool_reason::ReasonState, usize) {
    let personal = {
        let map = shared.agent_mcp.read().await;
        map.get(agent_id).and_then(|state| {
            state
                .manager
                .tools()
                .iter()
                .find(|tool| tool.name() == call.name)
                .cloned()
        })
    };
    let tool = match personal {
        Some(tool) => Some(tool),
        None => shared.tools.read().await.get(&call.name).cloned(),
    };
    match tool {
        Some(tool) if tool.wants_reason() => crate::tool_reason::read(&call.args),
        Some(_) => (crate::tool_reason::ReasonState::Unsupported, 0),
        None => (crate::tool_reason::ReasonState::Excluded, 0),
    }
}

async fn execute_tool(
    shared: &Arc<Shared>,
    agent_id: &AgentId,
    call: &crate::llm::ToolCall,
    cancel: &tokio_util::sync::CancellationToken,
) -> CoreResult<String> {
    // 実行解決は提示と同じ規則の逆引き: **個別 MCP を先に**引き、
    // 無ければ共有 registry（同名は個別が勝つ）。個別ツールは registry に
    // 入っていないため、他エージェントからは名前を知っていても実行できない。
    let personal = {
        let map = shared.agent_mcp.read().await;
        map.get(agent_id).and_then(|state| {
            state
                .manager
                .tools()
                .iter()
                .find(|tool| tool.name() == call.name)
                .cloned()
        })
    };
    let tool = match personal {
        Some(tool) => Some(tool),
        None => shared.tools.read().await.get(&call.name).cloned(),
    };

    let Some(tool) = tool else {
        return Ok(format!(
            "`{}` というツールはありません。提示された名前から選んでください。",
            call.name
        ));
    };

    // 作業フォルダ（grep / diff の探索範囲）と宣言フォルダ（rag）は
    // 呼び出しの瞬間に解決する。ツール登録時に固定すると、設定変更が
    // 次の再登録まで効かない。
    let (work_dir, rag_roots, language) = {
        let world = shared.world.read().await;
        let record = world.agent(agent_id).ok();
        (
            record
                .as_ref()
                .and_then(|record| record.spec.work_dir.clone())
                .map(std::path::PathBuf::from),
            record
                .map(|record| {
                    record.spec.rag_sources.iter().map(std::path::PathBuf::from).collect()
                })
                .unwrap_or_default(),
            world.language().unwrap_or(crate::world::Language::Ja),
        )
    };

    let ctx = ToolContext {
        agent_id: agent_id.clone(),
        work_dir,
        rag_roots,
        language,
        // ターンのトークンを渡す。**見るのは `run` だけ**（外部プロセスを
        // 起動するツールは、周回境界まで待つと最長 1 時間走り続ける）。
        // Spec 10 の不変条件 1（検査点は周回境界だけ）はターンループの話で、
        // 葉で 1 箇所見ることはその構造を変えない。
        cancel: Some(cancel.clone()),
    };
    tool.call(&ctx, &call.args).await
}


#[cfg(test)]
mod tests {
    use super::*;

    fn spec_named(name: &str) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: String::new(),
            parameters: serde_json::json!({ "type": "object" }),
        }
    }

    /// 漏れたツール呼び出しの検出は、**開始タグが食われた実物の形**で発火すること。
    ///
    /// 2026-08-02 の実機ログがこの形。`<invoke` だけを探す検出器はここで
    /// 素通りし、計器としては存在しないのと同じになる。
    #[test]
    fn the_leak_detector_fires_on_the_shape_actually_observed() {
        let observed = "MCP_DOCKER__fetch\">\n\
                        <parameter name=\"url\">https://news.yahoo.co.jp/topics/top-picks</parameter>\n\
                        <parameter name=\"max_length\">4000</parameter>\n\
                        </invoke>";
        assert!(
            looks_like_leaked_tool_call(observed),
            "先頭の <invoke name=\" が無い実物で発火すること"
        );

        // 開始タグが残っている形でも発火する。
        assert!(looks_like_leaked_tool_call(
            "<invoke name=\"fetch\"><parameter name=\"url\">x</parameter></invoke>"
        ));
    }

    /// 普通の答えでは発火しないこと（誤検出はログを無意味にする）。
    #[test]
    fn the_leak_detector_stays_quiet_on_ordinary_answers() {
        assert!(!looks_like_leaked_tool_call(
            "調べました。fetch で取得した結果は次のとおりです。\n\n- 1 件目\n- 2 件目"
        ));
        // コードブロック中の HTML/XML は普通に出てくるが、ツール呼び出しの形では
        // ないので黙っていること。
        assert!(!looks_like_leaked_tool_call(
            "```html\n<div class=\"x\"><span>値</span></div>\n```"
        ));
    }

    /// 同名は個別が勝つ（上書き可能な加算）。順序は共有 → 個別。
    #[test]
    fn personal_tools_override_shared_ones_by_name() {
        let shared = vec![spec_named("grep"), spec_named("memo__recall")];
        let personal = vec![spec_named("memo__recall"), spec_named("memo__store")];

        let merged = merge_tool_specs(shared, personal);
        let names: Vec<&str> = merged.iter().map(|s| s.name.as_str()).collect();

        assert_eq!(names, vec!["grep", "memo__recall", "memo__store"]);
        assert_eq!(
            merged.iter().filter(|s| s.name == "memo__recall").count(),
            1,
            "同名は 1 本に畳まれ、個別側が残る"
        );
    }

    /// 同じ呼び出し + 同じ結果が 2 回続いたら、3 回目は実行しない。
    #[test]
    fn the_third_identical_call_is_blocked() {
        let args = serde_json::json!({ "path": "README.en.md" });
        let err = "`README.en.md` を読めません: 見つかりません";
        let mut guard = RepeatGuard::default();

        assert!(!guard.blocks("file", &args), "1 回目は素通し");
        guard.observe("file", &args, err);
        assert!(!guard.blocks("file", &args), "2 回目も素通し（1 回では判定しない）");
        guard.observe("file", &args, err);

        assert!(guard.blocks("file", &args), "3 回目は実行しない");
        assert_eq!(guard.repeats("file", &args), 2);
    }

    /// **間に別の呼び出しが挟まっても数えは切れない。**
    ///
    /// 隣接だけを見ていた最初の実装は、実機で 1 件も発火しなかった
    /// （2026-07-31 のログ: `file(A)` が round 24・25・28 に出たが、26 の
    /// `grep` で数えが切れて 3 回目が素通しした）。
    #[test]
    fn an_interleaved_call_does_not_clear_the_count() {
        let sd = serde_json::json!({ "pattern": "a", "replacement": "b" });
        let grep = serde_json::json!({ "pattern": "a" });
        let mut guard = RepeatGuard::default();

        guard.observe("sd", &sd, "対象がありません");
        guard.observe("grep", &grep, "一致なし");
        guard.observe("sd", &sd, "対象がありません");

        assert!(
            guard.blocks("sd", &sd),
            "呼び出しごとに数えるので、挟まれても 2 回は 2 回"
        );
        assert!(!guard.blocks("grep", &grep), "挟まった側は 1 回のまま");
    }

    /// 1 周に並列で複数本呼ばれても同じ（実機の主な形）。
    ///
    /// 2026-07-31 のログ: round 2 と round 3 で同じ `file(B)` が呼ばれ、
    /// round 3 は 3 本の並列呼び出しだった。隣接判定はここで必ず切れる。
    #[test]
    fn parallel_calls_in_one_round_do_not_clear_the_count() {
        let target = serde_json::json!({ "op": "read", "path": "README.md" });
        let other = serde_json::json!({ "op": "read", "path": "CLAUDE.md" });
        let third = serde_json::json!({ "op": "read", "path": "failures.md" });
        let body = "（12,045 字の本文）";
        let mut guard = RepeatGuard::default();

        // round 2
        guard.observe("file", &target, body);
        // round 3（並列 3 本。同じ読み直しが 1 本目に混ざる）
        guard.observe("file", &target, body);
        guard.observe("file", &other, "別の本文");
        guard.observe("file", &third, "また別の本文");

        assert!(guard.blocks("file", &target), "round 5 の 3 回目は実行しない");
        assert!(!guard.blocks("file", &other), "他の読み込みは巻き添えにしない");
    }

    /// 成功でも同じことが起きる。同じ入力に同じ出力が返るなら 3 回目に新しい
    /// 情報は無い（同梱ツールは失敗も `Ok` の本文で返すため、失敗と成功を
    /// 区別する材料がそもそも無い）。
    #[test]
    fn identical_successes_are_blocked_too() {
        let args = serde_json::json!({ "pattern": "fn main" });
        let mut guard = RepeatGuard::default();

        guard.observe("grep", &args, "src/main.rs:1: fn main() {");
        guard.observe("grep", &args, "src/main.rs:1: fn main() {");

        assert!(guard.blocks("grep", &args));
    }

    /// 結果が変われば止めない。**同じ操作が実を結んでいる**（追記が進む・
    /// 待っていた状態が変わる）ので、繰り返し自体は正当。
    ///
    /// 隣接を捨てた後もここは守る必要がある。「呼び出しごとの通算回数」で
    /// 数えると、実を結んでいる追記まで 3 回目で止まってしまう。
    #[test]
    fn a_changed_result_clears_the_count() {
        let args = serde_json::json!({ "op": "append", "path": "log.md" });
        let mut guard = RepeatGuard::default();

        guard.observe("file", &args, "1 行追記しました。");
        guard.observe("file", &args, "1 行追記しました。");
        guard.observe("file", &args, "2 行追記しました。");

        assert!(!guard.blocks("file", &args), "結果が変わったら数え直す");
        assert_eq!(guard.repeats("file", &args), 1);
    }

    /// 引数が変われば別の呼び出しとして数える。**別の場所を試している**のは
    /// 行き詰まりではない。数えは呼び出しごとに独立しているので、
    /// 一方を止めてももう一方は素通しする。
    #[test]
    fn each_argument_is_counted_independently() {
        let first = serde_json::json!({ "path": "a.md" });
        let second = serde_json::json!({ "path": "b.md" });
        let err = "読めません";
        let mut guard = RepeatGuard::default();

        guard.observe("file", &first, err);
        guard.observe("file", &first, err);
        guard.observe("file", &second, err);

        assert!(!guard.blocks("file", &second), "宛先を変えた失敗は繰り返しではない");
        assert!(guard.blocks("file", &first), "止まっている側だけを止める");
    }

    /// 引数の一致はキーの並びに依存しない（`serde_json::Value` の等価は
    /// 中身で決まる）。プロバイダが並べ替えて返しても同じ呼び出しと数える。
    #[test]
    fn argument_equality_ignores_key_order() {
        let a = serde_json::json!({ "op": "read", "path": "x.md" });
        let b = serde_json::json!({ "path": "x.md", "op": "read" });
        let err = "読めません";
        let mut guard = RepeatGuard::default();

        guard.observe("file", &a, err);
        guard.observe("file", &b, err);

        assert!(guard.blocks("file", &a), "キーの並びで別物にしない");
    }
}
