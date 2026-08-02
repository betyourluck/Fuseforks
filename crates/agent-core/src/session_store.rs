//! 会話の永続化とセッション管理の**純機構**（Spec 12 P1）。
//!
//! ここは `{workspace}/sessions.redb` を読み書きするだけの同期モジュールで、
//! オーケストレーターも Tokio も知らない。配線（`Shared::record` /
//! `push_exchange` の書き込み点・bootstrap での復元）は P2 の担当。
//!
//! ## なぜ 2 層あるのか（この村に固有の事情）
//!
//! | | 何を持つか | 形 |
//! |---|---|---|
//! | `Shared.log` | 村の会話ログ | [`AgentMessage`] |
//! | `AgentRecord.history` | エージェント個別のプロンプト履歴 | [`ChatMessage`] |
//!
//! **片方から他方を復元できない。** `history` には #45 の規律で「送った文字列
//! そのもの」（畳んだ可変文脈込み）が入り、その文字列は `log` のどこにも無い。
//! `log` には System 行や他人宛の発話があり、どの `history` にも入らない。
//! ゆえに会話ログだけ保存して再開すると、画面は正しいのに全エージェントが
//! 健忘症で始まり、**その 2 つは画面上区別が付かない**。
//! [`Record`] が `message` と `exchange` を別種別で持つのはこのため。
//!
//! ## 保存先を redb 1 ファイルにした理由
//!
//! rev1 の `JSONL + index.json` は**索引ファイルを置いた時点で真実が 2 つ**になり、
//! 原子性の破れと fork の孤児がその帰結だった。redb は機構を足すのではなく減らす —
//! `index.json`・temp+rename の自前実装・壊れた行の切り捨て・fork の 2 段階が
//! 全部消える（Spec 12 rev2 の R0）。
//!
//! ## 読める出口を機構の一部にする
//!
//! redb はバイナリなので、[`SessionStore::export_session`] で JSONL へ書き出せることを
//! **P1 の完了条件**にしてある。この企画の診断は grep に依存しており
//! （failures.md #47 は `concordia.log` の grep で解けた）、読めない保存先を作るなら
//! 出口は機構の一部でなければならない。

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult};
use crate::llm::ChatMessage;
use crate::model::{AgentId, AgentMessage, Endpoint, now_ms};
use crate::note;
use crate::world::exchange_pair;

/// セッションのメタデータ表。key = `session_id`、value = [`SessionMeta`] の JSON。
const SESSIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("sessions");

/// レコード表。key = `(session_id, seq)`、value = [`Record`] の JSON。
///
/// **タプルキーの範囲走査**が「後ろから N 件」を O(log n + k) で返すことが、
/// 滑る窓の復元を索引なしで成立させている（実測 0.03 ms / 20,000 件）。
const RECORDS: TableDefinition<(&str, u64), &[u8]> = TableDefinition::new("records");

/// この数を超えたセッションがあると、開いたときに WARN を 1 行出す。**削除はしない。**
pub const SESSION_COUNT_WARN: usize = 200;

/// 1 セッションのレコード数がこれを超えたら WARN を出す（要約を促す）。
pub const RECORD_COUNT_WARN: u64 = 50_000;

/// 自動生成するタイトルの長さ（文字数。バイトではない）。
pub const TITLE_MAX_CHARS: usize = 30;

/// セッション 1 つの表題と系譜。
///
/// `parentId` / `forkedAtSeq` は [`SessionStore::fork_session`] で生まれたときだけ入る。
/// **「最新」の判定は `updatedAt` で行う** — `session_id` の辞書順に依存させない
/// （ID の時刻順は人が一覧を読むときの都合に留める）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    /// 表題。最初のユーザー発話の先頭 [`TITLE_MAX_CHARS`] 字から自動生成する。
    pub title: String,
    /// 作成時刻（UNIX エポックからのミリ秒）。
    pub created_at: u64,
    /// 最後にレコードを積んだ時刻。**一覧の並びと「最新」の判定はこれで行う。**
    pub updated_at: u64,
    /// 分岐元のセッション ID。分岐で生まれた場合だけ入る。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// 分岐した地点の `seq`（**この seq を含む**）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_at_seq: Option<u64>,
    /// 保持しているレコード数。
    pub record_count: u64,
}

/// 一覧の 1 行。ID とメタデータの対。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    /// セッション ID。
    pub id: String,
    /// メタデータ。
    pub meta: SessionMeta,
}

/// 保存されるレコード。**3 種別で閉じる。**
///
/// 2 種別にできないのは、上のモジュール解説にある「履歴が 2 層ある」事情による。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Record {
    /// 村の会話ログ 1 件。**[`AgentMessage`] 丸ごと**を持つ。
    ///
    /// `grounding` / `co_recipients` まで保存するのは、どちらも**再開後の表示に
    /// 要る**ため。出典の無表示は「出典が存在しない」という判定に見え、同報の
    /// 欠落は会話の読みを変える。
    Message(Box<AgentMessage>),

    /// エージェント個別の履歴 1 往復。
    #[serde(rename_all = "camelCase")]
    Exchange {
        /// 対象エージェント。
        agent_id: String,
        /// **実際に送った文字列そのもの**（#45 の規律を保存側へ延長）。
        ///
        /// 可変文脈（入退室通知・広場ログ・顔ぶれ）を畳んだ後の文字列であり、
        /// 会話ログの本文とは一致しない。ここで畳む前の文字列を保存すると、
        /// 再開後のプロンプトが保存前と食い違い、キャッシュの前方一致も切れる。
        sent: String,
        /// そのとき返った本文。
        replied: String,
    },

    /// 要約 1 本（Spec 12 P4 で書き込む。P1 は読み書きの機構だけ持つ）。
    #[serde(rename_all = "camelCase")]
    Summary {
        /// 対象エージェント。
        agent_id: String,
        /// 要約の本文。
        text: String,
        /// **この seq まで**を覆う要約であることを示す境界。
        ///
        /// 件数ではなくレコードの seq 境界。`coversUpToSeq < 自身の seq` が
        /// 不変条件で、これが単調増加するので要約の要約が循環しない。
        covers_up_to_seq: u64,
    },
}

impl Record {
    /// 会話ログ 1 件のレコードを作る。
    pub fn message(message: AgentMessage) -> Self {
        Self::Message(Box::new(message))
    }

    /// 履歴 1 往復のレコードを作る。
    pub fn exchange(agent_id: &AgentId, sent: impl Into<String>, replied: impl Into<String>) -> Self {
        Self::Exchange {
            agent_id: agent_id.as_str().to_owned(),
            sent: sent.into(),
            replied: replied.into(),
        }
    }

    /// 要約 1 本のレコードを作る。
    pub fn summary(agent_id: &AgentId, text: impl Into<String>, covers_up_to_seq: u64) -> Self {
        Self::Summary {
            agent_id: agent_id.as_str().to_owned(),
            text: text.into(),
            covers_up_to_seq,
        }
    }
}

/// 復元したエージェント別の文脈。
///
/// **要約は履歴の中に席を持たない。** 注入は既存の可変文脈の畳みに相乗りさせる
/// （次のターンの `context` へ差し、その結果できた `sent_user_turn` がそのまま
/// `exchange` として保存される）。ここで `histories` へ混ぜてしまうと、
/// 送信と保存が食い違って #45 の規律が破れるので、**型の上で分けてある**。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RestoredHistories {
    /// エージェント別のプロンプト履歴。滑る窓を適用済み。空のものは含めない。
    pub histories: BTreeMap<AgentId, Vec<ChatMessage>>,
    /// エージェント別の**最新の**要約本文。古い要約は痕跡として残るが現役ではない。
    pub summaries: BTreeMap<AgentId, String>,
}

/// 分岐できる地点（Spec 12 P3 — fork の UI が枝を切る位置を選ぶための投影）。
///
/// 候補を**ユーザー発話に限る**のは、それが人にとって会話の節目だから。
/// 全レコードを並べると入退室の System 行や往復の記録まで候補に出て、
/// 「どこで切ったのか」が読めなくなる。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkPoint {
    /// この発話のレコード seq。`fork_session(id, seq)` へそのまま渡せる
    /// （**この seq を含めて**複製される）。
    pub seq: u64,
    /// 発話の先頭。一覧に出す短い手がかり。
    pub preview: String,
    /// 発話時刻（UNIX エポックからのミリ秒）。
    pub ts_ms: u64,
}

/// [`ForkPoint::preview`] の長さ（文字数）。
pub const FORK_PREVIEW_CHARS: usize = 60;

/// `{workspace}/sessions.redb` への読み書き。
///
/// **書き込みトランザクションを `await` を跨いで持たない。** redb は同期 API で
/// 書き込みは 1 本に直列化されるため、トランザクションを持ったまま LLM 呼び出しや
/// ファイル I/O を待つと村全体が固まる。このモジュールの各メソッドは
/// **1 呼び出しの中でトランザクションを開いて閉じる**ことでこれを構造的に守る。
#[derive(Debug)]
pub struct SessionStore {
    /// 開いているデータベース。
    db: Database,
    /// 診断メッセージに載せるパス。
    path: PathBuf,
}

impl SessionStore {
    /// 保存先を開く（無ければ作る）。
    ///
    /// 起動時に 1 回だけ呼ぶ。テーブルが無い状態で読み取りトランザクションを
    /// 開くと `TableDoesNotExist` になるため、ここで両方のテーブルを作ってから返す。
    ///
    /// セッション数が [`SESSION_COUNT_WARN`] を超えていれば WARN を 1 行出す。
    /// **削除はしない** — 消していいかを機械が決める場面ではない。
    ///
    /// # Errors
    /// 親フォルダを作れない、ファイルを開けない、初期化に失敗した場合。
    pub fn open(path: impl Into<PathBuf>) -> CoreResult<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| CoreError::SessionStore {
                path: parent.display().to_string(),
                operation: "作成",
                reason: err.to_string(),
            })?;
            restrict_to_owner(parent, 0o700);
        }

        let db = Database::create(&path).map_err(|err| CoreError::SessionStore {
            path: path.display().to_string(),
            operation: "開く",
            reason: err.to_string(),
        })?;
        restrict_to_owner(&path, 0o600);

        let store = Self { db, path };

        // 空のテーブルを実体化しておく。読み取り側に「まだ無い」の場合分けを持たせない。
        let txn = store.db.begin_write().map_err(|e| store.err("開始", e))?;
        {
            txn.open_table(SESSIONS).map_err(|e| store.err("初期化", e))?;
            txn.open_table(RECORDS).map_err(|e| store.err("初期化", e))?;
        }
        txn.commit().map_err(|e| store.err("確定", e))?;

        let count = store.list_sessions()?.len();
        if count > SESSION_COUNT_WARN {
            note!(
                "WARN session store: 保存されている会話が {count} 件あります（目安 {SESSION_COUNT_WARN} 件）— \
                 会話ペインの一覧から不要な会話を削除すると、起動時の読み込みが軽くなります"
            );
        }

        Ok(store)
    }

    /// 開いているファイルのパス。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 新しいセッションを開く。ID は `{epoch_ms}-{短い乱数}` で時刻順に並ぶ。
    ///
    /// `title` を省略すると空のまま始まり、**最初のユーザー発話**が積まれた時点で
    /// その先頭 [`TITLE_MAX_CHARS`] 字から自動生成される。
    ///
    /// # Errors
    /// 書き込みに失敗した場合。
    pub fn create_session(&self, title: Option<&str>) -> CoreResult<String> {
        let now = now_ms();
        let id = format!("{now}-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let meta = SessionMeta {
            title: title.unwrap_or_default().to_owned(),
            created_at: now,
            updated_at: now,
            parent_id: None,
            forked_at_seq: None,
            record_count: 0,
        };
        self.put_meta(&id, &meta)?;
        Ok(id)
    }

    /// レコードを 1 件積み、採番した `seq` を返す。
    ///
    /// **採番・レコード追加・メタデータ更新は 1 トランザクション**で行う。
    /// 分けると「レコードはあるが件数が古い」状態が作れてしまい、
    /// それは真実が 2 つある状態と同じ意味になる。
    ///
    /// レコード数が [`RECORD_COUNT_WARN`] を跨いだ 1 回だけ WARN を出す
    /// （毎回出すと、要約を促す行が要約を邪魔する量になる）。
    ///
    /// # Errors
    /// セッションが存在しない、または書き込みに失敗した場合。
    pub fn append(&self, session_id: &str, record: &Record) -> CoreResult<u64> {
        let payload = serde_json::to_vec(record)?;
        let txn = self.db.begin_write().map_err(|e| self.err("開始", e))?;
        let seq;
        {
            let mut sessions = txn.open_table(SESSIONS).map_err(|e| self.err("書き込み", e))?;
            let mut meta = {
                let guard = sessions
                    .get(session_id)
                    .map_err(|e| self.err("読み込み", e))?
                    .ok_or_else(|| CoreError::SessionNotFound(session_id.to_owned()))?;
                serde_json::from_slice::<SessionMeta>(guard.value())?
            };

            let mut records = txn.open_table(RECORDS).map_err(|e| self.err("書き込み", e))?;
            seq = {
                let mut range = records
                    .range((session_id, 0u64)..=(session_id, u64::MAX))
                    .map_err(|e| self.err("読み込み", e))?;
                match range.next_back() {
                    Some(entry) => {
                        let (key, _) = entry.map_err(|e| self.err("読み込み", e))?;
                        key.value().1.saturating_add(1)
                    }
                    None => 0,
                }
            };
            records
                .insert((session_id, seq), payload.as_slice())
                .map_err(|e| self.err("書き込み", e))?;

            meta.updated_at = now_ms();
            meta.record_count = meta.record_count.saturating_add(1);
            if meta.title.is_empty() {
                if let Some(title) = auto_title(record) {
                    meta.title = title;
                }
            }
            if meta.record_count == RECORD_COUNT_WARN.saturating_add(1) {
                note!(
                    "WARN session store: 会話 `{session_id}` のレコードが {RECORD_COUNT_WARN} 件を超えました — \
                     会話ペインの「要約して続ける」を押すと、以後のプロンプトが短くなります"
                );
            }
            let encoded = serde_json::to_vec(&meta)?;
            sessions
                .insert(session_id, encoded.as_slice())
                .map_err(|e| self.err("書き込み", e))?;
        }
        txn.commit().map_err(|e| self.err("確定", e))?;
        Ok(seq)
    }

    /// セッションのメタデータを引く。存在しなければ `None`。
    ///
    /// # Errors
    /// 読み込みに失敗した場合。
    pub fn session_meta(&self, session_id: &str) -> CoreResult<Option<SessionMeta>> {
        let txn = self.db.begin_read().map_err(|e| self.err("開始", e))?;
        let sessions = txn.open_table(SESSIONS).map_err(|e| self.err("読み込み", e))?;
        let guard = sessions.get(session_id).map_err(|e| self.err("読み込み", e))?;
        match guard {
            Some(guard) => Ok(Some(serde_json::from_slice(guard.value())?)),
            None => Ok(None),
        }
    }

    /// 全セッションを `updatedAt` の新しい順で返す。
    ///
    /// 並びを ID に依存させないのは、ID の時刻部分が**作成時刻**であって
    /// 最終更新ではないため。古い会話を再開したら一覧の先頭に来るのが正しい。
    ///
    /// # Errors
    /// 読み込みに失敗した場合。
    pub fn list_sessions(&self) -> CoreResult<Vec<SessionSummary>> {
        let txn = self.db.begin_read().map_err(|e| self.err("開始", e))?;
        let sessions = txn.open_table(SESSIONS).map_err(|e| self.err("読み込み", e))?;
        let mut out = Vec::new();
        for entry in sessions.iter().map_err(|e| self.err("読み込み", e))? {
            let (key, value) = entry.map_err(|e| self.err("読み込み", e))?;
            out.push(SessionSummary {
                id: key.value().to_owned(),
                meta: serde_json::from_slice(value.value())?,
            });
        }
        out.sort_by(|a, b| {
            b.meta
                .updated_at
                .cmp(&a.meta.updated_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        Ok(out)
    }

    /// 最後に更新されたセッションの ID。1 件も無ければ `None`。
    ///
    /// 起動時の既定はこれを開く。**読めない・0 件なら呼び出し側が新規を作る** —
    /// 起動が止まる経路は作らない（Spec 12 D1）。
    ///
    /// # Errors
    /// 読み込みに失敗した場合。
    pub fn latest_session(&self) -> CoreResult<Option<String>> {
        Ok(self.list_sessions()?.into_iter().next().map(|s| s.id))
    }

    /// セッションのレコードを seq 昇順で全部返す。
    ///
    /// 索引を作らない根拠がここ — 20,000 件の全走査が 11 ms で、索引が要るのは
    /// その 1 桁上から。
    ///
    /// # Errors
    /// セッションが存在しない、または読み込みに失敗した場合。
    pub fn records(&self, session_id: &str) -> CoreResult<Vec<(u64, Record)>> {
        self.ensure_exists(session_id)?;
        let txn = self.db.begin_read().map_err(|e| self.err("開始", e))?;
        let records = txn.open_table(RECORDS).map_err(|e| self.err("読み込み", e))?;
        let mut out = Vec::new();
        for entry in records
            .range((session_id, 0u64)..=(session_id, u64::MAX))
            .map_err(|e| self.err("読み込み", e))?
        {
            let (key, value) = entry.map_err(|e| self.err("読み込み", e))?;
            out.push((key.value().1, serde_json::from_slice(value.value())?));
        }
        Ok(out)
    }

    /// 会話ログの末尾 `limit` 件を古い順で返す。
    ///
    /// `Shared.log` の 5,000 件は**メモリ上の上限であって保存の上限ではない**。
    /// ファイルには全量が残り、起動時に読むのはここで切った末尾だけになる。
    ///
    /// # Errors
    /// セッションが存在しない、または読み込みに失敗した場合。
    pub fn tail_messages(&self, session_id: &str, limit: usize) -> CoreResult<Vec<AgentMessage>> {
        self.ensure_exists(session_id)?;
        if limit == 0 {
            return Ok(Vec::new());
        }
        let txn = self.db.begin_read().map_err(|e| self.err("開始", e))?;
        let records = txn.open_table(RECORDS).map_err(|e| self.err("読み込み", e))?;
        let mut out = Vec::new();
        let range = records
            .range((session_id, 0u64)..=(session_id, u64::MAX))
            .map_err(|e| self.err("読み込み", e))?;
        for entry in range.rev() {
            let (_, value) = entry.map_err(|e| self.err("読み込み", e))?;
            if let Record::Message(message) = serde_json::from_slice::<Record>(value.value())? {
                out.push(*message);
                if out.len() >= limit {
                    break;
                }
            }
        }
        out.reverse();
        Ok(out)
    }

    /// エージェント別の履歴を組み直す。
    ///
    /// **滑る窓はロード時に再適用する。** レコードには全 `exchange` が残っており、
    /// ここで `history_turns` 往復に切る。保存時に切ると、設定を広げても過去は
    /// 戻らない（切り落とした文字列はもう無い）。
    ///
    /// 要約がある場合は「**最新の summary** ＋ `coversUpToSeq` **より後**の
    /// exchange」で組む。要約の本文は [`RestoredHistories::summaries`] へ分けて返し、
    /// 履歴の中には入れない（上の型解説を見よ）。
    ///
    /// # Errors
    /// セッションが存在しない、または読み込みに失敗した場合。
    pub fn restore_histories(
        &self,
        session_id: &str,
        history_turns: usize,
    ) -> CoreResult<RestoredHistories> {
        let mut latest_summary: BTreeMap<AgentId, (u64, String, u64)> = BTreeMap::new();
        let mut exchanges: BTreeMap<AgentId, Vec<(u64, String, String)>> = BTreeMap::new();

        for (seq, record) in self.records(session_id)? {
            match record {
                Record::Message(_) => {}
                Record::Exchange {
                    agent_id,
                    sent,
                    replied,
                } => exchanges
                    .entry(AgentId::new(agent_id))
                    .or_default()
                    .push((seq, sent, replied)),
                Record::Summary {
                    agent_id,
                    text,
                    covers_up_to_seq,
                } => {
                    let agent = AgentId::new(agent_id);
                    // 同じエージェントに複数あれば seq の大きいほうが現役。
                    let replace = latest_summary
                        .get(&agent)
                        .is_none_or(|(current, _, _)| seq > *current);
                    if replace {
                        latest_summary.insert(agent, (seq, text, covers_up_to_seq));
                    }
                }
            }
        }

        let mut restored = RestoredHistories::default();
        for (agent, mut turns) in exchanges {
            let covered = latest_summary
                .get(&agent)
                .map_or(0, |(_, _, covers)| covers.saturating_add(1));
            turns.retain(|(seq, _, _)| *seq >= covered);
            if turns.len() > history_turns {
                turns.drain(..turns.len() - history_turns);
            }
            if history_turns == 0 || turns.is_empty() {
                continue;
            }
            let mut history = Vec::with_capacity(turns.len() * 2);
            for (_, sent, replied) in turns {
                history.extend(exchange_pair(&sent, &replied));
            }
            restored.histories.insert(agent, history);
        }
        for (agent, (_, text, _)) in latest_summary {
            restored.summaries.insert(agent, text);
        }
        Ok(restored)
    }

    /// 分岐できる地点を古い順で返す（Spec 12 P3）。
    ///
    /// 候補は**ユーザー発話だけ**。返る `seq` はその発話自身のもので、
    /// [`Self::fork_session`] へ渡すと**その発話を含めて**複製される
    /// （= 「この依頼までは同じで、そこから別の頼み方を試す」）。
    ///
    /// # Errors
    /// セッションが存在しない、または読み込みに失敗した場合。
    pub fn fork_points(&self, session_id: &str) -> CoreResult<Vec<ForkPoint>> {
        let mut out = Vec::new();
        for (seq, record) in self.records(session_id)? {
            let Record::Message(message) = record else {
                continue;
            };
            if !matches!(message.from, Endpoint::User) {
                continue;
            }
            let flattened = message
                .content
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            out.push(ForkPoint {
                seq,
                preview: flattened.chars().take(FORK_PREVIEW_CHARS).collect(),
                ts_ms: message.ts_ms,
            });
        }
        Ok(out)
    }

    /// `at_seq` **までを含めて**複製した新しいセッションを作る。
    ///
    /// **seq は振り直さない。** 振り直すと `summary.coversUpToSeq` が指す境界が
    /// ずれ、複製した側だけ要約の覆う範囲が変わる。元のセッションは不変のまま残る。
    ///
    /// # Errors
    /// 分岐元が存在しない、または書き込みに失敗した場合。
    pub fn fork_session(&self, source_id: &str, at_seq: u64) -> CoreResult<String> {
        let now = now_ms();
        let new_id = format!("{now}-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);

        let txn = self.db.begin_write().map_err(|e| self.err("開始", e))?;
        {
            let mut sessions = txn.open_table(SESSIONS).map_err(|e| self.err("書き込み", e))?;
            let source = {
                let guard = sessions
                    .get(source_id)
                    .map_err(|e| self.err("読み込み", e))?
                    .ok_or_else(|| CoreError::SessionNotFound(source_id.to_owned()))?;
                serde_json::from_slice::<SessionMeta>(guard.value())?
            };

            let mut records = txn.open_table(RECORDS).map_err(|e| self.err("書き込み", e))?;
            // 走査と書き込みを同じテーブルで交互にはできないので、いったん持ち上げる。
            // 実測で 10,000 件の複製が 55 ms（1 トランザクション）。
            let copied: Vec<(u64, Vec<u8>)> = {
                let range = records
                    .range((source_id, 0u64)..=(source_id, at_seq))
                    .map_err(|e| self.err("読み込み", e))?;
                let mut buf = Vec::new();
                for entry in range {
                    let (key, value) = entry.map_err(|e| self.err("読み込み", e))?;
                    buf.push((key.value().1, value.value().to_vec()));
                }
                buf
            };
            let record_count = copied.len() as u64;
            for (seq, payload) in copied {
                records
                    .insert((new_id.as_str(), seq), payload.as_slice())
                    .map_err(|e| self.err("書き込み", e))?;
            }

            let meta = SessionMeta {
                title: source.title,
                created_at: now,
                updated_at: now,
                parent_id: Some(source_id.to_owned()),
                forked_at_seq: Some(at_seq),
                record_count,
            };
            let encoded = serde_json::to_vec(&meta)?;
            sessions
                .insert(new_id.as_str(), encoded.as_slice())
                .map_err(|e| self.err("書き込み", e))?;
        }
        txn.commit().map_err(|e| self.err("確定", e))?;
        Ok(new_id)
    }

    /// セッションとそのレコードを消す。存在しなければ何もしない。
    ///
    /// メタデータとレコードを**同じトランザクション**で消す。分けると
    /// 「メタは消えたがレコードが残る」孤児が作れる（rev1 の fork がその形だった）。
    ///
    /// # Errors
    /// 書き込みに失敗した場合。
    pub fn delete_session(&self, session_id: &str) -> CoreResult<()> {
        let txn = self.db.begin_write().map_err(|e| self.err("開始", e))?;
        {
            let mut sessions = txn.open_table(SESSIONS).map_err(|e| self.err("書き込み", e))?;
            sessions
                .remove(session_id)
                .map_err(|e| self.err("書き込み", e))?;
            let mut records = txn.open_table(RECORDS).map_err(|e| self.err("書き込み", e))?;
            records
                .retain_in((session_id, 0u64)..=(session_id, u64::MAX), |_, _| false)
                .map_err(|e| self.err("書き込み", e))?;
        }
        txn.commit().map_err(|e| self.err("確定", e))?;
        Ok(())
    }

    /// セッションを JSONL で書き出し、書いたレコード数を返す。
    ///
    /// 1 行目はセッションのメタデータ（`{"session":{...}}`）、2 行目以降は
    /// `{"seq":N,"kind":"...",...}` の 1 レコード 1 行。**`kind` は 3 種別のまま**で、
    /// ヘッダ行は `kind` を持たない — `"kind":"message"` の grep が
    /// ヘッダに引っかからないようにするため。
    ///
    /// これが P1 の完了条件。redb はバイナリなので、この出口が無いと
    /// 診断が grep できなくなる。
    ///
    /// # Errors
    /// セッションが存在しない、読み込みに失敗した、または書き出しに失敗した場合。
    pub fn export_session(&self, session_id: &str, out: &mut dyn Write) -> CoreResult<u64> {
        let meta = self
            .session_meta(session_id)?
            .ok_or_else(|| CoreError::SessionNotFound(session_id.to_owned()))?;

        let header = serde_json::json!({
            "session": { "id": session_id, "meta": meta },
        });
        writeln!(out, "{header}").map_err(|e| self.err("書き出し", e))?;

        let mut written = 0u64;
        for (seq, record) in self.records(session_id)? {
            let line = serde_json::to_string(&ExportLine { seq, record: &record })?;
            writeln!(out, "{line}").map_err(|e| self.err("書き出し", e))?;
            written += 1;
        }
        Ok(written)
    }

    /// [`Self::export_session`] の結果をファイルへ書く。
    ///
    /// # Errors
    /// セッションが存在しない、またはファイルを作れない・書けない場合。
    pub fn export_session_to_file(&self, session_id: &str, path: impl AsRef<Path>) -> CoreResult<u64> {
        let path = path.as_ref();
        let mut file = std::fs::File::create(path).map_err(|err| CoreError::SessionStore {
            path: path.display().to_string(),
            operation: "書き出し",
            reason: err.to_string(),
        })?;
        self.export_session(session_id, &mut file)
    }

    /// メタデータを丸ごと差し替える（新規作成にも使う）。
    fn put_meta(&self, session_id: &str, meta: &SessionMeta) -> CoreResult<()> {
        let encoded = serde_json::to_vec(meta)?;
        let txn = self.db.begin_write().map_err(|e| self.err("開始", e))?;
        {
            let mut sessions = txn.open_table(SESSIONS).map_err(|e| self.err("書き込み", e))?;
            sessions
                .insert(session_id, encoded.as_slice())
                .map_err(|e| self.err("書き込み", e))?;
        }
        txn.commit().map_err(|e| self.err("確定", e))?;
        Ok(())
    }

    /// 存在しないセッションを読もうとしたら、空ではなく失敗として返す。
    fn ensure_exists(&self, session_id: &str) -> CoreResult<()> {
        if self.session_meta(session_id)?.is_none() {
            return Err(CoreError::SessionNotFound(session_id.to_owned()));
        }
        Ok(())
    }

    /// 保存先の失敗を、どの段で落ちたかを添えて包む。
    fn err(&self, operation: &'static str, reason: impl std::fmt::Display) -> CoreError {
        CoreError::SessionStore {
            path: self.path.display().to_string(),
            operation,
            reason: reason.to_string(),
        }
    }
}

/// 書き出し 1 行の形。`seq` の後ろに [`Record`] を平らに展開する。
#[derive(Serialize)]
struct ExportLine<'a> {
    /// レコードの seq。
    seq: u64,
    /// レコード本体。`kind` を含めて同じ階層へ展開される。
    #[serde(flatten)]
    record: &'a Record,
}

/// 最初のユーザー発話から表題を作る。ユーザー発話でなければ `None`。
///
/// 改行は空白へ潰す。一覧は 1 行で並ぶので、改行が入ると表題が崩れる。
fn auto_title(record: &Record) -> Option<String> {
    let Record::Message(message) = record else {
        return None;
    };
    if !matches!(message.from, Endpoint::User) {
        return None;
    }
    let flattened = message.content.split_whitespace().collect::<Vec<_>>().join(" ");
    if flattened.is_empty() {
        return None;
    }
    Some(flattened.chars().take(TITLE_MAX_CHARS).collect())
}

/// unix でだけ所有者専用の権限へ絞る。
///
/// **Windows では何もしない。** `{workspace}` は `%APPDATA%` 配下で既にユーザー私有の
/// ACL 下にあり、POSIX 権限の API も無い。ここで chmod しようとするほうがバグになる。
/// 失敗しても保存は続ける — 権限を絞れないことは、会話を保存しない理由にならない。
#[cfg(unix)]
fn restrict_to_owner(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

/// unix 以外では何もしない。
#[cfg(not(unix))]
fn restrict_to_owner(_path: &Path, _mode: u32) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::Role;

    /// テスト用の一時フォルダ。redb のファイルを開いたまま消せないので、
    /// `SessionStore` を先に落としてから片付ける。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "concordia-session-{}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir_all(&path).expect("一時フォルダを作れること");
            Self(path)
        }

        fn db(&self) -> PathBuf {
            self.0.join("sessions.redb")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn user_message(text: &str) -> AgentMessage {
        AgentMessage::new(
            Endpoint::User,
            Endpoint::Agent {
                id: AgentId::new("agent_01"),
            },
            text,
            0,
        )
    }

    fn agent() -> AgentId {
        AgentId::new("agent_01")
    }

    #[test]
    fn seq_starts_at_zero_and_increases_monotonically() {
        let dir = TempDir::new();
        let store = SessionStore::open(dir.db()).expect("開けること");
        let id = store.create_session(None).expect("作れること");

        let first = store
            .append(&id, &Record::message(user_message("最初")))
            .expect("積めること");
        let second = store
            .append(&id, &Record::exchange(&agent(), "送った", "返った"))
            .expect("積めること");

        assert_eq!(first, 0, "seq は 0 から始まる");
        assert_eq!(second, 1, "種別が違っても同じ採番を共有する");
        assert_eq!(
            store.session_meta(&id).unwrap().unwrap().record_count,
            2,
            "件数はレコードと同じトランザクションで更新される"
        );
    }

    /// 表題は最初の**ユーザー**発話から採る。エージェント同士の発話では変わらない。
    #[test]
    fn title_comes_from_the_first_user_message_only() {
        let dir = TempDir::new();
        let store = SessionStore::open(dir.db()).expect("開けること");
        let id = store.create_session(None).expect("作れること");

        let from_agent = AgentMessage::new(
            Endpoint::Agent { id: agent() },
            Endpoint::User,
            "エージェントの独り言",
            0,
        );
        store.append(&id, &Record::message(from_agent)).unwrap();
        assert_eq!(
            store.session_meta(&id).unwrap().unwrap().title,
            "",
            "ユーザー発話でなければ表題は付かない"
        );

        store
            .append(&id, &Record::message(user_message("あいうえお".repeat(10).as_str())))
            .unwrap();
        let title = store.session_meta(&id).unwrap().unwrap().title;
        assert_eq!(title.chars().count(), TITLE_MAX_CHARS, "文字数で切る（バイトではない）");

        store
            .append(&id, &Record::message(user_message("2 回目の発話")))
            .unwrap();
        assert_eq!(
            store.session_meta(&id).unwrap().unwrap().title,
            title,
            "一度決まった表題は後の発話で上書きしない"
        );
    }

    /// 滑る窓はロード時に適用する。**窓を広げたら過去が戻る**ことがこの設計の要点。
    #[test]
    fn sliding_window_is_reapplied_at_load_time() {
        let dir = TempDir::new();
        let store = SessionStore::open(dir.db()).expect("開けること");
        let id = store.create_session(None).expect("作れること");
        for i in 0..10 {
            store
                .append(
                    &id,
                    &Record::exchange(&agent(), format!("送信 {i}"), format!("応答 {i}")),
                )
                .unwrap();
        }

        let narrow = store.restore_histories(&id, 2).unwrap();
        let history = &narrow.histories[&agent()];
        assert_eq!(history.len(), 4, "2 往復 = 4 メッセージ");
        assert_eq!(history[0].content, "送信 8", "残るのは新しいほう");

        let wide = store.restore_histories(&id, 8).unwrap();
        assert_eq!(
            wide.histories[&agent()].len(),
            16,
            "窓を広げると過去が戻る（保存時に切っていない証拠）"
        );

        let none = store.restore_histories(&id, 0).unwrap();
        assert!(none.histories.is_empty(), "窓 0 は履歴なし（push_exchange と同じ）");
    }

    /// 復元は「最新の summary ＋ coversUpToSeq **より後**の exchange」で組む。
    #[test]
    fn restore_uses_the_latest_summary_and_only_later_exchanges() {
        let dir = TempDir::new();
        let store = SessionStore::open(dir.db()).expect("開けること");
        let id = store.create_session(None).expect("作れること");

        for i in 0..4 {
            store
                .append(&id, &Record::exchange(&agent(), format!("古い {i}"), "はい"))
                .unwrap();
        }
        // seq 4 の要約が seq 0〜3 を覆う。
        let summary_seq = store
            .append(&id, &Record::summary(&agent(), "前半の要約", 3))
            .unwrap();
        assert!(3 < summary_seq, "coversUpToSeq は自身の seq より小さい");
        for i in 0..2 {
            store
                .append(&id, &Record::exchange(&agent(), format!("新しい {i}"), "はい"))
                .unwrap();
        }

        let restored = store.restore_histories(&id, 8).unwrap();
        let history = &restored.histories[&agent()];
        assert_eq!(history.len(), 4, "要約が覆った 4 往復は履歴に入らない");
        assert_eq!(history[0].content, "新しい 0");
        assert_eq!(
            restored.summaries[&agent()], "前半の要約",
            "要約は履歴ではなく別の口から返る"
        );
        assert!(
            history.iter().all(|m| m.content != "前半の要約"),
            "履歴に summary 専用の席を作らない（#45 の規律）"
        );
    }

    /// 要約の要約: 新しいほうだけが現役で、古い要約は痕跡として残る。
    #[test]
    fn only_the_newest_summary_is_used_for_restore() {
        let dir = TempDir::new();
        let store = SessionStore::open(dir.db()).expect("開けること");
        let id = store.create_session(None).expect("作れること");

        store.append(&id, &Record::exchange(&agent(), "1 番目", "はい")).unwrap();
        store.append(&id, &Record::summary(&agent(), "古い要約", 0)).unwrap();
        store.append(&id, &Record::exchange(&agent(), "2 番目", "はい")).unwrap();
        store.append(&id, &Record::summary(&agent(), "新しい要約", 2)).unwrap();
        store.append(&id, &Record::exchange(&agent(), "3 番目", "はい")).unwrap();

        let restored = store.restore_histories(&id, 8).unwrap();
        assert_eq!(restored.summaries[&agent()], "新しい要約");
        assert_eq!(restored.histories[&agent()][0].content, "3 番目");
        assert_eq!(
            store.records(&id).unwrap().len(),
            5,
            "古い要約もレコードとしては消えない"
        );
    }

    /// 空の発言は復元時も目印へ置き換える（failures.md #29 の保存側）。
    #[test]
    fn empty_utterances_become_placeholders_on_restore() {
        let dir = TempDir::new();
        let store = SessionStore::open(dir.db()).expect("開けること");
        let id = store.create_session(None).expect("作れること");
        store.append(&id, &Record::exchange(&agent(), "  ", "")).unwrap();

        let restored = store.restore_histories(&id, 8).unwrap();
        let history = &restored.histories[&agent()];
        assert_eq!(history[0].content, "（発言なし）");
        assert_eq!(history[1].content, "（発言なし）");
        assert_eq!(history[0].role, Role::User);
        assert_eq!(history[1].role, Role::Assistant);
    }

    /// 飛行中に落ちた形（message はあるが exchange が無い）は**正しい記録**。
    #[test]
    fn a_message_without_an_exchange_shows_but_does_not_enter_history() {
        let dir = TempDir::new();
        let store = SessionStore::open(dir.db()).expect("開けること");
        let id = store.create_session(None).expect("作れること");
        store
            .append(&id, &Record::message(user_message("返事が返らなかった依頼")))
            .unwrap();

        assert_eq!(store.tail_messages(&id, 100).unwrap().len(), 1, "画面には出す");
        assert!(
            store.restore_histories(&id, 8).unwrap().histories.is_empty(),
            "履歴には入れない"
        );
    }

    /// 会話ログの末尾だけを読む。exchange / summary は混ざらない。
    #[test]
    fn tail_messages_returns_the_newest_in_chronological_order() {
        let dir = TempDir::new();
        let store = SessionStore::open(dir.db()).expect("開けること");
        let id = store.create_session(None).expect("作れること");
        for i in 0..5 {
            store
                .append(&id, &Record::message(user_message(&format!("発話 {i}"))))
                .unwrap();
            store.append(&id, &Record::exchange(&agent(), "送信", "応答")).unwrap();
        }

        let tail = store.tail_messages(&id, 2).unwrap();
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[0].content, "発話 3", "古い順で返る");
        assert_eq!(tail[1].content, "発話 4");
    }

    /// fork は `at_seq` を**含めて**複製し、元は不変のまま残る。seq も振り直さない。
    #[test]
    fn fork_copies_up_to_and_including_at_seq_and_leaves_the_source_untouched() {
        let dir = TempDir::new();
        let store = SessionStore::open(dir.db()).expect("開けること");
        let id = store.create_session(None).expect("作れること");
        for i in 0..5 {
            store
                .append(&id, &Record::exchange(&agent(), format!("送信 {i}"), "はい"))
                .unwrap();
        }

        let forked = store.fork_session(&id, 2).expect("分岐できること");
        let copied = store.records(&forked).unwrap();
        assert_eq!(copied.len(), 3, "at_seq を含めて 0・1・2 の 3 件");
        assert_eq!(copied[2].0, 2, "seq は振り直さない（要約の覆う境界がずれる）");

        let meta = store.session_meta(&forked).unwrap().unwrap();
        assert_eq!(meta.parent_id.as_deref(), Some(id.as_str()));
        assert_eq!(meta.forked_at_seq, Some(2));

        assert_eq!(store.records(&id).unwrap().len(), 5, "元は不変");
        let next = store
            .append(&forked, &Record::exchange(&agent(), "分岐後", "はい"))
            .unwrap();
        assert_eq!(next, 3, "分岐先の採番は複製した末尾の次から続く");
        assert_eq!(store.records(&id).unwrap().len(), 5, "分岐先へ積んでも元は増えない");
    }

    /// 分岐点の候補はユーザー発話だけ。System 行や往復の記録は出さない。
    #[test]
    fn fork_points_list_user_messages_only() {
        let dir = TempDir::new();
        let store = SessionStore::open(dir.db()).expect("開けること");
        let id = store.create_session(None).unwrap();

        let system = AgentMessage::new(
            Endpoint::System,
            Endpoint::User,
            "agent_01（PlannerAgent）が稼働を開始しました",
            0,
        );
        store.append(&id, &Record::message(system)).unwrap();
        let first = store
            .append(&id, &Record::message(user_message("最初の依頼\nです")))
            .unwrap();
        store.append(&id, &Record::exchange(&agent(), "送信", "応答")).unwrap();
        let second = store
            .append(&id, &Record::message(user_message("次の依頼")))
            .unwrap();

        let points = store.fork_points(&id).unwrap();
        assert_eq!(points.len(), 2, "System 行と exchange は候補にしない");
        assert_eq!(points[0].seq, first);
        assert_eq!(points[0].preview, "最初の依頼 です", "改行は空白へ潰す");
        assert_eq!(points[1].seq, second);

        // 返った seq をそのまま fork へ渡すと、その発話まで含めて複製される。
        let forked = store.fork_session(&id, points[0].seq).unwrap();
        assert_eq!(store.tail_messages(&forked, 10).unwrap().len(), 2);
    }

    /// 分岐先で要約の覆う境界が生きていること（seq を振り直さない理由の実証）。
    #[test]
    fn fork_keeps_summary_coverage_valid() {
        let dir = TempDir::new();
        let store = SessionStore::open(dir.db()).expect("開けること");
        let id = store.create_session(None).expect("作れること");
        store.append(&id, &Record::exchange(&agent(), "古い", "はい")).unwrap();
        store.append(&id, &Record::summary(&agent(), "要約", 0)).unwrap();
        store.append(&id, &Record::exchange(&agent(), "新しい", "はい")).unwrap();

        let forked = store.fork_session(&id, 2).unwrap();
        let restored = store.restore_histories(&forked, 8).unwrap();
        assert_eq!(restored.summaries[&agent()], "要約");
        assert_eq!(restored.histories[&agent()].len(), 2, "覆われた往復は復活しない");
        assert_eq!(restored.histories[&agent()][0].content, "新しい");
    }

    /// 新規セッションは前の会話を消さない（Spec 03 の改訂 = S3 の保存側）。
    #[test]
    fn creating_a_session_does_not_touch_the_previous_one() {
        let dir = TempDir::new();
        let store = SessionStore::open(dir.db()).expect("開けること");
        let first = store.create_session(Some("前の会話")).unwrap();
        store.append(&first, &Record::message(user_message("前の話"))).unwrap();

        let second = store.create_session(None).unwrap();
        store.append(&second, &Record::message(user_message("次の話"))).unwrap();

        assert_eq!(store.records(&first).unwrap().len(), 1, "前の会話は残る");
        assert_eq!(store.list_sessions().unwrap().len(), 2);
        assert_eq!(
            store.latest_session().unwrap().as_deref(),
            Some(second.as_str()),
            "最新は updatedAt で決まる"
        );
    }

    /// 一覧の並びは ID ではなく `updatedAt`。古い会話を再開したら先頭に来る。
    #[test]
    fn list_is_ordered_by_updated_at_not_by_id() {
        let dir = TempDir::new();
        let store = SessionStore::open(dir.db()).expect("開けること");
        let old = store.create_session(Some("古い")).unwrap();
        let new = store.create_session(Some("新しい")).unwrap();

        let mut meta = store.session_meta(&old).unwrap().unwrap();
        meta.updated_at = u64::MAX;
        store.put_meta(&old, &meta).unwrap();

        let listed = store.list_sessions().unwrap();
        assert_eq!(listed[0].id, old, "後から更新された古い会話が先頭へ来る");
        assert_eq!(listed[1].id, new);
    }

    /// 削除はメタデータとレコードを同時に消す（孤児を作らない）。
    #[test]
    fn delete_removes_meta_and_records_together() {
        let dir = TempDir::new();
        let store = SessionStore::open(dir.db()).expect("開けること");
        let doomed = store.create_session(None).unwrap();
        let kept = store.create_session(None).unwrap();
        store.append(&doomed, &Record::message(user_message("消える"))).unwrap();
        store.append(&kept, &Record::message(user_message("残る"))).unwrap();

        store.delete_session(&doomed).unwrap();

        assert!(store.session_meta(&doomed).unwrap().is_none());
        assert!(matches!(
            store.records(&doomed),
            Err(CoreError::SessionNotFound(_))
        ));
        assert_eq!(store.records(&kept).unwrap().len(), 1, "隣は巻き添えにしない");
    }

    /// 書き出した JSONL から同じ会話を組み直せること（P1 の完了条件）。
    #[test]
    fn export_writes_jsonl_that_reconstructs_the_conversation() {
        let dir = TempDir::new();
        let store = SessionStore::open(dir.db()).expect("開けること");
        let id = store.create_session(None).unwrap();
        store.append(&id, &Record::message(user_message("依頼です"))).unwrap();
        store.append(&id, &Record::exchange(&agent(), "送った文字列", "返った")).unwrap();
        store.append(&id, &Record::summary(&agent(), "要約", 1)).unwrap();

        let mut buf = Vec::new();
        let written = store.export_session(&id, &mut buf).unwrap();
        let text = String::from_utf8(buf).expect("UTF-8 で書かれること");
        let mut lines = text.lines();

        assert_eq!(written, 3);
        let header: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert_eq!(header["session"]["id"], id.as_str());
        assert_eq!(header["session"]["meta"]["title"], "依頼です");
        assert!(header.get("kind").is_none(), "ヘッダ行は kind を持たない");

        let rest: Vec<serde_json::Value> =
            lines.map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(rest[0]["kind"], "message");
        assert_eq!(rest[0]["seq"], 0);
        assert_eq!(rest[0]["content"], "依頼です", "AgentMessage は平らに展開される");
        assert_eq!(rest[1]["kind"], "exchange");
        assert_eq!(rest[1]["sent"], "送った文字列");
        assert_eq!(rest[2]["kind"], "summary");
        assert_eq!(rest[2]["coversUpToSeq"], 1);

        // 行を Record として読み戻せる = 会話が再構成できる。
        let decoded: Record = serde_json::from_str(lines_record(&rest[1])).unwrap();
        assert_eq!(
            decoded,
            Record::exchange(&agent(), "送った文字列", "返った")
        );
    }

    /// 書き出し行から `seq` を落として [`Record`] の形へ戻す（テスト補助）。
    fn lines_record(value: &serde_json::Value) -> &'static str {
        // serde_json::Value から &'static str は作れないので、Box::leak で寿命を伸ばす。
        // テスト内の 1 回だけの割り当てで、プロセス終了まで持てば足りる。
        let mut object = value.clone();
        object.as_object_mut().unwrap().remove("seq");
        Box::leak(object.to_string().into_boxed_str())
    }

    /// 閉じて開き直しても全部残っている（S1 の保存側）。
    #[test]
    fn reopening_the_file_keeps_sessions_and_records() {
        let dir = TempDir::new();
        let id = {
            let store = SessionStore::open(dir.db()).expect("開けること");
            let id = store.create_session(None).unwrap();
            store.append(&id, &Record::message(user_message("再起動前"))).unwrap();
            store
                .append(&id, &Record::exchange(&agent(), "送った文字列そのもの", "はい"))
                .unwrap();
            id
        };

        let store = SessionStore::open(dir.db()).expect("開き直せること");
        assert_eq!(store.latest_session().unwrap().as_deref(), Some(id.as_str()));
        assert_eq!(store.tail_messages(&id, 100).unwrap()[0].content, "再起動前");
        let restored = store.restore_histories(&id, 8).unwrap();
        assert_eq!(
            restored.histories[&agent()][0].content,
            "送った文字列そのもの",
            "履歴は log からではなく exchange から戻る"
        );
    }

    /// 存在しないセッションへの操作は、空ではなく失敗として返す。
    #[test]
    fn operations_on_an_unknown_session_fail_loudly() {
        let dir = TempDir::new();
        let store = SessionStore::open(dir.db()).expect("開けること");

        let appended = store.append("居ない", &Record::message(user_message("宛先なし")));
        assert!(matches!(appended, Err(CoreError::SessionNotFound(_))));
        assert!(matches!(
            store.fork_session("居ない", 0),
            Err(CoreError::SessionNotFound(_))
        ));
        assert!(matches!(
            store.export_session("居ない", &mut Vec::new()),
            Err(CoreError::SessionNotFound(_))
        ));
        assert!(store.session_meta("居ない").unwrap().is_none(), "問い合わせは None");
        store.delete_session("居ない").expect("削除は冪等");
    }
}
