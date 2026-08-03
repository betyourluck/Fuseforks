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
    /// うち入力トークン数（プロンプト側）。
    ///
    /// キャッシュの効きを測る分母。**出力はキャッシュできない**ので、
    /// 合計を分母にすると天井が 100% にならず、どこまで取り残しているのかが
    /// 読めない。入力を分けて持てば「入力の何 % が読み取りで済んだか」になる。
    pub prompt_tokens: u64,
    /// うちプロンプトキャッシュから読まれた入力トークン数。
    ///
    /// 合計だけでは、**キャッシュが一度も効いていない状態と完全に効いている状態が
    /// 同じ数字に見える**。実機で 5 体全員が無キャッシュのまま数日走っており、
    /// 気づいたのは請求ダッシュボードのグラフからだった（failures.md #33）。
    /// 割合を画面に出せば、設定を変えた次のターンで分かる。
    pub cached_tokens: u64,
    /// 直近の失敗。
    pub last_error: Option<ErrorPayload>,
    /// 直近の会話履歴（自分の発言を含む）。
    ///
    /// これが無いと、エージェントは毎回コールドスタートになり
    /// **自分が直前に何を言ったかを知らない**。同じ入力に同じ出力を返し続け、
    /// 会話が原理的に収束しなくなる（failures.md #12）。
    ///
    /// **セッションの寿命に閉じる**（Spec 12 で変更。それ以前はプロセス寿命だった）。
    ///
    /// `sessions.redb` の `exchange` レコードから再起動時に復元される。
    /// **会話ログからは復元できない** — ここには #45 の規律で「送った文字列
    /// そのもの」（畳んだ可変文脈込み）が入り、その文字列は `Shared.log` の
    /// どこにも無い。会話ログだけ戻すと、画面は正しいのに全員が健忘症で始まる。
    ///
    /// 始め直したいときは「新規チャット」（= 新しいセッション）を使う。
    /// エージェントの起動・停止では消えない。
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
            prompt_tokens: 0,
            cached_tokens: 0,
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
    ///
    /// **空の発言は空のまま積まない。** 履歴の空メッセージは次のターンの
    /// リクエストに空テキストブロックとして混入し、プロバイダによっては
    /// 400 で拒否される（Anthropic の実測。failures.md #29）。往復の対を
    /// 崩すと役割の交互性が壊れるため、落とすのではなく目印へ置き換える。
    pub fn push_exchange(&mut self, received: &str, replied: &str, max_turns: usize) {
        let [user, assistant] = exchange_pair(received, replied);
        self.history.push(user);
        self.history.push(assistant);

        let limit = max_turns.saturating_mul(2);
        if limit == 0 {
            self.history.clear();
        } else if self.history.len() > limit {
            self.history.drain(..self.history.len() - limit);
        }
    }
}

/// 1 往復を [`ChatMessage`] の対（user → assistant）へ落とす。
///
/// **空の発言は空のまま積まない。** 履歴の空メッセージは次のターンのリクエストに
/// 空テキストブロックとして混入し、プロバイダによっては 400 で拒否される
/// （Anthropic の実測。failures.md #29）。往復の対を崩すと役割の交互性が壊れるため、
/// 落とすのではなく目印へ置き換える。
///
/// [`AgentRecord::push_exchange`]（実行中に積む側）と
/// [`crate::session_store::SessionStore::restore_histories`]（保存から読み戻す側）が
/// **同じ規律で組む**必要があるため、実装をここ 1 箇所に置く。分けて書くと、
/// 復元した履歴だけが空メッセージを持って次のターンで 400 になる。
pub fn exchange_pair(received: &str, replied: &str) -> [ChatMessage; 2] {
    let placeholder = "（発言なし）";
    let received = if received.trim().is_empty() { placeholder } else { received };
    let replied = if replied.trim().is_empty() { placeholder } else { replied };
    [ChatMessage::user(received), ChatMessage::assistant(replied)]
}

/// 接続マップ上のノード座標。
///
/// 稼働状態と違い、再起動後にも意味が残る表示設定。座標の真実はこの型にだけ置き、
/// UI は world.json の投影として復元する。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopologyPosition {
    /// Vue Flow 座標系の横位置。
    pub x: f64,
    /// Vue Flow 座標系の縦位置。
    pub y: f64,
}

/// UI の表示言語（Spec 13 の settings_contract）。
///
/// **コアはこの値で分岐しない。** 多言語化 3 層の (2) は「コアは日本語のまま返し、
/// UI が `ErrorPayload.code` で引いて訳す」（案 A — コアは言語を知らない）。
/// コアの仕事は村の共有物としての保存だけ。System 行は会話ログに保存されるため、
/// この値はペイン幅と同じ棚（`localStorage`）には置けない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// 日本語。
    Ja,
    /// 英語。
    En,
}

impl Language {
    /// OS のロケール文字列から確定させる（純関数 — 検出は呼び出し側の責務）。
    ///
    /// `ja-JP` / `ja_JP` / `ja` の表記揺れは前方一致で吸収する。日本語以外は
    /// すべて英語へ倒す（選択肢は 2 つだけ。settings_contract）。
    pub fn from_os_locale(locale: Option<&str>) -> Self {
        match locale {
            Some(s) if s.starts_with("ja") => Language::Ja,
            _ => Language::En,
        }
    }

    /// ワイヤ値（`"ja"` / `"en"`）から読む。未知の値は `None`。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "ja" => Some(Language::Ja),
            "en" => Some(Language::En),
            _ => None,
        }
    }

    /// ワイヤ値。serde の `lowercase` と一致をテストで固定している。
    pub fn as_str(self) -> &'static str {
        match self {
            Language::Ja => "ja",
            Language::En => "en",
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
    /// 接続マップ上のノード座標。表示設定なので AgentSpec には含めない。
    #[serde(default)]
    pub topology_positions: BTreeMap<AgentId, TopologyPosition>,
    /// トークン予算の天井（Spec 11。実効トークン建て・村レベル）。
    ///
    /// `None` = 天井なし / `Some(n)` = 天井あり。**0 のマジック値は使わない** —
    /// `Some(0)` は読み込みで `None` へ正規化される。既定 `Some(1_000_000)` を
    /// 書くのは新規 world.json の作成時だけ（既存の村の挙動を黙って変えない）。
    /// `rename_all = camelCase` によりファイル上は `tokenBudget`（個別 rename 不要）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<u64>,
    /// UI の表示言語（Spec 13。`"ja"` / `"en"`）。
    ///
    /// **生の文字列で受ける**（`tokenBudget` の `Some(0)` と同じ判断） —
    /// 手編集の未知の値で world.json が開けなくなるのは罰が重すぎる。
    /// 解釈と正規化は [`World::from_persisted`] が担い、不正値は「未確定」として
    /// 起動時に OS から確定し直される。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

/// 登録簿本体。
#[derive(Debug, Default)]
pub struct World {
    agents: BTreeMap<AgentId, AgentRecord>,
    templates: BTreeMap<ModelTemplateId, ModelTemplate>,
    topology_positions: BTreeMap<AgentId, TopologyPosition>,
    /// トークン予算の天井（Spec 11）。意味論は [`PersistedWorld::token_budget`]。
    token_budget: Option<u64>,
    /// UI の表示言語（Spec 13）。`None` = 未確定（起動時に OS から確定される）。
    language: Option<Language>,
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
        // Some(0) は「即打ち切りの村」ではなく不正値 — None（天井なし）へ倒す
        // （token_budget 契約の ceiling。0 のマジック値を作らない）。
        world.token_budget = match persisted.token_budget {
            Some(0) => {
                crate::note!("token budget: tokenBudget=0 は不正値のため天井なしとして扱います");
                None
            }
            other => other,
        };
        // 未知の言語コードは「未確定」へ倒す（起動時に OS から確定し直される —
        // 黙って ja / en のどちらかへ貼り付けるより、未確定と同じ道を通すほうが
        // 手編集した人の意図（何かを変えたかった）に近い挙動になる）。
        world.language = match persisted.language.as_deref() {
            Some(raw) => {
                let parsed = Language::parse(raw);
                if parsed.is_none() {
                    crate::note!(
                        "language: `{raw}` は未知の値のため未確定として扱います（ja / en のみ）"
                    );
                }
                parsed
            }
            None => None,
        };
        world.topology_positions = persisted.topology_positions.clone();
        for template in persisted.model_templates {
            world.templates.insert(template.id.clone(), template);
        }
        for mut spec in persisted.agents {
            spec.connected_agents
                .retain(|target| *target != spec.id && known.contains(target));
            // 意図的に register_agent を通さない: 表示名の重複検査（書き込み時は
            // 拒否）を読み込みには適用しない。過去に作られた重複で world.json が
            // 開けなくなるのは、検査の目的（新しい重複を作らない）を超える罰になる。
            world.agents.insert(spec.id.clone(), AgentRecord::new(spec));
        }
        world
            .topology_positions
            .retain(|id, _| known.contains(id));
        world
    }

    /// 永続化用の表現へ落とす。
    pub fn to_persisted(&self) -> PersistedWorld {
        PersistedWorld {
            agents: self.agents.values().map(|r| r.spec.clone()).collect(),
            model_templates: self.templates.values().cloned().collect(),
            topology_positions: self.topology_positions.clone(),
            token_budget: self.token_budget,
            language: self.language.map(|l| l.as_str().to_string()),
        }
    }

    /// トークン予算の天井（実効トークン建て）。`None` = 天井なし。
    pub fn token_budget(&self) -> Option<u64> {
        self.token_budget
    }

    /// トークン予算の天井を差し替える（新規 world.json への既定値書き込み用）。
    pub fn set_token_budget(&mut self, ceiling: Option<u64>) {
        self.token_budget = ceiling;
    }

    /// UI の表示言語。`None` = 未確定（起動時の確定前だけ観測される）。
    pub fn language(&self) -> Option<Language> {
        self.language
    }

    /// UI の表示言語を確定させる。
    pub fn set_language(&mut self, language: Language) {
        self.language = Some(language);
    }

    // ---- エージェント -------------------------------------------------------

    /// 表示名が他のエージェントと衝突していないか。
    ///
    /// **表示名は会話・束ね・入退室通知・顔ぶれの語彙**であり、重複すると
    /// それら全部が「どちらの話か」を失う。ID の一意性は map の鍵で構造的に
    /// 保たれるが、名前はただのフィールドなので、書き込みの入口で確かめる。
    ///
    /// 判定は完全一致（trim 後）。全角/半角の正規化まではしない —
    /// 「ロボットくん1号」と「ロボットくん１号」を同一視する規則は、
    /// どこまで畳むかの線引きが恣意的になり、利用者の意図した区別を潰しうる。
    fn name_taken(&self, name: &str, excluding: &AgentId) -> bool {
        let name = name.trim();
        self.agents
            .iter()
            .any(|(id, record)| id != excluding && record.spec.name.trim() == name)
    }

    /// エージェントを登録する。
    ///
    /// # Errors
    /// - ID が既に使われている場合 [`CoreError::DuplicateAgent`]
    /// - 表示名が既に使われている場合 [`CoreError::DuplicateAgentName`]
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
        if self.name_taken(&spec.name, &spec.id) {
            return Err(CoreError::DuplicateAgentName(spec.name.clone()));
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
        // 改名も同じ入口で守る。登録時だけ確かめると、重複は改名経由で必ず入る
        // （外部が書いたデータの転送層では、除外リストは必ずもう一度落ちる —
        // failures.md #30 と同じ形の穴を、時間差で作らない）。
        if self.name_taken(&spec.name, &spec.id) {
            return Err(CoreError::DuplicateAgentName(spec.name.clone()));
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
        self.topology_positions.remove(id);
        for record in self.agents.values_mut() {
            record.spec.connected_agents.retain(|target| target != id);
        }
        Ok(())
    }

    /// 接続マップの座標を返す。未配置のエージェントは UI が自動配置する。
    pub fn topology_positions(&self) -> BTreeMap<AgentId, TopologyPosition> {
        self.topology_positions.clone()
    }

    /// 接続マップ上の 1 ノードの座標を保存する。
    pub fn set_topology_position(
        &mut self,
        id: &AgentId,
        position: TopologyPosition,
    ) -> CoreResult<()> {
        self.agent(id)?;
        self.topology_positions.insert(id.clone(), position);
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

    /// 全エージェントの会話履歴をクリアする（新規チャット。Spec 03）。
    ///
    /// 触るのは `history` だけ — 稼働状態・累積統計はエージェントの属性で
    /// あって会話の属性ではない。
    pub fn clear_histories(&mut self) {
        for record in self.agents.values_mut() {
            record.history.clear();
        }
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
            prompt_tokens: record.prompt_tokens,
            cached_tokens: record.cached_tokens,
            rag_sources: record.spec.rag_sources.clone(),
            connected_agents: record.spec.connected_agents.clone(),
            order: record.spec.order,
            work_dir: record.spec.work_dir.clone(),
            max_tool_iterations: record.spec.max_tool_iterations,
            enabled_tools: record.spec.enabled_tools.clone(),
            hears_room_log: record.spec.hears_room_log,
            batch_start: record.spec.batch_start,
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

    /// 表示名の重複を書き込みの入口で弾くこと（Spec 06）。
    ///
    /// 表示名は会話・束ね・入退室通知・顔ぶれの語彙で、重複するとそれら全部が
    /// 「どちらの話か」を失う。ID と違い構造では守られない。
    #[test]
    fn a_duplicate_display_name_is_rejected_on_register() {
        let mut world = world_with_two_agents();
        let err = world
            .register_agent(AgentSpec::new("agent_03", "Planner", "tpl"))
            .unwrap_err();
        assert_eq!(err.code(), "DUPLICATE_AGENT_NAME");

        // 前後の空白だけの違いは同名として扱う（見た目で区別できない）。
        let err = world
            .register_agent(AgentSpec::new("agent_03", " Planner ", "tpl"))
            .unwrap_err();
        assert_eq!(err.code(), "DUPLICATE_AGENT_NAME");
    }

    /// 改名も同じ入口で守ること。登録時だけ確かめると重複は改名経由で必ず入る。
    #[test]
    fn renaming_to_an_existing_display_name_is_rejected() {
        let mut world = world_with_two_agents();
        let err = world
            .update_agent(AgentSpec::new("agent_02", "Planner", "tpl"))
            .unwrap_err();
        assert_eq!(err.code(), "DUPLICATE_AGENT_NAME");

        // 自分自身の名前を保ったままの更新は通る（自分は衝突相手ではない）。
        world
            .update_agent(AgentSpec::new("agent_02", "Critic", "tpl"))
            .expect("同名のままの更新は正当");
    }

    /// 過去に作られた重複を含む world.json は開けること（読み込みは寛容）。
    ///
    /// 検査の目的は「新しい重複を作らない」であって、既存データへの罰ではない。
    #[test]
    fn a_persisted_world_with_duplicate_names_still_opens() {
        let persisted = PersistedWorld {
            agents: vec![
                AgentSpec::new("agent_01", "ロボットくん", "tpl"),
                AgentSpec::new("agent_02", "ロボットくん", "tpl"),
            ],
            model_templates: vec![ModelTemplate::new("tpl", "既定", "gpt-4o")],
            topology_positions: BTreeMap::new(),
            token_budget: None,
            language: None,
        };
        let world = World::from_persisted(persisted);
        assert_eq!(world.snapshots().len(), 2, "重複していても両方読めること");
    }

    /// 言語の読み込みは寛容に、書き出しは正規形で（Spec 13 の settings_contract）。
    ///
    /// 未知の値は「未確定」へ倒す — 黙って ja / en のどちらかへ貼り付けると、
    /// 手編集した人の「何かを変えたかった」意図ごと消える。未確定は起動時に
    /// OS から確定し直される（tokenBudget=0 → None と同じ道）。
    #[test]
    fn an_unknown_language_normalizes_to_undetermined_on_load() {
        let unknown = PersistedWorld {
            language: Some("jp".into()),
            ..Default::default()
        };
        assert_eq!(World::from_persisted(unknown).language(), None);

        let valid = PersistedWorld {
            language: Some("en".into()),
            ..Default::default()
        };
        assert_eq!(World::from_persisted(valid).language(), Some(Language::En));

        let mut world = World::new();
        world.set_language(Language::Ja);
        assert_eq!(world.to_persisted().language.as_deref(), Some("ja"));
    }

    /// OS ロケールの表記揺れ（`ja-JP` / `ja_JP` / `ja`）は前方一致で吸収し、
    /// 日本語以外は取得失敗も含めてすべて英語へ倒す（選択肢は 2 つだけ）。
    #[test]
    fn os_locale_variants_resolve_to_two_languages_only() {
        for ja in ["ja", "ja-JP", "ja_JP"] {
            assert_eq!(Language::from_os_locale(Some(ja)), Language::Ja, "{ja}");
        }
        for other in ["en-US", "zh-Hans-CN", "de-DE", "fr"] {
            assert_eq!(Language::from_os_locale(Some(other)), Language::En, "{other}");
        }
        assert_eq!(Language::from_os_locale(None), Language::En);
    }

    /// `as_str` と serde の直列化値の一致を固定する（Effort の `xhigh` で
    /// 実際に食い違った形 — ワイヤ値の二重定義はテストでしか守れない）。
    #[test]
    fn language_wire_values_match_serde() {
        for lang in [Language::Ja, Language::En] {
            let json = serde_json::to_value(lang).unwrap();
            assert_eq!(json.as_str(), Some(lang.as_str()));
            assert_eq!(Language::parse(lang.as_str()), Some(lang));
        }
    }

    /// `tokenBudget: 0` は「即打ち切りの村」ではなく不正値 — 読み込みで
    /// 天井なし（`None`）へ倒す（token_budget 契約の ceiling。マジック値を
    /// 作らない）。正の値と `None` はそのまま通る。
    #[test]
    fn a_zero_token_budget_normalizes_to_none_on_load() {
        let zero = PersistedWorld {
            token_budget: Some(0),
            ..Default::default()
        };
        assert_eq!(World::from_persisted(zero).token_budget(), None);

        let set = PersistedWorld {
            token_budget: Some(1_000_000),
            ..Default::default()
        };
        let world = World::from_persisted(set);
        assert_eq!(world.token_budget(), Some(1_000_000));
        // 保存表現へも往復する（新規の村の既定値がディスクへ届く経路）。
        assert_eq!(world.to_persisted().token_budget, Some(1_000_000));

        let unset = PersistedWorld::default();
        assert_eq!(World::from_persisted(unset).token_budget(), None);
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
            topology_positions: BTreeMap::from([
                (AgentId::from("agent_01"), TopologyPosition { x: 120.0, y: 80.0 }),
                (AgentId::from("ghost"), TopologyPosition { x: 0.0, y: 0.0 }),
            ]),
            token_budget: None,
            language: None,
        };

        let world = World::from_persisted(persisted);
        let edges = world.edges();

        assert_eq!(edges.len(), 1, "`ghost` への辺は落ちる");
        assert_eq!(edges[0].target, AgentId::from("agent_02"));
        assert_eq!(
            world.topology_positions(),
            BTreeMap::from([(
                AgentId::from("agent_01"),
                TopologyPosition { x: 120.0, y: 80.0 },
            )]),
            "存在しないエージェントの座標は復元時に落とす"
        );
    }

    #[test]
    fn topology_positions_round_trip_and_are_removed_with_the_agent() {
        let mut world = world_with_two_agents();
        let planner = AgentId::from("agent_01");
        let position = TopologyPosition { x: 240.0, y: 180.0 };

        world.set_topology_position(&planner, position).unwrap();
        assert_eq!(world.to_persisted().topology_positions.get(&planner), Some(&position));

        world.remove_agent(&planner).unwrap();
        assert!(world.topology_positions().is_empty());
    }
}
