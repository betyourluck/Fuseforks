//! 起動と復元。
//!
//! **起動時に自動起動はしない**（batch_start_invariant）— アプリを開いた時点で
//! 誰も走らない。開いただけで課金が始まる作りにしない、が凍結された理由。
//!
//! 会話は**開き直す**（Spec 12）。`sessions.redb` の exchange から履歴を
//! 復元し、要約は履歴の席を持たずに可変文脈へ差す。**会話ログからは復元できない** —
//! 履歴には「送った文字列そのもの」（畳んだ可変文脈込み）が入り、その文字列は
//! Shared.log のどこにも無い（#45）。会話ログだけ戻すと、画面は正しいのに
//! 全員が健忘症で始まる。

use super::*;

impl Orchestrator {
    /// ワークスペースから状態を復元してオーケストレーターを起動する。
    ///
    /// エージェントは復元しても自動起動しない。起動は明示操作に限る
    /// （アプリを開いた瞬間に全エージェントが課金を始めるのを避ける）。
    pub async fn bootstrap(
        store: ConfigStore,
        factory: Arc<dyn BackendFactory>,
        secrets: Arc<dyn SecretStore>,
        config: OrchestratorConfig,
    ) -> CoreResult<Self> {
        // world.json の有無は load の**前**に見る（load は不在を空の世界として
        // 返すため、後からは新規と空を区別できない）。
        let fresh_world = !store.world_exists();
        let persisted = store.load_world().await?;
        let mut world = World::from_persisted(persisted.clone());

        // トークン予算の既定値は**新規の村にだけ**書く（Spec 11 の ceiling 契約。
        // 既存の村へ黙って天井を足すと、昨日まで完走していた依頼が今日から
        // 止まる — それはパッチでやってよい変更ではない）。下の正規化
        // 書き戻しが world.json への実書き込みを担う。
        if fresh_world {
            world.set_token_budget(Some(crate::budget::DEFAULT_CEILING));
        }

        // 言語が未確定（新規の村 / 追記前の村 / 手編集の不正値）なら OS から
        // 確定する（Spec 13 の settings_contract — 「自動」の選択肢は無く、
        // 初回に ja / en のどちらかへ確定して保存する）。下の正規化書き戻しが
        // world.json への実書き込みを担う。コアはこの値で分岐しない。
        if world.language().is_none() {
            world.set_language(crate::world::Language::from_os_locale(
                sys_locale::get_locale().as_deref(),
            ));
        }

        // 「unset なのに秘密が実在する」テンプレートは keyring へ昇格させる。
        // clear_credential は秘密の削除と unset への遷移を一体で行うので、
        // この組み合わせは正規の操作では作れない——過去の巻き戻り事故（failures.md #16）が
        // ディスクへ固定された状態である。放置するとユーザーはキーを貼り直すまで
        // 接続できず、しかもテンプレートの画面は「登録済み」と表示する（矛盾が見えない）。
        // 資格情報ストアが応答しない場合は昇格を見送る。起動を止めるほどの事態ではなく、
        // 次回起動時にまた試せばよい。
        for mut template in world.templates() {
            if template.credential == CredentialSource::Unset
                && secrets.contains(template.id.as_str()).unwrap_or(false)
            {
                template.credential = CredentialSource::Keyring;
                world.upsert_template(template);
            }
        }

        // 読み込み時の正規化（平文の秘密の除去、宙に浮いた接続の切り離し）で内容が
        // 変わったなら、その場でファイルへ書き戻す。次の編集操作まで待つと、
        // ユーザーが何もしない限りディスク上に古い内容——場合によっては秘密——が残る。
        let normalized = world.to_persisted();
        if normalized != persisted {
            store.save_world(&normalized).await?;
        }

        // 天井なしの村は起動のたびに WARN で可視化する（安全装置は opt-in でも、
        // 危険な状態を警告で見せれば実質 opt-out に近づく）。次の道を書く —
        // 警告だけ出して直し方を言わないのは #44 で払った代償の再演になる。
        if world.token_budget().is_none() {
            note!(
                "WARN token budget: この村に天井がありません — world.json の \
                 tokenBudget に 1000000（推奨）を設定すると、依頼 1 つあたりの\
                 トークン消費に自動の上限が掛かります"
            );
        }

        // 予定の読み込み（Spec 07）。ファイル全体が読めない場合も起動は止めない
        // （設定を直す画面へ到達できなくなる。mcp.json と同じ判断）が、
        // 書き込みは拒否する — 上書きすると直せば戻ったはずの予定が消える。
        let (schedules, schedules_blocked) = match store.load_schedules().await {
            Ok(loaded) => {
                for reason in &loaded.dropped {
                    note!("schedule: {reason}");
                }
                // 宛先が存在しない予定はここで落とす（World::from_persisted が
                // 宙に浮いた接続を落とすのと同じ規律）。ディスクへの反映は
                // 次の保存に任せる — 起動時に書き戻すほどの緊急性（秘密の残留）が無い。
                let (kept, dangling): (Vec<_>, Vec<_>) = loaded
                    .tasks
                    .into_iter()
                    .partition(|task| world.agent(&task.to).is_ok());
                for task in &dangling {
                    note!(
                        "schedule: 宛先 {} が存在しないため予定 {} を落としました",
                        task.to, task.id
                    );
                }
                (kept, None)
            }
            Err(err) => {
                note!(
                    "schedule: schedules.json が読めないため予定なしで起動します\
                     （書き込みは保護のため拒否されます）: {err}"
                );
                (Vec::new(), Some(err.to_string()))
            }
        };

        // 会話の保存先を開き、最後に開いていた会話を戻す（Spec 12）。
        // **開けなくても起動は止めない** — 会話が戻らないことは、アプリが開かない
        // 理由にならない（D1 のフォールバックと同じ規律）。
        let (sessions, session_id, restored_log, restored_summaries) =
            match SessionStore::open(store.root().join("sessions.redb")) {
                Ok(sessions) => {
                    match open_session_at_boot(
                        &sessions,
                        &mut world,
                        config.log_capacity,
                        config.history_turns,
                    ) {
                        Some((id, log, summaries)) => (Some(sessions), id, log, summaries),
                        None => (None, String::new(), Vec::new(), BTreeMap::new()),
                    }
                }
                Err(err) => {
                    note!(
                        "WARN session store: 会話を保存できません（この起動では会話は\
                         再起動で消えます）— {err}"
                    );
                    (None, String::new(), Vec::new(), BTreeMap::new())
                }
            };

        let (events, _) = broadcast::channel(config.event_capacity);

        // 添付の置き場（Spec 23）。起動時に保持期間と容量の GC を掛ける（D9）。
        // 失敗しても起動は止めない — 消せなかった古いファイルは次の起動でまた
        // 候補になるだけで、会話の正しさには関わらない。
        let attachments = crate::attachment::AttachmentStore::new(store.root());
        match attachments.gc(std::time::SystemTime::now()).await {
            Ok(report) if report.removed > 0 || report.remaining_files > 0 => {
                // 種別内訳を併記する（Spec 36 D11）— 種別クォータを作らない
                // 代わりの計器で、動画が総量を食って画像を押し出す事態が
                // 実際に起きているかをここで観測する。0 も出す（#72）。
                note!(
                    "attachment gc: removed={} remaining={} bytes={} kinds={}",
                    report.removed,
                    report.remaining_files,
                    report.remaining_bytes,
                    report.kinds_line(),
                );
            }
            Ok(_) => {}
            Err(err) => note!("attachment gc failed: {err}"),
        }

        // 村の識別子（Spec 28）。**予定のティッカーが回り始める前**に確定させる —
        // 発火の途中で解決すると、識別子が未確定の窓で承認鍵が組めない。
        // 読めない・書けない村では空文字にして続ける（**空は承認鍵が一致しない側**
        // なので、前判定が走らなくなるだけで危険側へは倒れない）。
        let village_id = match store.village_id().await {
            Ok(id) => id,
            Err(err) => {
                note!("WARN village_id を確定できません（前判定は実行されません）: {err}");
                String::new()
            }
        };

        let shared = Arc::new(Shared {
            world: RwLock::new(world),
            mailboxes: RwLock::new(HashMap::new()),
            events,
            factory,
            backends: RwLock::new(HashMap::new()),
            secrets,
            store,
            attachments,
            log: RwLock::new(restored_log),
            tools: RwLock::new(ToolRegistry::new()),
            mcp: RwLock::new(crate::mcp::McpManager::default()),
            agent_mcp: RwLock::new(HashMap::new()),
            schedules: RwLock::new(schedules),
            probe_approvals: RwLock::new(None),
            village_id: RwLock::new(village_id),
            plan_waves: RwLock::new(PlanWaveStore::default()),
            wave_runs: Mutex::new(HashMap::new()),
            turns: Mutex::new(HashMap::new()),
            turn_seq: std::sync::atomic::AtomicU64::new(1),
            schedules_blocked,
            sessions,
            summaries: RwLock::new(restored_summaries),
            session_id: std::sync::RwLock::new(session_id),
            external_gate: tokio::sync::Semaphore::new(1),
            config,
        });

        let stats_task = spawn_stats_ticker(Arc::downgrade(&shared));
        let schedule_runtime = Arc::new(ScheduleRuntime::default());
        let schedule_task = spawn_schedule_ticker(
            Arc::downgrade(&shared),
            Arc::clone(&schedule_runtime),
            shared.events.subscribe(),
        );

        Ok(Self {
            shared,
            tasks: Mutex::new(HashMap::new()),
            stats_task,
            schedule_runtime,
            schedule_task,
        })
    }
}

/// 起動時に開くセッションを決め、会話ログと履歴を戻す（Spec 12 P2）。
///
/// 既定は**最新セッション**（`updatedAt` で判定）。読めない・0 件なら警告を
/// 1 行出して新規セッションを作る — **起動が止まる経路は作らない**（D1）。
/// セッションを 1 つも用意できなかったときだけ `None` を返し、呼び出し側は
/// 保存なしで起動する。
///
/// 復元は 2 層を**別々に**戻す。会話ログ（`Shared.log`）は末尾 `log_capacity` 件、
/// 履歴（`AgentRecord.history`）は `exchange` から `history_turns` 往復。
/// **片方から他方は作れない** — 会話ログだけ戻すと画面は正しいのに全員が
/// 健忘症で始まり、その 2 つは画面上区別が付かない。
fn open_session_at_boot(
    sessions: &SessionStore,
    world: &mut World,
    log_capacity: usize,
    history_turns: usize,
) -> Option<(String, Vec<AgentMessage>, BTreeMap<AgentId, String>)> {
    let existing = match sessions.latest_session() {
        Ok(found) => found,
        Err(err) => {
            note!("WARN session store: 会話の一覧を読めませんでした（新しい会話で始めます）: {err}");
            None
        }
    };

    let session_id = match existing {
        Some(id) => id,
        None => match sessions.create_session(None) {
            Ok(id) => id,
            Err(err) => {
                note!(
                    "WARN session store: 会話を作れませんでした（この起動では会話は\
                     再起動で消えます）— {err}"
                );
                return None;
            }
        },
    };

    // 会話ログ: リングと同じ形（末尾 log_capacity 件）で画面へ戻す。
    let log = match sessions.tail_messages(&session_id, log_capacity) {
        Ok(messages) => messages,
        Err(err) => {
            note!("WARN session store: 会話ログを読めませんでした（画面は空で始まります）: {err}");
            Vec::new()
        }
    };

    // 履歴: ここが S1 の本丸。画面ではなく、次のターンで LLM へ渡る側。
    let mut restored_agents = 0usize;
    let mut orphaned = 0usize;
    let mut summaries = BTreeMap::new();
    match sessions.restore_histories(&session_id, history_turns) {
        Ok(restored) => {
            for (agent_id, history) in restored.histories {
                match world.agent_mut(&agent_id) {
                    Ok(record) => {
                        record.history = history;
                        restored_agents += 1;
                    }
                    // 会話の後で消されたエージェント。履歴の行き先が無いので捨てる。
                    Err(_) => orphaned += 1,
                }
            }
            // 要約は履歴とは別の口で戻る（Spec 12 P4）。可変文脈へ差す側の材料で、
            // 履歴の中には置かない。
            summaries = restored.summaries;
        }
        Err(err) => {
            note!(
                "WARN session store: 履歴を復元できませんでした（エージェントは前回の\
                 話を覚えていない状態で始まります）: {err}"
            );
        }
    }
    if orphaned > 0 {
        note!("session: 復元した履歴のうち {orphaned} 体分は、該当エージェントが居ないため捨てました");
    }
    note!(
        "session: {session_id} を開きました（発話 {} 件 / 履歴 {restored_agents} 体 / \
         要約 {} 体）",
        log.len(),
        summaries.len()
    );

    Some((session_id, log, summaries))
}

/// 稼働統計を定期的に押し出すタスクを起こす。
///
/// `Weak` を握るのは、このタスクが [`Orchestrator`] の生存を延ばさないようにするため。
/// `Arc` を持たせると、オーケストレーターを捨ててもティッカーが動き続ける。
fn spawn_stats_ticker(shared: Weak<Shared>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let interval = match shared.upgrade() {
            Some(s) => s.config.stats_interval,
            None => return,
        };
        let mut ticker = tokio::time::interval(interval);

        loop {
            ticker.tick().await;
            let Some(shared) = shared.upgrade() else {
                return;
            };

            let world = shared.world.read().await;
            for snapshot in world.snapshots() {
                if snapshot.status.is_active() {
                    shared.emit(CoreEvent::AgentStatsUpdated {
                        agent_id: snapshot.id,
                        uptime_secs: snapshot.uptime_secs,
                        total_tokens: snapshot.total_tokens,
                        prompt_tokens: snapshot.prompt_tokens,
                        cached_tokens: snapshot.cached_tokens,
                        last_prompt_tokens: snapshot.last_prompt_tokens,
                    });
                }
            }
        }
    })
}
