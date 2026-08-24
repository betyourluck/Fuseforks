//! 村の設定と、GUI が読む資源の入口。
//!
//! トークン天井（Spec 11）・言語（Spec 13）・利用者と外部クライアントの
//! 呼び名とアイコン（Spec 19 / 25）・資格情報・MCP・条例・コマンド承認
//! （Spec 20）・黒板・作業フォルダの一覧（Spec 24）。
//!
//! **API キーはここを通らない。** set_credential は OS の資格情報ストアへ
//! 書くだけで、値を返す API は無い（読めるのは has_credential のみ）。
//! world.json に鍵が載らないことは、この非対称で保たれている。

use super::*;

impl Orchestrator {
    /// トークン予算の天井（実効トークン建て）を返す。`None` = 天井なし。
    pub async fn token_budget(&self) -> Option<u64> {
        self.shared.world.read().await.token_budget()
    }

    /// トークン予算の天井を差し替え、`world.json` へ書き戻す。
    ///
    /// 天井は依頼のたびに `new_root_budget` が `World` から読むので、
    /// **次の依頼から効く**。再起動は要らない（`settings_contract` の即時反映 —
    /// `world.json` は所有者ではなく投影）。
    ///
    /// # Errors
    /// - `Some(0)` は [`CoreError::InvalidTokenBudget`]。読み込み時の
    ///   `Some(0) → None` 正規化は外部編集の遡及回収であって、この経路で
    ///   受け付けて黙って倒すと「保存したのに別の値になる」
    pub async fn set_token_budget(&self, ceiling: Option<u64>) -> CoreResult<()> {
        if ceiling == Some(0) {
            return Err(CoreError::InvalidTokenBudget);
        }
        self.shared.world.write().await.set_token_budget(ceiling);
        self.persist().await
    }

    /// 委譲の待ち時間・秒（Spec 44）。`None` = 既定（600 秒）。
    pub async fn ask_timeout_secs(&self) -> Option<u64> {
        self.shared.world.read().await.ask_timeout_secs()
    }

    /// 委譲の待ち時間を差し替え、`world.json` へ書き戻す。
    ///
    /// `deliver_and_wait` が呼び出しごとに `World` から読むので、保存すれば
    /// **次の委譲から**効く（`new_root_budget` と同じ形。再起動不要）。
    ///
    /// # Errors
    /// 範囲（30..=3600 秒）の外は [`CoreError::InvalidAskTimeout`]。
    /// 0 や負を許すと全委譲が即死する村が作れる（凍結 4）。
    pub async fn set_ask_timeout(&self, secs: Option<u64>) -> CoreResult<()> {
        if let Some(secs) = secs
            && !(crate::world::ASK_TIMEOUT_MIN_SECS..=crate::world::ASK_TIMEOUT_MAX_SECS)
                .contains(&secs)
        {
            return Err(CoreError::InvalidAskTimeout);
        }
        self.shared.world.write().await.set_ask_timeout_secs(secs);
        self.persist().await
    }

    /// UI の表示言語。bootstrap が必ず確定させるので、未確定は起こらない
    /// （防御の既定は従来の見た目 = 日本語）。
    pub async fn language(&self) -> crate::world::Language {
        self.shared
            .world
            .read()
            .await
            .language()
            .unwrap_or(crate::world::Language::Ja)
    }

    /// UI の表示言語を差し替え、`world.json` へ書き戻す。
    ///
    /// **Spec 35 から、プロンプトの新規生成はこの値で分岐する**（枠組み・
    /// ツールの提示。次のターンから効く）。エラー文言は今も分岐しない
    /// （settings_contract 層 2 の案 A — コアは日本語で返し UI が訳す）。
    /// 履歴・バックエンドには触らず、保存済みの System 行と封筒は
    /// 記録時の言語のまま残る（settings_contract 層 3）。
    pub async fn set_language(&self, language: crate::world::Language) -> CoreResult<()> {
        self.shared.world.write().await.set_language(language);
        self.persist().await
    }

    /// 利用者の呼び名（Spec 19）。`None` = 未設定。
    ///
    /// 未設定を既定値へ倒さずそのまま返すのは、**画面が「未設定である」ことを
    /// 示せるようにする**ため（`language` と違い、こちらは未設定が正常な状態）。
    pub async fn user_name(&self) -> Option<String> {
        self.shared
            .world
            .read()
            .await
            .user_name()
            .map(str::to_owned)
    }

    /// 利用者のアイコン（WebP バイト列）。未設定なら `None`（Spec 19）。
    pub async fn user_icon(&self) -> CoreResult<Option<Vec<u8>>> {
        self.shared.store.read_user_icon().await
    }

    /// 利用者のアイコンを保存する（Spec 19）。
    ///
    /// # Errors
    /// WebP でない・サイズ上限超過の場合 [`CoreError::InvalidIcon`]。
    /// 検証は**エージェントのアイコンと同じ述語**を通る（`icon_contract`）。
    pub async fn set_user_icon(&self, bytes: &[u8]) -> CoreResult<()> {
        self.shared.store.write_user_icon(bytes).await
    }

    /// 利用者のアイコンを削除する。未設定でも成功（Spec 19）。
    pub async fn clear_user_icon(&self) -> CoreResult<()> {
        self.shared.store.delete_user_icon().await
    }

    /// 利用者の呼び名を差し替え、`world.json` へ書き戻す。`None` で既定へ戻す。
    ///
    /// 次のターンの封筒から効く（`attribute_sender` は呼び出しのたびに `World`
    /// から引く）。**過去の履歴と会話ログの `【送り手: 旧名】` は直さない** —
    /// 残り香を消す機構は作らない（`user_identity_contract` 凍結 8）。
    ///
    /// # Errors
    /// 書式が受け入れ条件を満たさない場合 [`CoreError::InvalidUserName`]。
    /// **拒否したときはメモリもファイルも触らない。**
    pub async fn set_user_name(&self, name: Option<&str>) -> CoreResult<()> {
        self.shared.world.write().await.set_user_name(name)?;
        self.persist().await
    }

    /// 外部クライアントの呼び名（Spec 25）。`None` = 未設定（名乗りへ落ちる）。
    pub async fn external_name(&self) -> Option<String> {
        self.shared
            .world
            .read()
            .await
            .external_name()
            .map(str::to_owned)
    }

    /// 外部クライアントの呼び名を差し替える。`None` で未設定へ戻す（Spec 25）。
    ///
    /// # Errors
    /// 書式が受け入れ条件を満たさない場合 [`CoreError::InvalidUserName`]。
    pub async fn set_external_name(&self, name: Option<&str>) -> CoreResult<()> {
        self.shared.world.write().await.set_external_name(name)?;
        self.persist().await
    }

    /// 外部クライアントのアイコン（WebP バイト列）。未設定なら `None`（Spec 25）。
    pub async fn external_icon(&self) -> CoreResult<Option<Vec<u8>>> {
        self.shared.store.read_external_icon().await
    }

    /// 外部クライアントのアイコンを保存する（Spec 25）。
    ///
    /// # Errors
    /// WebP でない・サイズ上限超過の場合 [`CoreError::InvalidIcon`]。
    /// **検証はエージェント・利用者と同じ述語**（`icon_contract`）を通る。
    pub async fn set_external_icon(&self, bytes: &[u8]) -> CoreResult<()> {
        self.shared.store.write_external_icon(bytes).await
    }

    /// 外部クライアントのアイコンを削除する（Spec 25）。
    pub async fn clear_external_icon(&self) -> CoreResult<()> {
        self.shared.store.delete_external_icon().await
    }

    /// 外部からの依頼を受ける窓口（Spec 25 D2）。`None` = 未設定。
    pub async fn reception(&self) -> Option<AgentId> {
        self.shared.world.read().await.reception().cloned()
    }

    /// 窓口を差し替える。`None` で未設定へ戻す（Spec 25 D2）。
    ///
    /// # Errors
    /// 指定したエージェントが未登録の場合 [`CoreError::AgentNotFound`]。
    pub async fn set_reception(&self, agent_id: Option<&AgentId>) -> CoreResult<()> {
        self.shared.world.write().await.set_reception(agent_id)?;
        self.persist().await
    }

    // ---- 資格情報 -----------------------------------------------------------

    /// テンプレートの API キーを OS の資格情報ストアへ登録する。
    ///
    /// 併せてテンプレートの取得元を [`CredentialSource::Keyring`] に切り替え、
    /// 構築済みバックエンドのキャッシュを捨てる。登録したのに次の発話まで
    /// 反映されない、という状態を作らないため。
    pub async fn set_credential(&self, id: &ModelTemplateId, secret: &str) -> CoreResult<()> {
        // 貼り付け由来の前後空白・改行を落とす。正当な API キーの先頭・末尾に
        // 空白が含まれることはなく、混入すると送信時の 401 (Invalid token 等)
        // としてしか表面化しない — 登録時に吸収するのが唯一気づける場所。
        let secret = secret.trim();
        {
            // 存在しないテンプレートに対して秘密を書き込ませない。
            let world = self.shared.world.read().await;
            world.template(id)?;
        }
        self.shared.secrets.set(id.as_str(), secret)?;

        {
            let mut world = self.shared.world.write().await;
            let mut template = world.template(id)?.clone();
            template.credential = CredentialSource::Keyring;
            world.upsert_template(template);
        }
        self.shared.backends.write().await.remove(id);
        self.persist().await
    }

    /// テンプレートの API キーを資格情報ストアから削除する。
    ///
    /// 取得元は「未設定」へ戻す。「認証不要」へ落とすと、キーを消しただけの
    /// テンプレートが認証ヘッダ無しで外部へ送られるようになる。
    pub async fn clear_credential(&self, id: &ModelTemplateId) -> CoreResult<()> {
        self.shared.secrets.delete(id.as_str())?;

        {
            let mut world = self.shared.world.write().await;
            if let Ok(existing) = world.template(id) {
                let mut template = existing.clone();
                template.credential = CredentialSource::Unset;
                world.upsert_template(template);
            }
        }
        self.shared.backends.write().await.remove(id);
        self.persist().await
    }

    /// API キーが登録済みかどうかだけを返す。**値は返さない。**
    pub fn has_credential(&self, id: &ModelTemplateId) -> CoreResult<bool> {
        self.shared.secrets.contains(id.as_str())
    }

    /// 設定ファイルを読む。
    pub async fn read_config(&self, id: &AgentId, kind: ConfigFileKind) -> CoreResult<String> {
        // 未登録エージェントのファイルを読めてしまわないよう存在確認を先に行う。
        self.shared.world.read().await.agent(id)?;
        self.shared.store.read_config(id, kind).await
    }

    // ---- MCP -----------------------------------------------------------------

    /// `mcp.json` の宣言を読む。
    pub async fn mcp_config(&self) -> CoreResult<crate::mcp::McpConfig> {
        self.shared.store.read_mcp_config().await
    }

    /// `mcp.json` を書き、その場で接続し直す。
    pub async fn set_mcp_config(&self, config: &crate::mcp::McpConfig) -> CoreResult<()> {
        self.shared.store.write_mcp_config(config).await?;
        self.reload_mcp().await
    }

    /// MCP サーバーへ接続し直し、ツール登録簿を入れ替える。
    ///
    /// 1 台の失敗で全体を止めない（[`crate::mcp::McpManager::connect_all`]）。
    /// 各サーバーの結果は [`Orchestrator::mcp_statuses`] で読める。
    ///
    /// # Errors
    /// `mcp.json` が壊れている場合。**空として扱わない** — 書き間違えた瞬間に
    /// 全ツールが黙って消えると、利用者は原因に辿り着けない。
    pub async fn reload_mcp(&self) -> CoreResult<()> {
        let config = self.shared.store.read_mcp_config().await?;
        let next = crate::mcp::McpManager::connect_all(&config).await;

        // 古い接続のツールを先に外す。消さずに新しいものを登録すると、
        // 繋がっていないサーバーのツールがモデルへ提示され続ける。
        let previous = {
            let mut slot = self.shared.mcp.write().await;
            std::mem::replace(&mut *slot, next)
        };
        {
            let mut registry = self.shared.tools.write().await;
            for tool in previous.tools() {
                registry.unregister(tool.name());
            }
            let current = self.shared.mcp.read().await;
            for tool in current.tools() {
                registry.register(Arc::clone(tool));
            }
        }
        // 旧接続は登録簿から外し終えてから畳む（畳む間も古い呼び出しは来ない）。
        previous.shutdown().await;
        Ok(())
    }

    /// 各 MCP サーバーの接続状態。UI へそのまま出せる。
    pub async fn mcp_statuses(&self) -> Vec<crate::mcp::McpServerStatus> {
        self.shared.mcp.read().await.statuses().to_vec()
    }

    /// エージェント別 MCP の状態（Spec 02）。
    ///
    /// 停止中は「未接続」としか答えられない — 接続はエージェントの稼働に
    /// 紐付き、状態は永続化しない（嘘をつく状態ファイルを持たない）。
    pub async fn agent_mcp_status(&self, id: &AgentId) -> CoreResult<AgentMcpStatus> {
        self.shared.world.read().await.agent(id)?;
        let map = self.shared.agent_mcp.read().await;
        Ok(match map.get(id) {
            Some(state) => AgentMcpStatus {
                running: true,
                load_error: state.load_error.clone(),
                servers: state.manager.statuses().to_vec(),
            },
            None => AgentMcpStatus {
                running: false,
                load_error: None,
                servers: Vec::new(),
            },
        })
    }

    // ---- 村の条例 -------------------------------------------------------------

    /// 村の条例（全エージェント共通の規則）を読む。未設定なら空文字。
    pub async fn read_ordinance(&self) -> CoreResult<String> {
        self.shared.store.read_ordinance().await
    }

    /// 村の条例を書く。次の発話からすべてのエージェントに反映される
    /// （プロンプトはメッセージごとに組み直すため、再起動は不要）。
    pub async fn write_ordinance(&self, content: &str) -> CoreResult<()> {
        self.shared.store.write_ordinance(content).await
    }

    // ---- コマンドの承認（Spec 20） ---------------------------------------------

    /// 全サーヴァントの `run.json` を読む。承認画面の投影用。
    ///
    /// **壊れている個体は `Err` を畳んで飛ばす**（既定を返さない）。既定を返すと
    /// 画面には「判断待ちゼロ・許可ゼロ」に見え、**壊れていることが分からない**。
    /// 読めなかった事実は `broken` に載せて画面へ運ぶ。
    pub async fn command_policies(&self) -> Vec<CommandPolicyView> {
        let mut views = Vec::new();
        for snapshot in self.snapshots().await {
            let view = match self.shared.store.read_command_policy(&snapshot.id).await {
                Ok(policy) => CommandPolicyView {
                    agent_id: snapshot.id,
                    name: snapshot.name,
                    pending: policy.pending,
                    broken: false,
                },
                Err(_) => CommandPolicyView {
                    agent_id: snapshot.id,
                    name: snapshot.name,
                    pending: Vec::new(),
                    broken: true,
                },
            };
            views.push(view);
        }
        views
    }

    /// `pending` の 1 件を承認して `allow` へ入れる（Spec 20）。
    ///
    /// **粒度は `open` だけで決まる。** パターン文字列を外から受け取らないのは、
    /// 受け取ると「粒度は機械が決めない」が**「粒度を GUI が何でも決められる」へ
    /// 反転する**ため（`*` 1 文字も送れてしまう）。
    ///
    /// `allow` の 1 件目が入ると、**次のターンからそのサーヴァントは実際に
    /// コマンドを実行できるようになる**。**提示はその前から起きている** —
    /// `run` は `enabledTools` にチェックがあれば `allow` が空でも提示される
    /// （2026-08-06 の撤回。提示が承認を待つと、要求が積めず**承認する対象が
    /// 生まれない**閉じた輪になっていた）。
    pub async fn approve_command(
        &self,
        id: &AgentId,
        command: &str,
        args: &[String],
        open: bool,
    ) -> CoreResult<ApprovalOutcome> {
        self.shared
            .store
            .update_command_policy(id, |policy| policy.approve(command, args, open))
            .await
    }

    /// `pending` の 1 件を却下して `deny` へ入れる（Spec 20）。
    pub async fn reject_command(
        &self,
        id: &AgentId,
        command: &str,
        args: &[String],
        open: bool,
    ) -> CoreResult<ApprovalOutcome> {
        self.shared
            .store
            .update_command_policy(id, |policy| policy.reject(command, args, open))
            .await
    }

    // ---- 村の黒板 -------------------------------------------------------------

    /// 村の黒板（work_dir の `blackboard/`）を読む。GUI 投影用・読み取り専用。
    ///
    /// 対象は登録エージェントの work_dir（先に見つかった順・重複除去）。
    /// 条例の運用は共通 work_dir が前提だが、複数の work_dir が混在していても
    /// 全部読み、[`crate::blackboard::BlackboardNote::dir`] で区別できる形で返す。
    pub async fn read_blackboard(&self) -> CoreResult<Vec<crate::blackboard::BlackboardNote>> {
        let mut notes = Vec::new();
        // **読みと削除で同じ列挙を使う。** 2 箇所に書くと、
        // 「画面に出ているのに消せない付箋」が生まれる余地ができる。
        for dir in self.blackboard_dirs().await {
            notes.extend(
                crate::blackboard::read_blackboard_dir(std::path::Path::new(&dir)).await?,
            );
        }
        Ok(notes)
    }

    /// 黒板の付箋を 1 枚ごみ箱へ移す（2026-08-12 の UI 追加）。
    ///
    /// **`dir` は「いまサーヴァントが向いている work_dir」のどれかでなければ
    /// 受け付けない。** GUI から任意のパスを渡せる形にすると、黒板の削除が
    /// **どこのファイルでも消せる口**になる（囲いは `resolve_in_work_dir` が
    /// ツール側で持っているが、この IPC はその外にある）。
    pub async fn delete_blackboard_note(&self, dir: &str, name: &str) -> CoreResult<()> {
        if !self.blackboard_dirs().await.iter().any(|known| known == dir) {
            return Err(crate::error::CoreError::BlackboardDeleteFailed {
                name: name.to_owned(),
                reason: "その作業フォルダは黒板の対象ではありません".to_owned(),
            });
        }
        crate::blackboard::delete_note(std::path::Path::new(dir), name).await
    }

    /// 黒板の付箋を全部ごみ箱へ移す。戻り値は移した枚数。
    ///
    /// **1 枚失敗したらそこで止める。** 半分消えた状態で成功を返すと、
    /// 画面は空に見えるのに実体が残る（次の再読で戻ってくる）。
    pub async fn clear_blackboard(&self) -> CoreResult<usize> {
        let mut removed = 0usize;
        for dir in self.blackboard_dirs().await {
            for note in crate::blackboard::read_blackboard_dir(std::path::Path::new(&dir)).await? {
                crate::blackboard::delete_note(std::path::Path::new(&dir), &note.name).await?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// 黒板を持ちうる work_dir の一覧（重複なし）。
    async fn blackboard_dirs(&self) -> Vec<String> {
        let mut dirs: Vec<String> = Vec::new();
        for snapshot in self.snapshots().await {
            if let Some(dir) = snapshot.work_dir
                && !dirs.contains(&dir)
            {
                dirs.push(dir);
            }
        }
        dirs
    }

    // ---- アイコン -------------------------------------------------------------

    /// エージェントのアイコン（WebP バイト列）を読む。未設定なら `None`。
    pub async fn agent_icon(&self, id: &AgentId) -> CoreResult<Option<Vec<u8>>> {
        self.shared.world.read().await.agent(id)?;
        self.shared.store.read_icon(id).await
    }

    /// 入力欄のパス補完へ渡すファイル一覧（Spec 24）。
    ///
    /// 作業フォルダが**未設定なら空の一覧**を返す。UI はそもそも
    /// `AgentSnapshot.workDir` を持っているので、未設定のときは呼ばずに
    /// 理由を出せる（`AgentSettingsDialog` の `noWorkDirWarn` と同じ形）—
    /// **判断に必要な情報を既に持っている層で判断する。**
    ///
    /// **囲いはここに無い**（Spec 24 Notes 5）。返すのは候補であって権限ではなく、
    /// 挿入されたパスを実際に読むのは `file` / `grep` で、あちらが
    /// `resolve_in_work_dir` で境界を守る。**ここに検査を足すと同じ規律が
    /// 2 箇所に生える**（「参照…」ボタンで選ばれたパスを検査しないのと同じ判断）。
    pub async fn list_work_dir_files(&self, id: &AgentId) -> CoreResult<WorkDirListing> {
        let work_dir = {
            let world = self.shared.world.read().await;
            world.agent(id)?.spec.work_dir.clone()
        };
        let Some(work_dir) = work_dir else {
            return Ok(WorkDirListing {
                paths: Vec::new(),
                truncated: false,
            });
        };
        // 走査は同期 I/O なので blocking へ逃がす。20,000 件の走査で
        // ランタイムのワーカーを塞ぐと、その間ほかのエージェントのターンが止まる。
        let (paths, truncated) = tokio::task::spawn_blocking(move || {
            crate::tools::fs::relative_file_paths(std::path::Path::new(&work_dir))
        })
        .await
        .unwrap_or_else(|_| (Vec::new(), false));
        Ok(WorkDirListing { paths, truncated })
    }

    /// 添付画像の実体（WebP バイト列）を読む（Spec 23。表示用）。
    ///
    /// `None` は「保持期間を過ぎて削除された」（D9）— エラーではなく
    /// 通常の答えで、UI はプレースホルダの枠を出す。
    ///
    /// # Errors
    /// id が UUID の字種でない場合 [`CoreError::UnsafeIdentifier`]。
    pub async fn read_attachment(&self, id: &str) -> CoreResult<Option<Vec<u8>>> {
        self.shared.attachments.read(id).await
    }

    /// エージェントのアイコンを設定する。中身の検証（WebP・サイズ上限）は store が担う。
    pub async fn set_agent_icon(&self, id: &AgentId, bytes: &[u8]) -> CoreResult<()> {
        self.shared.world.read().await.agent(id)?;
        self.shared.store.write_icon(id, bytes).await
    }

    /// エージェントのアイコンを削除する。
    pub async fn clear_agent_icon(&self, id: &AgentId) -> CoreResult<()> {
        self.shared.world.read().await.agent(id)?;
        self.shared.store.delete_icon(id).await
    }

    /// 設定ファイルを書く。
    ///
    /// `mcp.json` の保存で、そのエージェントが**稼働中なら**個別接続を
    /// 張り直す（Spec 02）。停止中は検証つきの保存だけで、次回起動で反映。
    pub async fn write_config(
        &self,
        id: &AgentId,
        kind: ConfigFileKind,
        content: &str,
    ) -> CoreResult<()> {
        self.shared.world.read().await.agent(id)?;
        self.shared.store.write_config(id, kind, content).await?;

        if kind == ConfigFileKind::Mcp {
            let running = self.tasks.lock().await.contains_key(id);
            if running {
                if let Some(old) = self.shared.agent_mcp.write().await.remove(id) {
                    old.manager.shutdown().await;
                }
                connect_agent_mcp(&self.shared, id).await;
            }
        }
        Ok(())
    }

    /// 登録簿を永続化する。
    pub async fn persist(&self) -> CoreResult<()> {
        let persisted = self.shared.world.read().await.to_persisted();
        self.shared.store.save_world(&persisted).await
    }

    // ---- ライフサイクル -----------------------------------------------------
}
