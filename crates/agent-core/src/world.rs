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
    AgentId, AgentRole, AgentRoleId, AgentSnapshot, AgentSpec, AgentStatus, ModelTemplate,
    ModelTemplateId, TopologyEdge,
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

/// 利用者の呼び名の字数上限（`user_identity_contract` 凍結 4）。
///
/// 呼び名は封筒 `【送り手: {名前}】` として**毎ターン・全宛先**に乗るので、
/// 長い名前は全ターンの固定費になる。
pub const USER_NAME_MAX_CHARS: usize = 32;

/// 封筒の閉じ括弧。呼び名に含めると 1 つの発話に封筒が 2 つあるように読める。
const USER_NAME_RESERVED: char = '】';

/// 利用者の呼び名を正規化して検証する（`user_identity_contract` 凍結 4）。
///
/// 成功したら **trim 済みの文字列**を返す。失敗したら拒否の理由を返す —
/// **理由に入力値そのものを載せない**（拒否の過程で壊れた値を再放流しない）。
///
/// 純関数として切り出してあるのは、保存経路（`set_user_name`）と読み込み経路
/// （[`World::from_persisted`] の遡及回収）の**両方が同じ述語を通す**ため。
/// 入口だけを塞ぐと、塞ぐ前に手で書かれた値がそのまま封筒へ流れる。
///
/// # 字数はコードポイントで数える
///
/// [`str::len`] は UTF-8 のバイト長なので、日本語なら**上限の 1/3 で発火する**。
/// `MAX_*_CHARS` という名前の定数を `len()` と突き合わせる誤りは、この村の査読で
/// 2 回続けて出ている（ASCII だけのテストでは通ってしまう）。
pub fn normalize_user_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("空にはできません。呼び名を消すには「既定へ戻す」を選んでください".to_owned());
    }
    if name.contains(USER_NAME_RESERVED) {
        return Err(format!("`{USER_NAME_RESERVED}` は使えません（送り手の表示に使う記号です）"));
    }
    if name.chars().any(char::is_control) {
        return Err("改行や制御文字は使えません（呼び名は 1 行で表示されます）".to_owned());
    }
    // **バイト長ではなくコードポイント数**で数える（上の doc）。
    let chars = name.chars().count();
    if chars > USER_NAME_MAX_CHARS {
        return Err(format!(
            "{USER_NAME_MAX_CHARS} 字以内にしてください（現在 {chars} 字）"
        ));
    }
    Ok(name.to_owned())
}

/// 外部クライアントの名乗りが受け入れ条件を満たさないときの既定ラベル
/// （`mcp_server_contract` 凍結 6）。
pub const DEFAULT_EXTERNAL_CLIENT: &str = "external";

/// 外部 MCP クライアントの名乗りを、封筒へ入れられる形に正規化する（Spec 25）。
///
/// **検査は [`normalize_user_name`] と同じ述語**（`】`・制御文字・字数）。
/// 共有するのは述語であって処方ではない — **落ちたときは拒否ではなく既定
/// ラベルへ落とす**。`clientInfo.name` は呼び出し側が対話的に直せない値なので、
/// 拒否すると扉ごと使えなくなる（`mcp_server_contract` 凍結 6）。
///
/// 落としたことは WARN 1 行で残す。**理由だけを出し、値は出さない** —
/// [`normalize_user_name`] の理由は入力値を含まない契約なので、そのまま
/// ログへ流してよい（拒否の過程で壊れた値を再放流しない）。
pub fn normalize_client_name(raw: &str) -> String {
    match normalize_user_name(raw) {
        Ok(name) => name,
        Err(reason) => {
            crate::note!(
                "mcp client: 名乗りを `{DEFAULT_EXTERNAL_CLIENT}` として扱います（{reason}）"
            );
            DEFAULT_EXTERNAL_CLIENT.to_owned()
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
    /// 利用者の呼び名（Spec 19。`None` = 未設定 = 封筒は既定の「ユーザー」）。
    ///
    /// **村の共有物なので `world.json` に住む** — 封筒 `【送り手: {名前}】` は
    /// `AgentRecord.history` と `session_store` の両方へ**文字列そのもの**として
    /// 入るので、`language` と同じ理由で `localStorage` には置けない。
    ///
    /// **生の文字列で受ける**（`language` と同じ判断）。手編集で書式が壊れていても
    /// `world.json` ごと開けなくなるのは罰が重すぎる。検証と正規化は
    /// [`World::from_persisted`] が [`normalize_user_name`] を通して担う。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_name: Option<String>,
    /// 役職（Spec 14）。エージェントの雛形と、場に見えるラベル。
    ///
    /// **村の共有物**なので `world.json` に住む — 村を配ると役職も付いて回る。
    /// `#[serde(default)]` なので、役職を 1 つも持たない既存の村もそのまま開く。
    #[serde(default)]
    pub roles: Vec<AgentRole>,
    /// 外部からの依頼を受ける窓口（Spec 25。`None` = 未設定）。
    ///
    /// **村の内容物なので `world.json` に住む** — どの個体が窓口かは村ごとに
    /// 違う（進行役はトポロジーの帰結であって固定の役ではない）。サーバーの
    /// ON/OFF・ポート・トークンは**アプリの設定**なので
    /// `{app_data_dir}/mcp_server.json` 側にあり、置き場が割れるのは正しい
    /// （#52 の境界「村の内容物か、アプリの設定か」）。**この分離の帰結として
    /// 村を配っても扉は開かない**（配るのは workspace だけ）。
    ///
    /// **削除済みのエージェントを指していても落とさない。** 読み出し側が
    /// 「窓口が見つからない」と報告するほうが、黙って「未設定」へ化けるより
    /// 診断になる（役職 `role_id` と同じ読み時解決）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reception: Option<AgentId>,
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
    /// 利用者の呼び名（Spec 19）。意味論は [`PersistedWorld::user_name`]。
    /// **ここに入るのは検証を通った値だけ**（読み込みでも保存でも同じ述語を通す）。
    user_name: Option<String>,
    /// 役職（Spec 14）。意味論は [`PersistedWorld::roles`]。
    roles: BTreeMap<AgentRoleId, AgentRole>,
    /// 外部からの依頼を受ける窓口（Spec 25）。意味論は [`PersistedWorld::reception`]。
    reception: Option<AgentId>,
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
        // 手編集で書式が壊れた呼び名は落とす（未設定へ倒す）。**入口を塞ぐだけでは
        // 足りない** — 塞ぐ前に書かれた値はそのまま封筒へ流れる。api_key_env の
        // 事故の処方 (2)「読み込み時に不正値を落とす」の写し。
        //
        // **ファイルへは書き戻さない。** 呼び名は秘密ではないので、ディスクに残る
        // 害は「起動のたびに WARN が出る」だけで、むしろ手で直す手掛かりになる
        // （api_key_env が書き戻しを要したのは、残るのが秘密だったから）。
        // 次に何かを保存した時点で to_persisted が None を書いて消える。
        world.user_name = match persisted.user_name.as_deref() {
            Some(raw) => match normalize_user_name(raw) {
                Ok(name) => Some(name),
                Err(reason) => {
                    // **理由だけを出し、値は出さない**（拒否の過程で再放流しない）。
                    crate::note!("user name: 保存された呼び名を未設定として扱います（{reason}）");
                    None
                }
            },
            None => None,
        };
        // **窓口は検査せずそのまま読む**（役職と同じ判断）。指す先が消えていても
        // 落とさない — 読み出し側が「窓口が見つからない」と報告するほうが、
        // 「未設定」へ黙って化けるより診断になる。
        world.reception = persisted.reception.clone();
        world.topology_positions = persisted.topology_positions.clone();
        for template in persisted.model_templates {
            world.templates.insert(template.id.clone(), template);
        }
        // 役職は**検査せずそのまま読む**。壊れた役職があっても world.json が
        // 開けなくなるほうが害が大きいのはテンプレートと同じ。加えて、
        // 役職を指す role_id が引けなくても**サーヴァントの動作は変わらない**
        // （設定の中身は作成時にコピー済み）ので、孤児の掃除も要らない。
        // 表示側が「引けなければ表示ごと省く」で受ける（role_contract 凍結 5）。
        for role in persisted.roles {
            world.roles.insert(role.id.clone(), role);
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
            user_name: self.user_name.clone(),
            roles: self.roles.values().cloned().collect(),
            reception: self.reception.clone(),
        }
    }

    /// 外部からの依頼を受ける窓口（Spec 25）。`None` = 未設定。
    ///
    /// **存在検査はしない** — 指す先が消えていても値をそのまま返し、
    /// 「見つからない」の報告は呼び出し側が担う（読み時解決）。
    pub fn reception(&self) -> Option<&AgentId> {
        self.reception.as_ref()
    }

    /// 窓口を差し替える。`None` で未設定へ戻す。
    ///
    /// # Errors
    /// 指定したエージェントが未登録の場合 [`CoreError::AgentNotFound`]。
    /// **書き込みの入口でだけ確かめる** — 予定の登録（`create_schedule`）と
    /// 同じ形で、「呼ばれるまで誰も気づかない設定」を作らせない。
    /// **拒否したときは 1 バイトも変更しない。**
    pub fn set_reception(&mut self, agent_id: Option<&AgentId>) -> CoreResult<()> {
        self.reception = match agent_id {
            Some(id) => {
                self.agent(id)?;
                Some(id.clone())
            }
            None => None,
        };
        Ok(())
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

    /// 利用者の呼び名。`None` = 未設定（封筒は既定の「ユーザー」になる）。
    pub fn user_name(&self) -> Option<&str> {
        self.user_name.as_deref()
    }

    /// 利用者の呼び名を差し替える。`None` で既定へ戻す。
    ///
    /// # Errors
    /// 書式が受け入れ条件を満たさない場合 [`CoreError::InvalidUserName`]。
    /// **拒否したときは 1 バイトも変更しない**（「保存したのに別の値になる」を作らない）。
    pub fn set_user_name(&mut self, name: Option<&str>) -> CoreResult<()> {
        self.user_name = match name {
            Some(raw) => Some(
                normalize_user_name(raw)
                    .map_err(|reason| CoreError::InvalidUserName { reason })?,
            ),
            None => None,
        };
        Ok(())
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
            role_id: record.spec.role_id.clone(),
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

    // ---- 役職（Spec 14） ----------------------------------------------------

    /// 役職を登録または更新する。
    ///
    /// **既存のサーヴァントには何も起きない。** `AgentRoleDefaults` は新規作成の
    /// ときにコピーされる（`role_contract` 凍結 4 — 流し込みの発火点は新規作成
    /// ただ 1 つ）ので、ここで中身を書き換えても既に居る個体の設定は変わらない。
    /// 変わるのは `name` を参照している**表示だけ**。
    pub fn upsert_role(&mut self, role: AgentRole) {
        self.roles.insert(role.id.clone(), role);
    }

    /// 役職を削除する。
    ///
    /// **参照中でも拒まない。** [`World::remove_template`] とはここが決定的に
    /// 違う — テンプレートを参照したまま消すとそのエージェントは起動した瞬間に
    /// 必ず失敗するが、**役職はコピー済みなので消してもサーヴァントの動作は
    /// 1 ミリも変わらない**（`role_id` が引けなくなり、バッジと顔ぶれの
    /// `[...]` が消えるだけ）。これがコピー方式の効き所で、参照方式なら
    /// 削除が全個体の人格を消すことになる。
    ///
    /// # Errors
    /// 未登録の id なら [`CoreError::RoleNotFound`]。
    pub fn remove_role(&mut self, id: &AgentRoleId) -> CoreResult<()> {
        if self.roles.remove(id).is_none() {
            return Err(CoreError::RoleNotFound(id.to_string()));
        }
        Ok(())
    }

    /// 役職を借用する。
    ///
    /// # Errors
    /// 未登録の id なら [`CoreError::RoleNotFound`]。**表示側はこの失敗を
    /// エラーとして出さず、役職の表示ごと省く**（`role_contract` 凍結 5）。
    pub fn role(&self, id: &AgentRoleId) -> CoreResult<&AgentRole> {
        self.roles
            .get(id)
            .ok_or_else(|| CoreError::RoleNotFound(id.to_string()))
    }

    /// 役職の表示名を引く。**引けなければ `None`**（`[不明]` とは書かない）。
    ///
    /// バッジ・地図・顔ぶれの 3 箇所はこの 1 実装を通す（`role_contract`
    /// 凍結 5）。3 箇所で規則が分かれると「画面のバッジは消えたのにプロンプトには
    /// `[不明]` が残る」が生まれる。
    pub fn role_label(&self, id: Option<&AgentRoleId>) -> Option<&str> {
        self.roles.get(id?).map(|role| role.name.as_str())
    }

    /// 全役職。
    pub fn roles(&self) -> Vec<AgentRole> {
        self.roles.values().cloned().collect()
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
            user_name: None,
            roles: Vec::new(),
            reception: None,
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

    /// 呼び名の受け入れ条件 4 つ（`user_identity_contract` 凍結 4）。
    #[test]
    fn a_user_name_is_rejected_when_empty_reserved_control_or_too_long() {
        assert!(normalize_user_name("").is_err(), "空");
        assert!(normalize_user_name("   ").is_err(), "空白のみ");
        assert!(normalize_user_name("た】か").is_err(), "封筒の閉じ括弧");
        assert!(normalize_user_name("た\nか").is_err(), "改行");
        assert!(normalize_user_name("た\tか").is_err(), "タブ");

        assert_eq!(normalize_user_name("  たかはし  ").unwrap(), "たかはし");
        // 封筒の開き括弧は通す — 閉じないので封筒の境界は壊れない。
        assert_eq!(normalize_user_name("【ぬし").unwrap(), "【ぬし");
    }

    /// **字数はコードポイントで数える。**
    ///
    /// `str::len` は UTF-8 のバイト長で、日本語は 1 字 3 バイト — 上限の 1/3 で
    /// 発火する。この村の査読で 2 回続けて出た誤りで、**ASCII だけのテストでは
    /// 通ってしまう**ので、境界は必ず日本語で踏む。
    #[test]
    fn a_user_name_is_measured_in_chars_not_bytes() {
        let at_limit = "あ".repeat(USER_NAME_MAX_CHARS);
        assert_eq!(at_limit.chars().count(), USER_NAME_MAX_CHARS);
        assert!(
            at_limit.len() > USER_NAME_MAX_CHARS,
            "バイト長は上限を超えている（超えていないとこのテストが誤りを検出できない）"
        );
        assert!(normalize_user_name(&at_limit).is_ok(), "ちょうど上限は通る");

        let over = "あ".repeat(USER_NAME_MAX_CHARS + 1);
        assert!(normalize_user_name(&over).is_err(), "1 字超過は拒否");
    }

    /// 拒否の理由に**入力値そのものを載せない**（拒否の過程で再放流しない）。
    #[test]
    fn a_rejection_reason_never_echoes_the_input() {
        let secret = "ひみつ".repeat(20);
        let reason = normalize_user_name(&secret).unwrap_err();
        assert!(!reason.contains("ひみつ"), "理由に入力値が出ている: {reason}");
    }

    /// 手編集で壊れた呼び名は読み込みで未設定へ倒す。
    ///
    /// **入口（`set_user_name`）を塞ぐだけでは足りない** — 塞ぐ前に書かれた値は
    /// そのまま封筒へ流れる。api_key_env の処方 (2) の写し。
    #[test]
    fn a_malformed_stored_user_name_falls_back_to_unset() {
        let broken = PersistedWorld {
            user_name: Some("こわれ】た".into()),
            ..Default::default()
        };
        assert_eq!(World::from_persisted(broken).user_name(), None);

        let padded = PersistedWorld {
            user_name: Some("  たかはし  ".into()),
            ..Default::default()
        };
        assert_eq!(
            World::from_persisted(padded).user_name(),
            Some("たかはし"),
            "読み込みでも trim される（保存経路と同じ述語を通る）"
        );
    }

    /// 拒否したときは 1 バイトも変えない（「保存したのに別の値になる」を作らない）。
    #[test]
    fn a_rejected_user_name_leaves_the_previous_value_intact() {
        let mut world = World::new();
        world.set_user_name(Some("たかはし")).unwrap();
        assert_eq!(world.user_name(), Some("たかはし"));

        let err = world.set_user_name(Some("だめ】")).unwrap_err();
        assert_eq!(err.code(), "INVALID_USER_NAME");
        assert_eq!(world.user_name(), Some("たかはし"), "拒否で巻き戻らない");

        world.set_user_name(None).unwrap();
        assert_eq!(world.user_name(), None, "None は既定へ戻す（検証しない）");
        assert_eq!(
            world.to_persisted().user_name,
            None,
            "未設定はファイルへ書かない（skip_serializing_if）"
        );
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
            user_name: None,
            roles: Vec::new(),
            reception: None,
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

    // ---- 役職（Spec 14 P1） -------------------------------------------------

    fn with_template() -> World {
        let mut world = World::new();
        world.upsert_template(ModelTemplate::new("tpl", "既定", "gpt-4o"));
        world
    }

    fn role(id: &str, name: &str) -> AgentRole {
        AgentRole {
            id: id.into(),
            name: name.into(),
            description: String::new(),
            color: None,
            defaults: crate::model::AgentRoleDefaults::default(),
        }
    }

    /// **参照中の役職も削除できる。** `remove_template` との決定的な差。
    ///
    /// テンプレートを参照したまま消すとそのエージェントは起動した瞬間に必ず
    /// 失敗するが、役職は作成時にコピー済みなので**消しても動作は変わらない**
    /// （`role_contract` 凍結 5）。参照方式なら削除が全個体の人格を消す。
    #[test]
    fn a_role_in_use_can_still_be_removed() {
        let mut world = with_template();
        world.upsert_role(role("researcher", "調査役"));
        let mut spec = AgentSpec::new("agent_1", "ザリ", "tpl");
        spec.role_id = Some("researcher".into());
        world.register_agent(spec).unwrap();

        world.remove_role(&"researcher".into()).unwrap();

        // 個体は生きていて、設定も無傷。消えたのは表示だけ。
        let record = world.agent(&"agent_1".into()).unwrap();
        assert_eq!(record.spec.model_template_id, "tpl".into());
        assert_eq!(record.spec.role_id, Some("researcher".into()));
        assert_eq!(world.role_label(record.spec.role_id.as_ref()), None);
    }

    /// 引けない `role_id` は **`None`**。`[不明]` のような代替表示を作らない
    /// （`role_contract` 凍結 5 — 存在しない役は判断材料にならず、顔ぶれでは
    /// 毎ターンぶんのトークンを払うだけになる）。
    #[test]
    fn role_label_returns_none_for_unknown_or_absent_ids() {
        let mut world = World::new();
        world.upsert_role(role("researcher", "調査役"));

        assert_eq!(world.role_label(Some(&"researcher".into())), Some("調査役"));
        assert_eq!(world.role_label(Some(&"missing".into())), None);
        assert_eq!(world.role_label(None), None);
    }

    /// 役職を**改名すると表示が追従する**（名前は参照）。
    #[test]
    fn renaming_a_role_moves_every_label() {
        let mut world = World::new();
        world.upsert_role(role("researcher", "調査役"));
        world.upsert_role(role("researcher", "コード調査役"));

        assert_eq!(
            world.role_label(Some(&"researcher".into())),
            Some("コード調査役")
        );
        assert_eq!(world.roles().len(), 1, "改名で増えない（id が同じ）");
    }

    /// 未登録の削除は `RoleNotFound`。
    #[test]
    fn removing_an_unknown_role_is_an_error() {
        let mut world = World::new();
        let err = world.remove_role(&"missing".into()).unwrap_err();
        assert_eq!(err.code(), "ROLE_NOT_FOUND");
    }

    /// 役職は `world.json` を往復する（村の共有物）。
    #[test]
    fn roles_round_trip_through_persistence() {
        let mut world = World::new();
        world.upsert_role(role("researcher", "調査役"));

        let restored = World::from_persisted(world.to_persisted());

        assert_eq!(restored.roles().len(), 1);
        assert_eq!(
            restored.role_label(Some(&"researcher".into())),
            Some("調査役")
        );
    }

    /// 役職を 1 つも持たない既存の村もそのまま開く（`#[serde(default)]`）。
    #[test]
    fn a_world_without_roles_still_opens() {
        let json = r#"{"agents":[],"modelTemplates":[],"topologyPositions":{}}"#;
        let persisted: PersistedWorld = serde_json::from_str(json).unwrap();
        assert!(persisted.roles.is_empty());
        assert!(World::from_persisted(persisted).roles().is_empty());
    }

    /// **孤児の `role_id` は掃除しない。** 復元時に引けなくても、設定の中身は
    /// コピー済みなので動作に影響が無い。表示側が「引けなければ省く」で受ける。
    #[test]
    fn an_orphan_role_id_survives_restore_without_cleanup() {
        let mut world = with_template();
        let mut spec = AgentSpec::new("agent_1", "ザリ", "tpl");
        spec.role_id = Some("消えた役職".into());
        world.register_agent(spec).unwrap();

        let restored = World::from_persisted(world.to_persisted());
        let record = restored.agent(&"agent_1".into()).unwrap();

        assert_eq!(record.spec.role_id, Some("消えた役職".into()));
        assert_eq!(restored.role_label(record.spec.role_id.as_ref()), None);
    }
}
