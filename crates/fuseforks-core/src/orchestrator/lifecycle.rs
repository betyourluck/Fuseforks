//! 村の編成。サーヴァント・役職・モデルテンプレートの CRUD。
//!
//! **役職の設定は作成のときだけ流し込む**（role_contract 凍結 4）。
//! 発火点を 2 つ持つと、片方だけ流し込む実装がいつか生える。
//! update_agent が見るのは表示だけで、設定は 1 欄も触らない。
//!
//! **線は人が引く。** 役職の雛形に connected_agents は入れない —
//! 入れると役職を選んだ瞬間に線が引かれ、この村の原則が崩れる。

use super::*;

impl Orchestrator {
    /// エージェントを登録する。
    /// エージェントを新規登録する。
    ///
    /// **`spec.role_id` が指す役職があれば、ここで既定値を流し込む**（Spec 14）。
    /// `role_contract` 凍結 4 のとおり、**流し込みの発火点はこの 1 箇所だけ** —
    /// `update_agent` も `upsert_role` も既存の個体には触らない。ゆえに
    /// 「既存の個体は変わらない」があらゆる操作について成立する。
    ///
    /// 役職が引けないときは**流し込まずに作る**（`role_id` は残す）。存在しない
    /// 役職を指したまま作成そのものを拒むと、村を配った先で役職が欠けている
    /// だけで新規作成ができなくなる。
    pub async fn create_agent(&self, mut spec: AgentSpec) -> CoreResult<AgentSnapshot> {
        let id = spec.id.clone();

        // 流し込みは登録の前。register_agent が id の安全性と重複を弾くので、
        // 弾かれる spec に対してファイルを書きに行かずに済む。
        let construct = {
            let world = self.shared.world.read().await;
            match spec.role_id.clone().and_then(|rid| world.role(&rid).ok().cloned()) {
                Some(role) => {
                    let dropped = role
                        .defaults
                        .apply_to(&mut spec, |tid| world.template(tid).is_ok());
                    // **黙って落とさない。** 人が今まさに操作している最中なので、
                    // 黙ると「入れたはずの設定が入っていない」が見えない。
                    if !dropped.is_empty() {
                        crate::note!(
                            "role apply: 役職 `{}` の {} は参照先が無いため入れませんでした",
                            role.name,
                            dropped.join(" / ")
                        );
                    }
                    role.defaults.construct.clone()
                }
                None => String::new(),
            }
        };

        {
            let mut world = self.shared.world.write().await;
            world.register_agent(spec)?;
        }

        // Construct.md は `AgentSpec` の欄ではないので、ここでしか書けない。
        // **登録の後**に書くのは、id の検査を通った後でないとディレクトリを
        // 作る先が確定しないため。書き込みの失敗で登録を巻き戻さない —
        // 個体は既に村に居り、本文は設定ダイアログから書き直せる。
        if !construct.trim().is_empty() {
            if let Err(err) = self
                .shared
                .store
                .write_config(&id, ConfigFileKind::Construct, &construct)
                .await
            {
                crate::note!("role apply: Construct.md を書けませんでした: {err}");
            }
        }

        self.persist().await?;
        self.shared.emit(CoreEvent::TopologyChanged);
        self.snapshot(&id).await
    }

    // ---- 役職 (Spec 14) -----------------------------------------------------

    /// 登録済みの役職一覧。
    pub async fn list_roles(&self) -> Vec<AgentRole> {
        self.shared.world.read().await.roles()
    }

    /// 役職を登録または更新する。
    ///
    /// **既存のサーヴァントには何も起きない**（`role_contract` 凍結 4）。
    /// 中身はコピー済みなので、変わるのは `name` を参照している表示だけ。
    pub async fn upsert_role(&self, role: AgentRole) -> CoreResult<()> {
        // 改名は**その役職を持つ全個体**の表示を動かすので、影響範囲を先に取る
        // （機構 7 の発火は「表示名が変わったか」の 1 点で、操作の種類では分けない）。
        let affected = self.holders_of(&role.id, &role.name).await;
        {
            let mut world = self.shared.world.write().await;
            world.upsert_role(role);
        }
        for (id, before) in affected {
            let after = {
                let world = self.shared.world.read().await;
                world
                    .agent(&id)
                    .ok()
                    .and_then(|record| world.role_label(record.spec.role_id.as_ref()))
                    .map(str::to_owned)
            };
            self.announce_role_change(&id, before.as_deref(), after.as_deref())
                .await;
        }
        self.persist().await
    }

    /// その役職を持つ個体と、**変更前の**表示名の対。
    ///
    /// `new_name` は使わない（比較は `announce_role_change` が変更後に行う）が、
    /// 呼び出し側の意図を型で示すために受ける。
    async fn holders_of(&self, role_id: &AgentRoleId, _new_name: &str) -> Vec<(AgentId, Option<String>)> {
        let world = self.shared.world.read().await;
        world
            .snapshots()
            .into_iter()
            .filter(|snapshot| snapshot.role_id.as_ref() == Some(role_id))
            .map(|snapshot| {
                let before = world.role_label(snapshot.role_id.as_ref()).map(str::to_owned);
                (snapshot.id, before)
            })
            .collect()
    }

    /// 役職を削除する。**参照中でも拒まない**（`remove_template` との決定的な差）。
    ///
    /// 役職はコピー済みなので、消してもサーヴァントの動作は変わらない —
    /// バッジと顔ぶれの `[...]` が消えるだけ（`role_contract` 凍結 5）。
    pub async fn remove_role(&self, id: &AgentRoleId) -> CoreResult<()> {
        let affected = self.holders_of(id, "").await;
        {
            let mut world = self.shared.world.write().await;
            world.remove_role(id)?;
        }
        // 削除後は引けなくなるので after は必ず None。
        for (agent_id, before) in affected {
            self.announce_role_change(&agent_id, before.as_deref(), None)
                .await;
        }
        self.persist().await
    }

    /// エージェント定義を差し替える。
    ///
    /// 稼働中でも受け付ける。次の発話から新しい設定が反映される
    /// （プロンプトはメッセージごとに組み直すため）。
    pub async fn update_agent(&self, spec: AgentSpec) -> CoreResult<AgentSnapshot> {
        let id = spec.id.clone();
        // 役職表示（Spec 14）。**設定は 1 欄も流し込まない** — 流し込みの発火点は
        // 新規作成ただ 1 つ（role_contract 凍結 4）。ここで見るのは表示だけ。
        let (before, after) = {
            let mut world = self.shared.world.write().await;
            let before = world
                .agent(&id)
                .ok()
                .and_then(|record| world.role_label(record.spec.role_id.as_ref()))
                .map(str::to_owned);
            world.update_agent(spec)?;
            let after = world
                .agent(&id)
                .ok()
                .and_then(|record| world.role_label(record.spec.role_id.as_ref()))
                .map(str::to_owned);
            (before, after)
        };
        self.announce_role_change(&id, before.as_deref(), after.as_deref())
            .await;
        self.persist().await?;
        self.shared.emit(CoreEvent::TopologyChanged);
        self.snapshot(&id).await
    }

    /// 役職表示が変わったことを System 行 1 本で場に流す（Spec 14 機構 7）。
    ///
    /// **判定は「表示名が変わったか」の 1 点。** 付与・変更（改名を含む）・削除を
    /// 操作の種類で分けない — 分けると「改名だけ通知が出ない」のような穴が空く。
    /// 他のサーヴァントから見れば、どれも「あの個体の役職表示が変わった」で同じ事象。
    ///
    /// **これは保証ではない。** 届くのは `compose_presence_notices` 経由なので、
    /// 広場ログをオプトアウトした個体には届かず、窓から押し出されれば消える。
    /// 「自己申告する」を仕様の約束にはしない（Spec 14 機構 7）。
    async fn announce_role_change(&self, id: &AgentId, before: Option<&str>, after: Option<&str>) {
        if before == after {
            return;
        }
        let name = {
            let world = self.shared.world.read().await;
            match world.agent(id) {
                Ok(record) => record.spec.name.clone(),
                // 個体が消えていれば知らせる相手の話題も消えている。
                Err(_) => return,
            }
        };
        let text = match (before, after) {
            (None, Some(now)) => format!("{id}（{name}）の役職が「{now}」になりました"),
            (Some(_), Some(now)) => format!("{id}（{name}）の役職が「{now}」になりました"),
            (Some(was), None) => format!("{id}（{name}）の役職「{was}」が外れました"),
            (None, None) => return,
        };
        // 入退室通知と同じ経路（from: System / to: User。record のみで配送しない）。
        self.shared
            .record(AgentMessage::new(Endpoint::System, Endpoint::User, text, 0))
            .await;
    }

    /// エージェントを削除する。稼働中なら先に停止する。
    pub async fn delete_agent(&self, id: &AgentId) -> CoreResult<()> {
        self.stop_agent(id).await.ok();
        {
            let mut world = self.shared.world.write().await;
            world.remove_agent(id)?;
        }
        self.shared.store.remove_agent_dir(id).await?;
        self.persist().await?;

        // その宛先の予定も消す（Spec 07。remove_agent が他エージェントからの
        // 参照を外すのと同じ規律 — 参照の回収まで含めて 1 操作）。
        // schedules.json が壊れて書き込み保護中でも削除自体は止めない:
        // in-memory から消せば発火は起きず、ファイルの残骸は保護解除後の
        // 次の保存で消える。
        {
            let mut schedules = self.shared.schedules.write().await;
            let before = schedules.len();
            schedules.retain(|task| task.to != *id);
            if schedules.len() != before && self.shared.schedules_blocked.is_none() {
                self.shared.store.save_schedules(&schedules).await?;
            }
        }

        self.shared.emit(CoreEvent::TopologyChanged);
        Ok(())
    }

    /// 接続先を差し替える。
    pub async fn set_connections(&self, id: &AgentId, targets: Vec<AgentId>) -> CoreResult<()> {
        {
            let mut world = self.shared.world.write().await;
            world.set_connections(id, targets)?;
        }
        self.persist().await?;
        self.shared.emit(CoreEvent::TopologyChanged);
        Ok(())
    }

    /// 表示順を振り直す。
    pub async fn reorder_agents(&self, order: &[AgentId]) -> CoreResult<()> {
        self.shared.world.write().await.reorder(order);
        self.persist().await
    }

    /// 接続マップの保存済みノード座標を返す。
    pub async fn topology_positions(&self) -> BTreeMap<AgentId, TopologyPosition> {
        self.shared.world.read().await.topology_positions()
    }

    /// 接続マップ上で移動したノードの座標を保存する。
    pub async fn set_topology_position(
        &self,
        id: &AgentId,
        position: TopologyPosition,
    ) -> CoreResult<()> {
        self.shared
            .world
            .write()
            .await
            .set_topology_position(id, position)?;
        self.persist().await?;
        self.shared.emit(CoreEvent::TopologyChanged);
        Ok(())
    }

    /// モデルテンプレートを登録または更新する。
    ///
    /// 構築済みバックエンドのキャッシュを破棄する。これを怠ると、
    /// エンドポイントを直したのに古い接続先へ送り続ける（設定が効かない）。
    pub async fn upsert_template(&self, mut template: ModelTemplate) -> CoreResult<()> {
        let id = template.id.clone();
        {
            let mut world = self.shared.world.write().await;

            // `credential` はコアが所有する派生状態。正当な遷移経路は
            // `set_credential` / `clear_credential` と、認証不要チェックボックス由来の
            // unset ⇄ not_required だけ。クライアントの下書きは登録前の古い
            // スナップショットを保持しうるので、ここで素通しにすると
            // 「キーは資格情報ストアに実在するのに設定上は未登録」へ巻き戻る
            // （Gemini テンプレートで実際に起きた。failures.md #16）。
            template.credential = match (
                world.template(&id).map(|t| t.credential).ok(),
                template.credential,
            ) {
                // keyring からの離脱は clear_credential（秘密の削除と一体）に限る。
                (Some(CredentialSource::Keyring), _) => CredentialSource::Keyring,
                // 秘密の裏付けが無い keyring 主張は未登録へ引き戻す。素通しにすると、
                // 保存時に捕まえられる設定不備が送信時の「見つかりません」へずれ込む。
                (previous, CredentialSource::Keyring) => {
                    if self.shared.secrets.contains(id.as_str()).unwrap_or(false) {
                        CredentialSource::Keyring
                    } else {
                        previous.unwrap_or(CredentialSource::Unset)
                    }
                }
                // unset ⇄ not_required はチェックボックスの正当な遷移。
                (_, requested) => requested,
            };

            world.upsert_template(template);
        }
        self.shared.backends.write().await.remove(&id);
        self.persist().await
    }

    /// モデルテンプレートを削除する。参照中のエージェントが居れば拒否される。
    ///
    /// 資格情報ストアの登録も同時に消す。設定だけ消して秘密を残すと、
    /// 画面のどこからも見えない孤児が OS 側に溜まり続ける。
    pub async fn remove_template(&self, id: &ModelTemplateId) -> CoreResult<()> {
        {
            let mut world = self.shared.world.write().await;
            world.remove_template(id)?;
        }
        self.shared.backends.write().await.remove(id);
        self.shared.secrets.delete(id.as_str())?;
        self.persist().await
    }

    // ---- 村の設定 (Spec 13) -------------------------------------------------
}
