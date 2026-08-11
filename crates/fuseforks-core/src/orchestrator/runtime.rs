//! 稼働と入口。start / stop / 打ち切り / 発話の受け口。
//!
//! **failed は idle と同じ停止側**で、start_agent で再稼働できる。
//! ここを塞ぐと、画面のトグルは OFF に見えているのにアプリを再起動するまで
//! 戻せない状態になる（failures.md #78）。畳むのは次の start_agent の冒頭
//! （reap）で、JoinHandle::is_finished が「登録がある」と「走っている」を分ける。
//!
//! **外部からの依頼は同時 1 本まで**（Spec 25 D7）。外部入口は新しい因果の根で、
//! hop も予算も扉を通るたびに新品になるため、既存の歯止めは根の中でしか効かない。
//! 上限 1 なら 2 周目が必ず busy に当たり、閉路・併走の予算二重消費・
//! デッドロックの 3 つを 1 つの機構で塞ぐ。

use super::*;

impl Orchestrator {
    /// エージェントを起動する。
    ///
    /// # Errors
    /// - 未登録なら [`CoreError::AgentNotFound`]
    /// - 既に稼働中なら [`CoreError::AlreadyRunning`]
    pub async fn start_agent(&self, id: &AgentId) -> CoreResult<()> {
        let mut tasks = self.tasks.lock().await;
        if let Some(existing) = tasks.get(id) {
            // 稼働中なら断る。**ただし「登録がある」と「走っている」は別**
            // （失敗して自分から降りたタスクの登録は残る）。
            if !existing.join.is_finished() {
                return Err(CoreError::AlreadyRunning {
                    agent_id: id.to_string(),
                });
            }

            // 失敗して降りた残骸をここで回収する（reap）。`agent_loop` は
            // `tasks` に手が届かない（Orchestrator が持つ）ので自分の登録を
            // 消せず、停止経路の `stop_agent` に当たる後始末が失敗経路には
            // 無かった。回収しないと ON が `AlreadyRunning` で弾かれ、
            // **画面のトグルは OFF に見えたままアプリを再起動するまで戻せない**。
            if let Some(dead) = tasks.remove(id) {
                let _ = dead.join.await; // 完了済みなので即座に返る
            }

            // `stop_agent` が join のあとに畳むものを、ここで畳む。
            // 個別 MCP を落とさずに起動し直すと、子プロセスが 1 世代ぶん残る。
            if let Some(state) = self.shared.agent_mcp.write().await.remove(id) {
                state.manager.shutdown().await;
            }
            let had_error = {
                let mut world = self.shared.world.write().await;
                match world.agent_mut(id) {
                    Ok(record) => {
                        // 失敗した瞬間までを稼働時間に含める。畳まないと
                        // `started_at` が残り、停止しているのにカードの
                        // 稼働時間が増え続ける。
                        if let Some(started) = record.started_at.take() {
                            record.accumulated_uptime_secs += started.elapsed().as_secs();
                        }
                        record.last_error.is_some()
                    }
                    Err(_) => false,
                }
            };

            // 回収したことを 1 行残す。**これが無いと、失敗からの復帰は
            // 「再起動の行が無いのに次のターンが始まっている」という
            // 不在からの推測でしか読めない** — 沈黙を根拠に使う形になる
            // （failures.md #77 の一般化 1）。
            note!("agent reaped: agent={id} had_error={had_error}");
        }

        {
            // 起動前に定義とテンプレートの整合を確認する。
            // 起動してから最初の発話で落ちるより、ここで断るほうが原因が分かりやすい。
            let world = self.shared.world.read().await;
            let record = world.agent(id)?;
            world.template(&record.spec.model_template_id)?;
        }

        self.shared.set_status(id, AgentStatus::Starting).await;

        // エージェント別 MCP を接続する（Spec 02）。接続寿命は稼働に一致。
        // 読み込み失敗・接続失敗でも起動は止めない — 状態として保持され、
        // agent_mcp_status で読める（共通 MCP と同じ規律）。
        connect_agent_mcp(&self.shared, id).await;

        let (mailbox_tx, mailbox_rx) = mpsc::channel(self.shared.config.mailbox_capacity);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        // 受信箱を先に公開してからタスクを起こす。逆順だと、起動直後の
        // 送信が「稼働中なのに宛先が無い」で落ちる窓ができる。
        self.shared
            .mailboxes
            .write()
            .await
            .insert(id.clone(), mailbox_tx);

        {
            let mut world = self.shared.world.write().await;
            if let Ok(record) = world.agent_mut(id) {
                record.started_at = Some(std::time::Instant::now());
                record.last_error = None;
                // **履歴は消さない**（Spec 12 で変更。それ以前はここで clear していた）。
                //
                // 旧い規律は「起動は新しい会話の開始として扱う」で、会話を始め直す
                // 手段が他に無かった時期の代用だった。会話の寿命がセッションに
                // なった今、始め直しは「新規チャット」= 新しいセッションが担う。
                // ここで消すと、**再起動して開いた会話の履歴を、エージェントを
                // 起動した瞬間に捨てることになる** — 起動時に自動起動しない契約と
                // 組み合わさって、S1（続きから始められる）が原理的に成立しない。
            }
        }

        let shared = Arc::clone(&self.shared);
        let agent_id = id.clone();
        let join = tokio::spawn(async move {
            agent_loop(agent_id, mailbox_rx, shutdown_rx, shared).await;
        });

        tasks.insert(
            id.clone(),
            TaskHandle {
                shutdown: shutdown_tx,
                join,
            },
        );
        drop(tasks);

        self.shared.set_status(id, AgentStatus::Running).await;
        Ok(())
    }

    /// 飛行中のターンを協調的に打ち切る（Spec 10）。
    ///
    /// 切るのは**ターン**であってエージェントではない — 稼働は降ろさず、
    /// 会話も履歴も消えず、次の封筒は普通に処理される。検知は周回境界
    /// （次の LLM 呼び出しの前）なので、飛行中の呼び出し・実行中のツールは
    /// 完走してから止まる。
    ///
    /// 飛行中のターンが無ければ**何もしない**（出口 2c — 「今の仕事を止めて」に
    /// 仕事が無いのは成功であって失敗ではない。エラーも通知も出さない）。
    /// 冪等 — 二重に呼んでも計測の起点（最初の要求時刻）は動かず、
    /// 出口の行も 1 本のまま（書くのは切られたターン自身なので）。
    pub async fn interrupt_turn(&self, id: &AgentId) {
        let handle = {
            let turns = self.shared.turns.lock().await;
            turns.get(id).map(Arc::clone)
        };
        let Some(handle) = handle else { return };

        // 計測の起点は最初の要求。連打で上書きすると「要求から N 秒」が縮んで、
        // 検知の遅さ（Notes 2 の判断材料）が実際より小さく記録される。
        {
            let mut requested = handle.requested_at.lock().expect("await を跨がない");
            if requested.is_none() {
                *requested = Some(std::time::Instant::now());
            }
        }
        handle.token.cancel();
        note!("interrupt requested: agent={id} seq={}", handle.seq);
    }

    /// 村の飛行中ターンを全部打ち切る（Spec 10 P4）。
    ///
    /// [`Orchestrator::interrupt_turn`] を全員へ適用するだけの薄い皮 —
    /// **for 文であること自体が仕様**（独自の機構・独自の重複排除を持たない）。
    /// 冪等: 飛行中ターンが 1 つも無くても成功。進行役とワーカーが親子で
    /// 二重に切られても、出口の行は各ターンが検知時に 1 回書くだけなので
    /// 重複しない。
    pub async fn interrupt_all(&self) {
        let ids: Vec<AgentId> = self.shared.turns.lock().await.keys().cloned().collect();
        for id in ids {
            self.interrupt_turn(&id).await;
        }
    }

    /// エージェントを停止する。処理中の発話は完了を待つ。
    ///
    /// # Errors
    /// 稼働していない場合 [`CoreError::NotRunning`]。
    pub async fn stop_agent(&self, id: &AgentId) -> CoreResult<()> {
        let handle = {
            let mut tasks = self.tasks.lock().await;
            tasks.remove(id).ok_or_else(|| CoreError::NotRunning {
                agent_id: id.to_string(),
            })?
        };

        // 飛行中のターンへ**最初に**割り込みを立てる（Spec 10 P5）。これが無いと、
        // 長いツールループの完走を下の join で最大 30 秒待つ。ステータスは
        // 不変条件 4 の但し書き側 — Running へ戻さず Stopping → Idle へ進む
        // （finish_interrupted はステータスに触れないので衝突しない）。
        // Stopping の通知より前に立てるのは順序の保証のため — 通知を見た側
        // （UI・テスト）が「割り込みはもう立っている」に依存できる。
        // 30 秒の上限は割り込みが効かない異常系の網としてそのまま残す。
        self.interrupt_turn(id).await;

        self.shared.set_status(id, AgentStatus::Stopping).await;
        // 受信箱を先に外し、停止処理中に新しい発話が積まれないようにする。
        self.shared.mailboxes.write().await.remove(id);

        let _ = handle.shutdown.send(true);
        // 処理中の LLM 呼び出しが終わるのを待つ。無限には待たない。
        if tokio::time::timeout(Duration::from_secs(30), handle.join)
            .await
            .is_err()
        {
            // タイムアウトしてもタスクは自走を続けるが、受信箱は既に外れているので
            // 次のループで停止する。ここで abort しないのは前掲の理由による。
        }

        // 個別 MCP を畳む（**自分のエントリだけ**。同じコマンドを使う他
        // エージェントのプロセスは別 spawn なので巻き添えにならない）。
        if let Some(state) = self.shared.agent_mcp.write().await.remove(id) {
            state.manager.shutdown().await;
        }

        {
            let mut world = self.shared.world.write().await;
            if let Ok(record) = world.agent_mut(id) {
                if let Some(started) = record.started_at.take() {
                    record.accumulated_uptime_secs += started.elapsed().as_secs();
                }
            }
        }
        self.shared.set_status(id, AgentStatus::Idle).await;
        Ok(())
    }

    /// 全エージェントを停止する。アプリ終了時に呼ぶ。
    pub async fn shutdown(&self) {
        let ids: Vec<AgentId> = self.tasks.lock().await.keys().cloned().collect();
        for id in ids {
            let _ = self.stop_agent(&id).await;
        }
    }

    // ---- 配送 ---------------------------------------------------------------

    /// ユーザー発話をエージェントへ投入する。
    ///
    /// # Errors
    /// - 宛先が稼働していない場合 [`CoreError::NotRunning`]
    /// - 受信箱が飽和している場合 [`CoreError::MailboxFull`]
    pub async fn send_user_message(&self, to: &AgentId, content: &str) -> CoreResult<()> {
        self.send_user_message_broadcast(to, content, &[]).await
    }

    /// ユーザー発話を**同報の 1 通として**エージェントへ投入する。
    ///
    /// `co_recipients` は同報の全宛先（受信者自身を含む）。UI は宛先ごとに
    /// このメソッドを呼び、毎回同じリストを渡す。受信者のプロンプトには
    /// 「全員が既に受け取っている」という注記が入り、転送する理由を消す
    /// （同報の反響防止）。**宛先外のエージェントへは何も送られない** —
    /// 同報の存在自体、宛先本人たちしか知らない。
    ///
    /// 2 体未満のリストは単独宛と同義なので、注記は付かない。
    pub async fn send_user_message_broadcast(
        &self,
        to: &AgentId,
        content: &str,
        co_recipients: &[AgentId],
    ) -> CoreResult<()> {
        self.send_user_message_with_attachments(to, content, co_recipients, Vec::new())
            .await
    }

    /// 添付画像つきのユーザー発話を投入する（Spec 23）。
    ///
    /// 画像はここで検証して `{workspace}/attachments/` へ保存し、発話には
    /// **参照だけ**を載せる。上限は 1 発話 1 枚（D5）。
    ///
    /// # Errors
    /// 2 枚以上・検証に落ちる画像は [`CoreError::InvalidAttachment`]
    /// （何も書かず、発話も投入しない）。
    pub async fn send_user_message_with_attachments(
        &self,
        to: &AgentId,
        content: &str,
        co_recipients: &[AgentId],
        uploads: Vec<AttachmentUpload>,
    ) -> CoreResult<()> {
        if uploads.len() > 1 {
            return Err(CoreError::InvalidAttachment {
                reason: "1 つの発話に添付できる画像は 1 枚までです".to_owned(),
            });
        }
        // 保存は発話の記録より**前**。検証に落ちたら発話ごと拒否する —
        // 「画像なしで送信されました」は、送った人の意図と黙って食い違う。
        let mut attachments = Vec::with_capacity(uploads.len());
        for upload in &uploads {
            attachments.push(
                self.shared
                    .attachments
                    .save(&upload.file_name, &upload.bytes)
                    .await?,
            );
        }

        let mut message = AgentMessage::new(
            Endpoint::User,
            Endpoint::Agent { id: to.clone() },
            content,
            0,
        );
        message.attachments = attachments;
        if co_recipients.len() >= 2 {
            message.co_recipients = co_recipients.to_vec();
        }
        self.shared.record(message.clone()).await;
        // 因果の根 — 予算はここで生まれる（Spec 11）。同報は宛先ごとに
        // このメソッドが呼ばれるので、宛先ごとに独立した予算になる（契約どおり）。
        let budget = new_root_budget(&self.shared).await;
        // 利用者の発話は参加者を数えない — 自動要約は予定の発火だけの機能で、
        // 人が話している間に履歴を畳むのは「押していない操作」になる。
        deliver(&self.shared, to, message, budget, None).await
    }

    // ---- 外部からの依頼（Spec 25） -------------------------------------------

    /// 外部の MCP クライアントからの依頼を窓口へ渡し、答えを待つ（Spec 25）。
    ///
    /// **オーケストレーションの機構は 1 つも増えない。** 外部の呼び出しは
    /// 構造的に「あるサーヴァントが別のサーヴァントへ `ask` する」のと同じで、
    /// [`deliver_and_wait`] をそのまま通る（待ち方も失敗の分類も既存のまま）。
    /// 増えるのは送り手が [`Endpoint::External`] であることだけ。
    ///
    /// # 因果の根
    ///
    /// 外部依頼は**予算の根の 3 種類目**（ユーザー発話 / 予定の発火 / これ）。
    /// hop は 0 から始まり、予算プールも新品になる。**だからこそ `max_hops` と
    /// トークンの天井は、扉を通る閉路を塞げない** — 塞ぐのは冒頭の同時 1 本の
    /// ゲートで、それが唯一の歯止め（`mcp_server_contract` 凍結 5）。
    ///
    /// # Errors
    /// - 窓口が未設定 [`CoreError::ExternalReceptionUnset`]
    /// - 窓口が削除済み [`CoreError::AgentNotFound`]
    /// - 窓口が停止中 [`CoreError::NotRunning`]
    /// - 別の外部依頼を処理中 [`CoreError::ExternalBusy`]
    pub async fn ask_external(&self, client: &str, message: &str) -> CoreResult<String> {
        // D7 — 同時 1 本。**待たずに即断る**（待つと閉路のデッドロックが
        // ask_timeout ぶん居座り、呼ぶ側からは「重い依頼」と区別が付かない）。
        // permit はこの関数を抜けるまで握る = 答えが返るまで次を通さない。
        let _permit = self
            .shared
            .external_gate
            .try_acquire()
            .map_err(|_| CoreError::ExternalBusy)?;

        let to = {
            let world = self.shared.world.read().await;
            let Some(to) = world.reception().cloned() else {
                return Err(CoreError::ExternalReceptionUnset);
            };
            // 窓口が削除されていれば「見つからない」を返す。**「未設定」へ
            // 畳まない** — 設定し直すのと初めて設定するのでは人の次の手が違う。
            world.agent(&to)?;
            to
        };
        // 停止中は黙って待たない（S6）。受信箱の有無が稼働の判定
        // （「ここに居る = 送信できる」の不変条件）。
        if !self.shared.mailboxes.read().await.contains_key(&to) {
            return Err(CoreError::NotRunning {
                agent_id: to.to_string(),
            });
        }

        // 自己申告の名乗りはここで 1 回だけ正規化する。**プロンプトへ入る前**で
        // なければ意味がない（`mcp_server_contract` 凍結 6）。
        let from = Endpoint::External {
            client: crate::world::normalize_client_name(client),
        };
        let budget = new_root_budget(&self.shared).await;
        // 因果の根なので親トークンを持たない。打ち切りは既存の
        // `interrupt_turn` / `interrupt_all` が窓口のターンに効く。
        let cancel = tokio_util::sync::CancellationToken::new();
        let (answer, _state) = deliver_and_wait(
            &self.shared,
            &from,
            &to,
            message,
            0,
            &cancel,
            budget.as_ref(),
            // 外部依頼は予定ではないので参加者を数えない（自動要約の対象外）。
            None,
        )
        .await;
        // 分類は捨てる（`ask` と同じ）。**失敗も文字列で返る** — 相手が
        // 答えなかった・時間切れだったは会話の事実であって、扉の故障ではない。
        Ok(answer)
    }

    // ---- 予定（Spec 07） -----------------------------------------------------
}
