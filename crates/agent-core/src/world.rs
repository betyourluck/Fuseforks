//! 登録簿。エージェント定義・モデルテンプレート・実行時統計の保持。
//!
//! ここは**同期的な純データ構造**であり、ロックも非同期も持たない。
//! 排他は [`crate::orchestrator::Orchestrator`] が `RwLock` で外側から掛ける。
//! こうしておくと登録簿の不変条件（重複禁止・トポロジー健全性）を
//! ロックの都合と切り離してテストできる。

use std::collections::BTreeMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult, ErrorPayload};
use crate::llm::ChatMessage;
use crate::model::{
    AgentId, AgentSnapshot, AgentSpec, AgentStatus, ModelTemplate, ModelTemplateId, TopologyEdge,
};

/// 1 エージェントの定義と実行時状態。
#[derive(Debug)]
pub struct AgentRecord {
    /// 永続的な設定。
    pub spec: AgentSpec,
    /// 現在のライフサイクル状態。
    pub status: AgentStatus,
    /// 現在の稼働区間の開始時刻。停止中は `None`。
    pub started_at: Option<Instant>,
    /// 過去の稼働区間の合計。
    pub accumulated_uptime_secs: u64,
    /// 累積トークン数。
    pub total_tokens: u64,
    /// 直近の失敗。
    pub last_error: Option<ErrorPayload>,
    /// 直近の会話履歴（自分の発言を含む）。
    ///
    /// これが無いと、エージェントは毎回コールドスタートになり
    /// **自分が直前に何を言ったかを知らない**。同じ入力に同じ出力を返し続け、
    /// 会話が原理的に収束しなくなる（failures.md #12）。
    ///
    /// プロセス寿命に閉じる。保存しないのは、再開時に古い文脈が復活すると
    /// 「新しく始めたつもりが続きだった」という分かりにくい状態になるため。
    pub history: Vec<ChatMessage>,
}

impl AgentRecord {
    /// 定義から停止状態のレコードを作る。
    fn new(spec: AgentSpec) -> Self {
        Self {
            spec,
            status: AgentStatus::Idle,
            started_at: None,
            accumulated_uptime_secs: 0,
            total_tokens: 0,
            last_error: None,
            history: Vec::new(),
        }
    }

    /// 現時点の累積稼働秒数。稼働中なら進行中の区間を足して返す。
    pub fn uptime_secs(&self) -> u64 {
        let current = self
            .started_at
            .map_or(0, |start| start.elapsed().as_secs());
        self.accumulated_uptime_secs + current
    }

    /// 1 往復を履歴へ積み、直近 `max_turns` 往復だけ残す。
    ///
    /// 古いほうから捨てる。長時間の稼働で履歴が際限なく伸びると、
    /// プロンプトがコンテキスト長を超えて必ず失敗するようになる。
    pub fn push_exchange(&mut self, received: &str, replied: &str, max_turns: usize) {
        self.history.push(ChatMessage::user(received));
        self.history.push(ChatMessage::assistant(replied));

        let limit = max_turns.saturating_mul(2);
        if limit == 0 {
            self.history.clear();
        } else if self.history.len() > limit {
            self.history.drain(..self.history.len() - limit);
        }
    }
}

/// 永続化される世界の状態。
///
/// `Instant` は直列化できないため、保存対象は定義とテンプレートのみ。
/// 稼働時間の累積はプロセス寿命に閉じる（再起動でリセットされる）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedWorld {
    /// エージェント定義。
    #[serde(default)]
    pub agents: Vec<AgentSpec>,
    /// モデルテンプレート。
    #[serde(default)]
    pub model_templates: Vec<ModelTemplate>,
}

/// 登録簿本体。
#[derive(Debug, Default)]
pub struct World {
    agents: BTreeMap<AgentId, AgentRecord>,
    templates: BTreeMap<ModelTemplateId, ModelTemplate>,
}

impl World {
    /// 空の登録簿を作る。
    pub fn new() -> Self {
        Self::default()
    }

    /// 永続化された状態から復元する。
    ///
    /// 復元時にトポロジーの健全性を検査し、**未登録先への接続は黙って落とす**。
    /// 保存後に相手が削除された場合に、ファイルを開けなくなるほうが害が大きい。
    pub fn from_persisted(persisted: PersistedWorld) -> Self {
        let known: Vec<AgentId> = persisted.agents.iter().map(|s| s.id.clone()).collect();

        let mut world = Self::new();
        for template in persisted.model_templates {
            world.templates.insert(template.id.clone(), template);
        }
        for mut spec in persisted.agents {
            spec.connected_agents
                .retain(|target| *target != spec.id && known.contains(target));
            world.agents.insert(spec.id.clone(), AgentRecord::new(spec));
        }
        world
    }

    /// 永続化用の表現へ落とす。
    pub fn to_persisted(&self) -> PersistedWorld {
        PersistedWorld {
            agents: self.agents.values().map(|r| r.spec.clone()).collect(),
            model_templates: self.templates.values().cloned().collect(),
        }
    }

    // ---- エージェント -------------------------------------------------------

    /// エージェントを登録する。
    ///
    /// # Errors
    /// - ID が既に使われている場合 [`CoreError::DuplicateAgent`]
    /// - ID がパスとして安全でない場合 [`CoreError::UnsafeIdentifier`]
    /// - 参照するモデルテンプレートが無い場合 [`CoreError::ModelTemplateNotFound`]
    /// - 接続先が不正な場合 [`CoreError::InvalidTopology`]
    pub fn register_agent(&mut self, spec: AgentSpec) -> CoreResult<()> {
        if !spec.id.is_safe() {
            return Err(CoreError::UnsafeIdentifier {
                value: spec.id.to_string(),
            });
        }
        if self.agents.contains_key(&spec.id) {
            return Err(CoreError::DuplicateAgent(spec.id.to_string()));
        }
        if !self.templates.contains_key(&spec.model_template_id) {
            return Err(CoreError::ModelTemplateNotFound(
                spec.model_template_id.to_string(),
            ));
        }
        self.validate_connections(&spec.id, &spec.connected_agents)?;

        self.agents.insert(spec.id.clone(), AgentRecord::new(spec));
        Ok(())
    }

    /// エージェント定義を差し替える。統計と稼働状態は保持する。
    pub fn update_agent(&mut self, spec: AgentSpec) -> CoreResult<()> {
        if !self.agents.contains_key(&spec.id) {
            return Err(CoreError::AgentNotFound(spec.id.to_string()));
        }
        if !self.templates.contains_key(&spec.model_template_id) {
            return Err(CoreError::ModelTemplateNotFound(
                spec.model_template_id.to_string(),
            ));
        }
        self.validate_connections(&spec.id, &spec.connected_agents)?;

        if let Some(record) = self.agents.get_mut(&spec.id) {
            record.spec = spec;
        }
        Ok(())
    }

    /// エージェントを削除し、他エージェントからの参照も同時に外す。
    ///
    /// 参照の掃除を怠ると、削除済みの相手へ送ろうとする経路が残る。
    /// 削除は「消す」だけでなく「参照を回収する」まで含めて 1 操作。
    pub fn remove_agent(&mut self, id: &AgentId) -> CoreResult<()> {
        if self.agents.remove(id).is_none() {
            return Err(CoreError::AgentNotFound(id.to_string()));
        }
        for record in self.agents.values_mut() {
            record.spec.connected_agents.retain(|target| target != id);
        }
        Ok(())
    }

    /// 接続先を差し替える。
    pub fn set_connections(&mut self, id: &AgentId, targets: Vec<AgentId>) -> CoreResult<()> {
        self.validate_connections(id, &targets)?;
        let record = self
            .agents
            .get_mut(id)
            .ok_or_else(|| CoreError::AgentNotFound(id.to_string()))?;
        record.spec.connected_agents = targets;
        Ok(())
    }

    /// 表示順を与えられた並びで振り直す。列挙に無い ID は末尾へ回す。
    pub fn reorder(&mut self, order: &[AgentId]) {
        for (index, id) in order.iter().enumerate() {
            if let Some(record) = self.agents.get_mut(id) {
                record.spec.order = index as u32;
            }
        }
        let tail = order.len() as u32;
        for record in self.agents.values_mut() {
            if !order.contains(&record.spec.id) {
                record.spec.order = tail;
            }
        }
    }

    /// レコードを借用する。
    pub fn agent(&self, id: &AgentId) -> CoreResult<&AgentRecord> {
        self.agents
            .get(id)
            .ok_or_else(|| CoreError::AgentNotFound(id.to_string()))
    }

    /// レコードを可変借用する。
    pub fn agent_mut(&mut self, id: &AgentId) -> CoreResult<&mut AgentRecord> {
        self.agents
            .get_mut(id)
            .ok_or_else(|| CoreError::AgentNotFound(id.to_string()))
    }

    /// 登録済みエージェント数。
    pub fn agent_count(&self) -> usize {
        self.agents.len()
    }

    /// 表示順に並べた UI 向けスナップショット。
    pub fn snapshots(&self) -> Vec<AgentSnapshot> {
        let mut list: Vec<AgentSnapshot> = self.agents.values().map(|r| self.snapshot_of(r)).collect();
        list.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.id.cmp(&b.id)));
        list
    }

    /// 単一エージェントのスナップショット。
    pub fn snapshot(&self, id: &AgentId) -> CoreResult<AgentSnapshot> {
        Ok(self.snapshot_of(self.agent(id)?))
    }

    fn snapshot_of(&self, record: &AgentRecord) -> AgentSnapshot {
        AgentSnapshot {
            id: record.spec.id.clone(),
            name: record.spec.name.clone(),
            // テンプレートが失われていても一覧描画を止めない。欠落は表示で見せる。
            model: self
                .templates
                .get(&record.spec.model_template_id)
                .map_or_else(|| "<unknown>".to_owned(), |t| t.model.clone()),
            model_template_id: record.spec.model_template_id.clone(),
            status: record.status,
            uptime_secs: record.uptime_secs(),
            total_tokens: record.total_tokens,
            rag_sources: record.spec.rag_sources.clone(),
            connected_agents: record.spec.connected_agents.clone(),
            order: record.spec.order,
            work_dir: record.spec.work_dir.clone(),
            last_error: record.last_error.clone(),
        }
    }

    /// トポロジーの全辺。Vue Flow のエッジ生成に使う。
    pub fn edges(&self) -> Vec<TopologyEdge> {
        self.agents
            .values()
            .flat_map(|record| {
                record
                    .spec
                    .connected_agents
                    .iter()
                    .map(move |target| TopologyEdge {
                        source: record.spec.id.clone(),
                        target: target.clone(),
                    })
            })
            .collect()
    }

    /// 指定エージェントの接続先を複製して返す。
    pub fn connections_of(&self, id: &AgentId) -> CoreResult<Vec<AgentId>> {
        Ok(self.agent(id)?.spec.connected_agents.clone())
    }

    /// 接続関係の健全性を検査する。
    ///
    /// 弾くのは自己ループと未登録先への接続だけ。**循環は許す** —
    /// エージェント同士が往復するのはこのシステムの目的そのものであり、
    /// 無限往復は転送回数の上限（hop）で止めるのが正しい層。
    fn validate_connections(&self, owner: &AgentId, targets: &[AgentId]) -> CoreResult<()> {
        for target in targets {
            if target == owner {
                return Err(CoreError::InvalidTopology {
                    reason: format!("エージェント `{owner}` が自分自身に接続しています"),
                });
            }
            if !self.agents.contains_key(target) {
                return Err(CoreError::InvalidTopology {
                    reason: format!("接続先 `{target}` は登録されていません"),
                });
            }
        }
        Ok(())
    }

    // ---- モデルテンプレート -------------------------------------------------

    /// テンプレートを登録または更新する。
    ///
    /// 秘密の書式検査はもう要らない。[`ModelTemplate`] に秘密を置ける場所が無く、
    /// 実値は OS の資格情報ストアにしか入らないため、
    /// この経路を通って平文の設定ファイルへ秘密が入ることは構造上ありえない。
    pub fn upsert_template(&mut self, template: ModelTemplate) {
        self.templates.insert(template.id.clone(), template);
    }

    /// テンプレートを削除する。
    ///
    /// # Errors
    /// 参照中のエージェントが 1 体でも居れば [`CoreError::InvalidTopology`] で拒否する。
    /// 参照を残したまま消せると、そのエージェントは起動した瞬間に必ず失敗する。
    pub fn remove_template(&mut self, id: &ModelTemplateId) -> CoreResult<()> {
        let referencing: Vec<String> = self
            .agents
            .values()
            .filter(|r| r.spec.model_template_id == *id)
            .map(|r| r.spec.name.clone())
            .collect();

        if !referencing.is_empty() {
            return Err(CoreError::InvalidTopology {
                reason: format!(
                    "モデルテンプレート `{id}` は {} が参照中です",
                    referencing.join(", ")
                ),
            });
        }
        if self.templates.remove(id).is_none() {
            return Err(CoreError::ModelTemplateNotFound(id.to_string()));
        }
        Ok(())
    }

    /// テンプレートを借用する。
    pub fn template(&self, id: &ModelTemplateId) -> CoreResult<&ModelTemplate> {
        self.templates
            .get(id)
            .ok_or_else(|| CoreError::ModelTemplateNotFound(id.to_string()))
    }

    /// 全テンプレート。
    pub fn templates(&self) -> Vec<ModelTemplate> {
        self.templates.values().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world_with_two_agents() -> World {
        let mut world = World::new();
        world.upsert_template(ModelTemplate::new("tpl", "既定", "gpt-4o"));
        world
            .register_agent(AgentSpec::new("agent_01", "Planner", "tpl"))
            .unwrap();
        world
            .register_agent(AgentSpec::new("agent_02", "Critic", "tpl"))
            .unwrap();
        world
    }

    #[test]
    fn duplicate_registration_is_rejected() {
        let mut world = world_with_two_agents();
        let err = world
            .register_agent(AgentSpec::new("agent_01", "重複", "tpl"))
            .unwrap_err();
        assert_eq!(err.code(), "DUPLICATE_AGENT");
    }

    #[test]
    fn unsafe_identifier_is_rejected_before_touching_the_filesystem() {
        let mut world = world_with_two_agents();
        let err = world
            .register_agent(AgentSpec::new("../escape", "悪い名前", "tpl"))
            .unwrap_err();
        assert_eq!(err.code(), "UNSAFE_IDENTIFIER");
    }

    #[test]
    fn missing_template_is_rejected() {
        let mut world = world_with_two_agents();
        let err = world
            .register_agent(AgentSpec::new("agent_03", "孤児", "missing_tpl"))
            .unwrap_err();
        assert_eq!(err.code(), "MODEL_TEMPLATE_NOT_FOUND");
    }

    #[test]
    fn self_loop_is_rejected_but_cycles_are_allowed() {
        let mut world = world_with_two_agents();

        let err = world
            .set_connections(&"agent_01".into(), vec!["agent_01".into()])
            .unwrap_err();
        assert_eq!(err.code(), "INVALID_TOPOLOGY");

        // 相互接続（循環）は正当な構成として通す。
        world
            .set_connections(&"agent_01".into(), vec!["agent_02".into()])
            .unwrap();
        world
            .set_connections(&"agent_02".into(), vec!["agent_01".into()])
            .unwrap();
        assert_eq!(world.edges().len(), 2);
    }

    #[test]
    fn connection_to_unknown_agent_is_rejected() {
        let mut world = world_with_two_agents();
        let err = world
            .set_connections(&"agent_01".into(), vec!["ghost".into()])
            .unwrap_err();
        assert_eq!(err.code(), "INVALID_TOPOLOGY");
    }

    #[test]
    fn removing_an_agent_also_removes_inbound_references() {
        let mut world = world_with_two_agents();
        world
            .set_connections(&"agent_01".into(), vec!["agent_02".into()])
            .unwrap();

        world.remove_agent(&"agent_02".into()).unwrap();

        assert_eq!(world.agent_count(), 1);
        assert!(world.edges().is_empty(), "参照が残らないこと");
    }

    #[test]
    fn template_in_use_cannot_be_removed() {
        let mut world = world_with_two_agents();
        let err = world.remove_template(&"tpl".into()).unwrap_err();

        assert_eq!(err.code(), "INVALID_TOPOLOGY");
        assert!(err.to_string().contains("Planner"), "参照元を名指しすること");
    }

    #[test]
    fn reorder_assigns_indices_and_pushes_unlisted_to_the_tail() {
        let mut world = world_with_two_agents();
        world
            .register_agent(AgentSpec::new("agent_03", "Third", "tpl"))
            .unwrap();

        world.reorder(&["agent_02".into(), "agent_01".into()]);

        let ids: Vec<String> = world.snapshots().iter().map(|s| s.id.to_string()).collect();
        assert_eq!(ids, vec!["agent_02", "agent_01", "agent_03"]);
    }

    #[test]
    fn snapshot_survives_a_missing_template() {
        let mut world = world_with_two_agents();
        world.remove_agent(&"agent_02".into()).unwrap();
        // 参照元を消してからテンプレートを消す（正規経路）。
        world.remove_agent(&"agent_01".into()).unwrap();
        world.upsert_template(ModelTemplate::new("tpl2", "別", "claude-opus-5"));
        world
            .register_agent(AgentSpec::new("agent_09", "Orphan", "tpl2"))
            .unwrap();
        world.remove_template(&"tpl".into()).unwrap();

        assert_eq!(world.snapshots()[0].model, "claude-opus-5");
    }

    #[test]
    fn persisted_roundtrip_drops_dangling_connections() {
        let persisted = PersistedWorld {
            agents: vec![
                {
                    let mut s = AgentSpec::new("agent_01", "Planner", "tpl");
                    s.connected_agents = vec!["agent_02".into(), "ghost".into()];
                    s
                },
                AgentSpec::new("agent_02", "Critic", "tpl"),
            ],
            model_templates: vec![ModelTemplate::new("tpl", "既定", "gpt-4o")],
        };

        let world = World::from_persisted(persisted);
        let edges = world.edges();

        assert_eq!(edges.len(), 1, "`ghost` への辺は落ちる");
        assert_eq!(edges[0].target, AgentId::from("agent_02"));
    }
}
