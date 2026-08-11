//! 会話の持ち替え（Spec 12）。新規チャット・開き直し・分岐・要約。
//!
//! **`orchestrator` の子モジュールなので、親の private がそのまま見える** —
//! 可視性を緩める必要は無い（Rust では private アイテムは宣言したモジュールと
//! その子孫から見える）。`use super::*` にしているのは、切り出しのたびに
//! use を積み直すと差分が「移動」に見えなくなるため。

use super::*;

impl Orchestrator {
    /// 会話をリセットする（新規チャット。Spec 03 → **Spec 12 で改訂**）。
    ///
    /// **捨てるのではなく、今の会話を閉じて新しい会話を開く。** 前の会話は
    /// ディスクに残り、一覧から戻れる。UI の「新規チャット」という名前は残す
    /// （利用者の語彙を変えない）。
    ///
    /// 消すのは `Shared.log` と各エージェントの `history` の**2 つだけ** —
    /// 稼働状態・累積統計・Memory.md・エージェント別 MCP 接続はすべて維持
    /// する（リセットするのは「会話」であって「エージェント」ではない）。
    ///
    /// 飛行中ターンの完了書き込みは**許容**する（Spec 03 の案 A は改訂後も
    /// そのまま生きる） — 白紙化の直後に飛行中だった発話 1 件が載るのは仕様。
    /// 着地先は新しい空の会話なので、既存の記録を汚さない。
    ///
    /// **保存先が開けていない村では、改訂前の挙動のまま**メモリを白紙にして
    /// [`CoreEvent::ConversationCleared`] だけを出す。保存できないことは、
    /// 新規チャットを使えなくする理由にならない。
    ///
    /// # Errors
    /// 保存先への書き込みに失敗した場合。
    pub async fn reset_conversation(&self) -> CoreResult<()> {
        if self.shared.sessions.is_none() {
            self.shared.log.write().await.clear();
            self.shared.world.write().await.clear_histories();
            self.shared.emit(CoreEvent::ConversationCleared);
            return Ok(());
        }
        self.create_session().await.map(|_| ())
    }

    // ---- セッション（Spec 12 — 会話の永続化） -------------------------------

    /// いま開いている会話の ID。保存先が開けていない村では空文字。
    pub fn current_session(&self) -> String {
        self.shared.current_session()
    }

    /// 保存されている会話の一覧（`updatedAt` の新しい順）。
    ///
    /// # Errors
    /// 保存先が開けていない、または読み込みに失敗した場合。
    pub async fn list_sessions(&self) -> CoreResult<Vec<SessionSummary>> {
        self.sessions()?.list_sessions()
    }

    /// 新しい会話を開く。今の会話は閉じるだけで、消えない。
    ///
    /// **切り替えの中でここだけは飛行中ターンを拒まない**（Spec 03 の案 A が
    /// Spec 12 改訂後もそのまま生きる）。着地先が「新しい空の会話」なので、
    /// 飛行中だった答えが後から載っても失うものが無い — 発話は起きた事実であり、
    /// メモリとディスクの両方に同じ形で残る。既存の会話を開く
    /// [`Orchestrator::resume_session`] / [`Orchestrator::fork_session`] は
    /// 事情が違い（**前に保存された会話へ無関係な答えが混ざる**）、そちらは拒む。
    ///
    /// # Errors
    /// 保存先が開けていない、または書き込みに失敗した場合。
    pub async fn create_session(&self) -> CoreResult<String> {
        let session_id = self.sessions()?.create_session(None)?;
        self.switch_to(&session_id).await?;
        Ok(session_id)
    }

    /// 保存されている会話を開き直す。
    ///
    /// # Errors
    /// 保存先が開けていない、会話が存在しない、飛行中のターンがある、
    /// または読み込みに失敗した場合。
    pub async fn resume_session(&self, session_id: &str) -> CoreResult<()> {
        self.ensure_idle_for_switch().await?;
        if self.sessions()?.session_meta(session_id)?.is_none() {
            return Err(CoreError::SessionNotFound(session_id.to_owned()));
        }
        self.switch_to(session_id).await
    }

    /// 最後に更新された会話を開き直す。1 件も無ければ新しく作る。
    ///
    /// # Errors
    /// 保存先が開けていない、飛行中のターンがある、または読み書きに失敗した場合。
    pub async fn continue_latest(&self) -> CoreResult<String> {
        self.ensure_idle_for_switch().await?;
        let store = self.sessions()?;
        let session_id = match store.latest_session()? {
            Some(id) => id,
            None => store.create_session(None)?,
        };
        self.switch_to(&session_id).await?;
        Ok(session_id)
    }

    /// 分岐できる地点（その会話のユーザー発話）を古い順で返す。
    ///
    /// # Errors
    /// 保存先が開けていない、会話が存在しない、または読み込みに失敗した場合。
    pub async fn list_fork_points(&self, session_id: &str) -> CoreResult<Vec<ForkPoint>> {
        self.sessions()?.fork_points(session_id)
    }

    /// 会話を `at_seq` **まで含めて**複製し、複製した側を開く。元は不変のまま残る。
    ///
    /// # Errors
    /// 保存先が開けていない、分岐元が存在しない、飛行中のターンがある、
    /// または書き込みに失敗した場合。
    pub async fn fork_session(&self, session_id: &str, at_seq: u64) -> CoreResult<String> {
        self.ensure_idle_for_switch().await?;
        let forked = self.sessions()?.fork_session(session_id, at_seq)?;
        self.switch_to(&forked).await?;
        Ok(forked)
    }

    /// 会話を消す。
    ///
    /// **開いている会話を消した場合は、次の会話へ切り替える**（無ければ新規を作る）。
    /// 消したまま開きっぱなしにすると、以後の発話が行き先の無いセッションへ
    /// 書かれ続ける。開いていない会話の削除は、飛行中ターンがあっても通る
    /// （着地先は変わらないため）。
    ///
    /// # Errors
    /// 保存先が開けていない、開いている会話を消すのに飛行中のターンがある、
    /// または書き込みに失敗した場合。
    pub async fn delete_session(&self, session_id: &str) -> CoreResult<()> {
        let store = self.sessions()?;
        if session_id != self.shared.current_session() {
            return store.delete_session(session_id);
        }

        self.ensure_idle_for_switch().await?;
        store.delete_session(session_id)?;
        let next = match store.latest_session()? {
            Some(id) => id,
            None => store.create_session(None)?,
        };
        self.switch_to(&next).await
    }

    /// いまの会話を要約して続ける（Spec 12 P4）。要約できたエージェント数を返す。
    ///
    /// **人が押したときだけ走る。** 自動では要約しない — 要約は LLM 呼び出し
    /// = トークンで、Spec 11 の天井と競合する（自動で仕事を増やす機構は入れない、
    /// という Replan 不採用と同じ規律）。
    ///
    /// **対象は稼働中のサーヴァントだけ。** 要約の目的は以後のプロンプトを短くする
    /// ことで、停止中の個体には以後のターンが無い。それでも呼べば、参加していない
    /// 個体のぶんまで押した人がトークンを払うことになる。履歴は停止しても消えない
    /// ので、起動してから押せばそのとき要約される。
    ///
    /// 稼働中のエージェントごとに 1 回ずつ呼び、`summary` レコードを追加して**その
    /// エージェントの履歴を畳む**。要約は履歴の席を持たず、次のターンの可変文脈へ
    /// 差される（#45 との衝突を避けるため）。**元のレコードは消さない** —
    /// 要約の品質が悪かったときに、書き出しから元の往復を読み戻せる。
    ///
    /// **飛行中のターンがあっても拒まない。** 飛行中のターンはそのターンの
    /// `sent_user_turn` を既に組み終えており、完了時に積む `exchange` は要約が
    /// 覆う `coversUpToSeq` より後の seq を取る。畳んだ履歴の上に 1 往復が乗るだけで、
    /// 復元しても同じ形になる。
    ///
    /// # Errors
    /// 保存先が開けていない、または保存に失敗した場合。**個々のエージェントの
    /// 要約が失敗しても全体は失敗させない**（その相手の履歴は畳まずに残す —
    /// 要約に失敗した代償が履歴の喪失になるのは最悪の交換）。
    pub async fn summarize_session(&self) -> CoreResult<usize> {
        // 本体は Shared に置いてある。予定の完了後の自動要約（Spec 28 の
        // `summarizeAfter`）が Orchestrator を持たない場所から呼ぶ必要があり、
        // **同じ規律を 2 箇所に書かない**ために下ろした。
        self.sessions()?;
        self.shared.summarize_agents(None).await
    }

    /// 会話を JSONL で書き出し、書いたレコード数を返す。
    ///
    /// 読める出口は機構の一部（この企画の診断は grep に依存している）。
    ///
    /// # Errors
    /// 保存先が開けていない、会話が存在しない、または書き出しに失敗した場合。
    pub async fn export_session(
        &self,
        session_id: &str,
        dest: impl AsRef<std::path::Path>,
    ) -> CoreResult<u64> {
        self.sessions()?.export_session_to_file(session_id, dest)
    }

    /// 保存先を借りる。開けていなければ、直し方を添えて失敗を返す。
    fn sessions(&self) -> CoreResult<&SessionStore> {
        self.shared
            .sessions
            .as_ref()
            .ok_or_else(|| CoreError::SessionStore {
                path: self
                    .shared
                    .store
                    .root()
                    .join("sessions.redb")
                    .display()
                    .to_string(),
                operation: "開く",
                reason: "起動時に開けませんでした（起動ログの WARN 行に理由が出ています）"
                    .to_owned(),
            })
    }

    /// 切り替えの前提。**飛行中のターンがあれば拒否する**（Spec 12 の不変条件 11）。
    ///
    /// 自動で `interrupt_all` を呼んでから切り替えることは**しない** —
    /// 飛行中のターンを畳むかどうかは人の判断で、機械が黙って決める場面ではない。
    async fn ensure_idle_for_switch(&self) -> CoreResult<()> {
        let in_flight = self.shared.turns.lock().await.len();
        if in_flight > 0 {
            return Err(CoreError::SessionSwitchBlocked { in_flight });
        }
        Ok(())
    }

    /// メモリ上の会話を差し替えて、指定した会話を開いた状態にする。
    ///
    /// 処理順は契約で固定: **log クリア → history クリア → 復元 →
    /// `conversationCleared` → `sessionSwitched`**。
    /// `conversationCleared` を出さない選択は採らない — 会話ペインを空にする
    /// 指示はこれが唯一の経路で、意味を変えると既存 UI が誤動作する。
    async fn switch_to(&self, session_id: &str) -> CoreResult<()> {
        // 本体は Shared に置いてある。予定の発火（Spec 28 の `sessionMode: fresh`）
        // が Orchestrator を持たずに切り替える必要があり、**同じ規律を 2 箇所に
        // 書かないため**に下ろした。
        self.shared.open_session(session_id).await
    }
}
