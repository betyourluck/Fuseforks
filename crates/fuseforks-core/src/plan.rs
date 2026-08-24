//! plan 実行の観測記録（Spec 08 — 波ペイン）。
//!
//! `run_plan` が波の開始・タスクの解決・波の完了をここへ刻み、GUI は
//! [`crate::event::CoreEvent`] の 3 種（更新）と `list_plan_waves`（再投影）で読む。
//!
//! **所有者は in-memory で、ファイルへは書かない**（会話ログ・統計と同じ
//! プロセス寿命。再起動生存は別 Spec の管轄）。記録は読み取り専用の観測であり、
//! ここから plan を操作する経路は作らない。

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::model::AgentId;

/// 保持する波の上限。押し出しは**状態を問わず**古い方から。
///
/// 記録は文字列の本文を持たない（文字数と分類だけ）ので、律速はメモリではなく
/// 「見て意味のある遡り幅」。実測が出たら動かす（1 波 ≈ 400 byte、50 波 ≈ 20 KB）。
pub const PLAN_WAVE_CAPACITY: usize = 50;

/// plan の 1 タスクの解決分類。
///
/// **文言 parse では取らない** — 文言を直した瞬間に黙って壊れる。分類は
/// `deliver_and_wait` の返り値と、返信路の積み荷（`Reply.kind` — 刻み手は
/// `handle_message` の Finish / Handoff 分岐の 1 箇所）が型で運ぶ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanTaskState {
    /// 配送済み・解決待ち（状態遷移の始点）。
    Running,
    /// 答えが返った。
    Answered,
    /// 答えではなく「転送した」という事実が返った。
    HandedOff,
    /// 停止中・受信箱飽和で配送できなかった。
    Undeliverable,
    /// 相手が答えずタスクを終えた。
    NoAnswer,
    /// 時間内に返らなかった。
    TimedOut,
    /// 人が止めさせた（Spec 10）。失敗ではない — セル色も失敗色にしない
    /// （`turn_interrupt` の不変条件 4 と同じ判断）。刻み手は 3 箇所に限る:
    /// 飛行中の中断（`handle_message` の検知点）・未着手封筒の畳み
    /// （`agent_loop` のターン開始直後、Phase 2）・`run_plan` の
    /// `finish_wave`（割り込みで波を閉じるときの running の倒し先、Phase 2）。
    Interrupted,
    /// トークン予算の天井が止めた（Spec 11）。資源の事実なので色は失敗系 —
    /// 人の打ち切り（`Interrupted` の注意色）と混ぜない。優先順位は
    /// `token_budget.precedence`（cancel > budget_exhausted > RepeatGuard >
    /// hop / rounds）。
    BudgetExhausted,
}

/// 波レベルの状態（Spec 43）。**セルの分類（[`PlanTaskState`]）とは別の軸** —
/// あちらは配送されたタスクの結末、こちらは波そのものの段。
///
/// serde default = `dispatched`（[`default_wave_state`]）— 旧レコード・
/// 編集窓 OFF の経路は従来の意味のまま（加算的変更。`Interrupted` /
/// `BudgetExhausted` と同じ流儀）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanWaveState {
    /// 提案（編集窓）。配送されておらず、人の承認・破棄を待っている。
    Pending,
    /// 配送済み（従来の波はすべてこれ）。
    Dispatched,
    /// 人が破棄した。配送は一度も起きていない。
    Discarded,
}

/// [`PlanWaveState`] の serde 既定値。関数なのは serde の `default =` が
/// パスしか受けないため。
fn default_wave_state() -> PlanWaveState {
    PlanWaveState::Dispatched
}

/// `dispatch_plan_wave` / `discard_plan_wave` が受けるタスクの入力形
/// （ワイヤは `{to, message}` — plan ツールの引数と同じ 2 欄）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanTaskInput {
    /// 宛先。
    pub to: AgentId,
    /// 依頼本文。
    pub message: String,
}

/// `PlanWaveStarted` が運ぶタスクの告知形。開始時点で確定している 2 欄だけを持つ
/// （state は必ず `running`、elapsed は未確定 — 載せると「まだ無い値」の席になる）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanTaskAnnounced {
    /// 宛先。
    pub to: AgentId,
    /// 依頼本文の文字数。
    pub msg_chars: u32,
}

/// 波の 1 タスクの記録。同一性は `(plan_id, to)` —
/// 同一宛先の重複は静的な不正として配送前に差し戻される（Spec 04）ため、
/// 配送された波の中で `to` は必ず一意。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanTaskRecord {
    /// 宛先。
    pub to: AgentId,
    /// 解決分類。**波が `pending` の間は未配送の置き場**（`running` のまま。
    /// セルの意味は波の state が決め、dispatch がタスクを最終形で置き換える）。
    pub state: PlanTaskState,
    /// 配送からこのタスクの解決まで。相手のキュー待ちを含む（並列なのは配送）。
    pub elapsed_ms: Option<u64>,
    /// 依頼本文の文字数（配送された波は本文そのものを持たない）。
    pub msg_chars: u32,
    /// 依頼本文。**`pending` の間だけ埋まる**（編集 UI が読む — 提案の真実。
    /// Spec 43 D4）。dispatch / discard で落とす — 配送後の本文は束ねと
    /// ワーカーの履歴に住み、記録が二重に持つとリングの概算が嘘になる。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// plan 1 波の実行記録。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanWaveRecord {
    /// プロセス内で単調増加。**1 始まり・0 は予約**（未採番と空状態の区別用）。
    /// モデルには見せない — 束ねの文言・ツール説明は変えない。
    pub plan_id: u64,
    /// 進行役。
    pub agent_id: AgentId,
    /// ターン内連番（stderr の `wave=` と同じ値）。同定は `plan_id` の仕事。
    pub wave: u32,
    /// 波レベルの状態（Spec 43）。旧レコードは欄が無く `dispatched` へ落ちる。
    #[serde(default = "default_wave_state")]
    pub state: PlanWaveState,
    /// 波の開始時刻（epoch ms・壁時計）。`elapsed_ms` は単調時計由来で別系統。
    /// **`pending` では提示時刻、dispatch が配送時刻で上書きする**（Spec 43 —
    /// 所要の意味を「配送からの経過」に保つ）。
    pub started_at_ms: u64,
    /// タスク。**入力順**（束ねと同じ。解決順ではない）。
    pub tasks: Vec<PlanTaskRecord>,
    /// 束ねの文字数。波の完了時に埋まる。
    pub bundle_chars: Option<u64>,
    /// 波全体の所要（= キュー待ち込みの最遅 1 体分）。波の完了時に埋まる。
    pub elapsed_ms: Option<u64>,
}

/// 波のリングバッファ。純データで、イベントの発行と時刻の取得は呼び出し側が持つ
/// （壁時計を内蔵するとテストが実行機の時計に依存する — schedule の純関数と同じ判断）。
pub struct PlanWaveStore {
    waves: VecDeque<PlanWaveRecord>,
    /// 次に払い出す `plan_id`。1 始まり。
    next_id: u64,
}

impl Default for PlanWaveStore {
    fn default() -> Self {
        Self {
            waves: VecDeque::new(),
            next_id: 1,
        }
    }
}

impl PlanWaveStore {
    /// 波を開始し、`plan_id` を払い出す。タスクは `(宛先, 本文の文字数)` の入力順。
    pub fn begin_wave(
        &mut self,
        agent_id: AgentId,
        wave: u32,
        tasks: &[(AgentId, u32)],
        started_at_ms: u64,
    ) -> u64 {
        let plan_id = self.next_id;
        self.next_id += 1;

        if self.waves.len() >= PLAN_WAVE_CAPACITY {
            // 状態を問わず古い方から捨てる。実行中の波を特別扱いすると、
            // 詰まった波が居座り続けて上限が上限でなくなる。
            self.waves.pop_front();
        }
        self.waves.push_back(PlanWaveRecord {
            plan_id,
            agent_id,
            wave,
            state: PlanWaveState::Dispatched,
            started_at_ms,
            tasks: tasks
                .iter()
                .map(|(to, msg_chars)| PlanTaskRecord {
                    to: to.clone(),
                    state: PlanTaskState::Running,
                    elapsed_ms: None,
                    msg_chars: *msg_chars,
                    message: None,
                })
                .collect(),
            bundle_chars: None,
            elapsed_ms: None,
        });
        plan_id
    }

    /// 提案（編集窓・Spec 43）を記録し、`plan_id` を払い出す。配送は起きていない。
    ///
    /// 本文を持つのはこの形だけ — 提案の真実は**ここに記録された tasks**で、
    /// フロントの編集中状態は真実ではない（D4）。
    pub fn begin_pending_wave(
        &mut self,
        agent_id: AgentId,
        wave: u32,
        tasks: &[(AgentId, String)],
        started_at_ms: u64,
    ) -> u64 {
        let plan_id = self.next_id;
        self.next_id += 1;

        if self.waves.len() >= PLAN_WAVE_CAPACITY {
            self.waves.pop_front();
        }
        self.waves.push_back(PlanWaveRecord {
            plan_id,
            agent_id,
            wave,
            state: PlanWaveState::Pending,
            started_at_ms,
            tasks: tasks
                .iter()
                .map(|(to, message)| PlanTaskRecord {
                    to: to.clone(),
                    state: PlanTaskState::Running,
                    elapsed_ms: None,
                    msg_chars: message.chars().count() as u32,
                    message: Some(message.clone()),
                })
                .collect(),
            bundle_chars: None,
            elapsed_ms: None,
        });
        plan_id
    }

    /// 提案の中身を返す（`dispatch_plan_wave` / `discard_plan_wave` の入口検査用）。
    /// `pending` でない・押し出し済みなら `None`。
    pub fn proposal(&self, plan_id: u64) -> Option<(AgentId, u32)> {
        self.waves
            .iter()
            .find(|w| w.plan_id == plan_id && w.state == PlanWaveState::Pending)
            .map(|w| (w.agent_id.clone(), w.wave))
    }

    /// 提案を配送済みへ進め、タスクを**人が承認した最終形で置き換える**（Spec 43）。
    ///
    /// 提示時の tasks は残さない — 配送の記録が持つべきは走った形だけで、
    /// 提示と配送の差分は `plan pending:` / `plan dispatch:` のログが運ぶ。
    /// `pending` でなければ何もしない（呼び出し側が `proposal` で先に確かめる —
    /// 押し出し済みへの更新を無視する `resolve_task` と同じ規律）。
    pub fn dispatch_wave(
        &mut self,
        plan_id: u64,
        tasks: &[(AgentId, u32)],
        started_at_ms: u64,
    ) {
        let Some(wave) = self
            .waves
            .iter_mut()
            .find(|w| w.plan_id == plan_id && w.state == PlanWaveState::Pending)
        else {
            return;
        };
        wave.state = PlanWaveState::Dispatched;
        wave.started_at_ms = started_at_ms;
        wave.tasks = tasks
            .iter()
            .map(|(to, msg_chars)| PlanTaskRecord {
                to: to.clone(),
                state: PlanTaskState::Running,
                elapsed_ms: None,
                msg_chars: *msg_chars,
                message: None,
            })
            .collect();
    }

    /// 提案を破棄する（Spec 43）。本文を落とし、配送は一度も起きない。
    /// `pending` でなければ何もしない（`dispatch_wave` と同じ規律）。
    pub fn discard_wave(&mut self, plan_id: u64) {
        let Some(wave) = self
            .waves
            .iter_mut()
            .find(|w| w.plan_id == plan_id && w.state == PlanWaveState::Pending)
        else {
            return;
        };
        wave.state = PlanWaveState::Discarded;
        for task in &mut wave.tasks {
            task.message = None;
        }
    }

    /// タスクの解決を記録する。押し出された波への更新は**窓の外として無視**する
    /// （event は普通に飛ぶので、投影側の欠けであって配送の欠けではない）。
    pub fn resolve_task(&mut self, plan_id: u64, to: &AgentId, state: PlanTaskState, elapsed_ms: u64) {
        let Some(wave) = self.waves.iter_mut().find(|w| w.plan_id == plan_id) else {
            return;
        };
        if let Some(task) = wave.tasks.iter_mut().find(|t| t.to == *to) {
            task.state = state;
            task.elapsed_ms = Some(elapsed_ms);
        }
    }

    /// 波の完了を記録する。押し出し済みなら無視（`resolve_task` と同じ規律）。
    ///
    /// `Running` のまま残ったタスクは `NoAnswer` に倒す。この経路へ来るのは
    /// JoinSet のタスク異常（パニック）だけで、`deliver_and_wait` は必ず分類を
    /// 返す — 完了した波に永遠の「実行中」を残さないための後始末であって、
    /// 正常系の分類ではない。
    pub fn finish_wave(&mut self, plan_id: u64, bundle_chars: u64, elapsed_ms: u64) {
        let Some(wave) = self.waves.iter_mut().find(|w| w.plan_id == plan_id) else {
            return;
        };
        wave.bundle_chars = Some(bundle_chars);
        wave.elapsed_ms = Some(elapsed_ms);
        for task in &mut wave.tasks {
            if task.state == PlanTaskState::Running {
                task.state = PlanTaskState::NoAnswer;
            }
        }
    }

    /// 保持中の全記録（古い順・実行中の波も含む）。
    pub fn list(&self) -> Vec<PlanWaveRecord> {
        self.waves.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(id: &str) -> AgentId {
        AgentId::from(id)
    }

    /// `Interrupted` は加算的変更であること（Spec 10 Phase 4）。
    ///
    /// 旧レコードの wire 値（interrupted を知らない時代の 6 値）はそのまま読め、
    /// 新値は snake_case で往復する — マイグレーションは不要。この保証が
    /// 崩れる変更（値の改名・削除）は、配布済みの記録を読めなくする。
    #[test]
    fn interrupted_is_an_additive_wire_value() {
        // 旧時代の値はそのまま読める。
        for (wire, expected) in [
            ("\"running\"", PlanTaskState::Running),
            ("\"answered\"", PlanTaskState::Answered),
            ("\"handed_off\"", PlanTaskState::HandedOff),
            ("\"undeliverable\"", PlanTaskState::Undeliverable),
            ("\"no_answer\"", PlanTaskState::NoAnswer),
            ("\"timed_out\"", PlanTaskState::TimedOut),
        ] {
            let parsed: PlanTaskState = serde_json::from_str(wire).unwrap();
            assert_eq!(parsed, expected, "旧値 {wire} が読めること");
        }

        // 新値は snake_case で往復する。
        let wire = serde_json::to_string(&PlanTaskState::Interrupted).unwrap();
        assert_eq!(wire, "\"interrupted\"");
        let round: PlanTaskState = serde_json::from_str(&wire).unwrap();
        assert_eq!(round, PlanTaskState::Interrupted);
    }

    /// `BudgetExhausted` も加算的変更であること（Spec 11 P3 — 旧 7 値が
    /// そのまま読め、新値が snake_case で往復する）。
    #[test]
    fn budget_exhausted_is_an_additive_wire_value() {
        // interrupted までの旧 7 値はそのまま読める。
        for (wire, expected) in [
            ("\"running\"", PlanTaskState::Running),
            ("\"answered\"", PlanTaskState::Answered),
            ("\"handed_off\"", PlanTaskState::HandedOff),
            ("\"undeliverable\"", PlanTaskState::Undeliverable),
            ("\"no_answer\"", PlanTaskState::NoAnswer),
            ("\"timed_out\"", PlanTaskState::TimedOut),
            ("\"interrupted\"", PlanTaskState::Interrupted),
        ] {
            let parsed: PlanTaskState = serde_json::from_str(wire).unwrap();
            assert_eq!(parsed, expected, "旧値 {wire} が読めること");
        }

        let wire = serde_json::to_string(&PlanTaskState::BudgetExhausted).unwrap();
        assert_eq!(wire, "\"budget_exhausted\"");
        let round: PlanTaskState = serde_json::from_str(&wire).unwrap();
        assert_eq!(round, PlanTaskState::BudgetExhausted);
    }

    /// 波レベル状態は加算的変更であること（Spec 43）。
    ///
    /// 旧レコード（state 欄を知らない時代）はそのまま読めて `dispatched` へ
    /// 落ち、pending のタスクだけが本文を持つ。
    #[test]
    fn wave_state_is_an_additive_wire_value() {
        let mut store = PlanWaveStore::default();
        let id = store.begin_wave(agent("agent_1"), 1, &[(agent("agent_2"), 7)], 0);
        let mut json = serde_json::to_value(&store.list()[0]).unwrap();

        // 配送済みの波は本文を持たない（skip_serializing_if — 旧クライアントが
        // 知らない欄を読む席をそもそも作らない）。
        assert!(json["tasks"][0].get("message").is_none());
        assert_eq!(json["state"], "dispatched");

        // 旧レコード相当（state 欄なし）はそのまま読めて dispatched へ落ちる。
        json.as_object_mut().unwrap().remove("state");
        let parsed: PlanWaveRecord = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.plan_id, id);
        assert_eq!(parsed.state, PlanWaveState::Dispatched);
    }

    /// 提案は本文を持ち、dispatch が最終形で置き換えて本文を落とす（Spec 43 D4）。
    #[test]
    fn a_pending_wave_carries_messages_until_dispatched() {
        let mut store = PlanWaveStore::default();
        let id = store.begin_pending_wave(
            agent("agent_1"),
            1,
            &[
                (agent("agent_2"), "調べて".to_owned()),
                (agent("agent_3"), "まとめて".to_owned()),
            ],
            100,
        );

        let wave = &store.list()[0];
        assert_eq!(wave.state, PlanWaveState::Pending);
        assert_eq!(wave.tasks[0].message.as_deref(), Some("調べて"));
        assert_eq!(wave.tasks[0].msg_chars, 3);
        assert_eq!(store.proposal(id), Some((agent("agent_1"), 1)));

        // 人が 1 件消して 1 件直した最終形で配送。
        store.dispatch_wave(id, &[(agent("agent_3"), 5)], 200);
        let wave = &store.list()[0];
        assert_eq!(wave.state, PlanWaveState::Dispatched);
        assert_eq!(wave.started_at_ms, 200, "所要の起点は配送時刻へ動く");
        assert_eq!(wave.tasks.len(), 1, "タスクは承認された最終形で置き換わる");
        assert_eq!(wave.tasks[0].to, agent("agent_3"));
        assert!(wave.tasks[0].message.is_none(), "配送後は本文を持たない");
        // 確定した波はもう提案ではない。
        assert_eq!(store.proposal(id), None);
    }

    /// discard は pending にだけ効き、本文を落とす。確定済みへは何もしない。
    #[test]
    fn discard_only_settles_a_pending_wave() {
        let mut store = PlanWaveStore::default();
        let pending =
            store.begin_pending_wave(agent("agent_1"), 1, &[(agent("agent_2"), "調べて".to_owned())], 0);
        let dispatched = store.begin_wave(agent("agent_1"), 2, &[(agent("agent_2"), 3)], 0);

        store.discard_wave(pending);
        store.discard_wave(dispatched); // 何もしない（pending ではない）

        let waves = store.list();
        assert_eq!(waves[0].state, PlanWaveState::Discarded);
        assert!(waves[0].tasks[0].message.is_none(), "破棄で本文を落とす");
        assert_eq!(waves[1].state, PlanWaveState::Dispatched);
        // 破棄済みへの dispatch も何もしない（一方通行）。
        store.dispatch_wave(pending, &[(agent("agent_2"), 3)], 9);
        assert_eq!(store.list()[0].state, PlanWaveState::Discarded);
    }

    #[test]
    fn plan_ids_start_at_one() {
        let mut store = PlanWaveStore::default();
        let first = store.begin_wave(agent("agent_1"), 1, &[(agent("agent_2"), 10)], 0);
        let second = store.begin_wave(agent("agent_1"), 2, &[(agent("agent_2"), 10)], 0);
        // 0 は「未採番」の予約値。空状態と最初の波を区別できるようにする。
        assert_eq!(first, 1);
        assert_eq!(second, 2);
    }

    #[test]
    fn tasks_begin_as_running_in_input_order() {
        let mut store = PlanWaveStore::default();
        store.begin_wave(
            agent("agent_1"),
            1,
            &[(agent("agent_3"), 30), (agent("agent_2"), 20)],
            123,
        );

        let waves = store.list();
        assert_eq!(waves.len(), 1);
        assert_eq!(waves[0].started_at_ms, 123);
        // 入力順を保つ（解決順・ID 順に並べ替えない）。
        assert_eq!(waves[0].tasks[0].to, agent("agent_3"));
        assert_eq!(waves[0].tasks[1].to, agent("agent_2"));
        assert!(waves[0].tasks.iter().all(|t| t.state == PlanTaskState::Running));
        assert_eq!(waves[0].bundle_chars, None);
    }

    #[test]
    fn resolution_updates_the_matching_task_only() {
        let mut store = PlanWaveStore::default();
        let id = store.begin_wave(
            agent("agent_1"),
            1,
            &[(agent("agent_2"), 10), (agent("agent_3"), 10)],
            0,
        );
        store.resolve_task(id, &agent("agent_3"), PlanTaskState::HandedOff, 42);

        let waves = store.list();
        assert_eq!(waves[0].tasks[0].state, PlanTaskState::Running);
        assert_eq!(waves[0].tasks[1].state, PlanTaskState::HandedOff);
        assert_eq!(waves[0].tasks[1].elapsed_ms, Some(42));
    }

    #[test]
    fn finish_fills_totals_and_closes_leftover_running() {
        let mut store = PlanWaveStore::default();
        let id = store.begin_wave(
            agent("agent_1"),
            1,
            &[(agent("agent_2"), 10), (agent("agent_3"), 10)],
            0,
        );
        store.resolve_task(id, &agent("agent_2"), PlanTaskState::Answered, 5);
        // agent_3 は解決が来ないまま（JoinSet パニックの経路）。
        store.finish_wave(id, 400, 99);

        let waves = store.list();
        assert_eq!(waves[0].bundle_chars, Some(400));
        assert_eq!(waves[0].elapsed_ms, Some(99));
        assert_eq!(waves[0].tasks[0].state, PlanTaskState::Answered);
        // 完了した波に永遠の「実行中」を残さない。
        assert_eq!(waves[0].tasks[1].state, PlanTaskState::NoAnswer);
    }

    #[test]
    fn ring_evicts_the_oldest_regardless_of_state() {
        let mut store = PlanWaveStore::default();
        // 最古の 1 波は実行中のまま（resolve も finish もしない）。
        let oldest = store.begin_wave(agent("agent_1"), 1, &[(agent("agent_2"), 1)], 0);
        for i in 0..PLAN_WAVE_CAPACITY {
            let id = store.begin_wave(agent("agent_1"), i as u32 + 2, &[(agent("agent_2"), 1)], 0);
            store.finish_wave(id, 1, 1);
        }

        let waves = store.list();
        assert_eq!(waves.len(), PLAN_WAVE_CAPACITY);
        assert!(
            waves.iter().all(|w| w.plan_id != oldest),
            "実行中でも最古なら押し出されること"
        );
        // 古い順を保つ。
        assert!(waves.windows(2).all(|pair| pair[0].plan_id < pair[1].plan_id));
    }

    #[test]
    fn updates_to_an_evicted_wave_are_ignored() {
        let mut store = PlanWaveStore::default();
        let evicted = store.begin_wave(agent("agent_1"), 1, &[(agent("agent_2"), 1)], 0);
        for i in 0..PLAN_WAVE_CAPACITY {
            store.begin_wave(agent("agent_1"), i as u32 + 2, &[(agent("agent_2"), 1)], 0);
        }

        // 窓の外への更新。パニックせず、何も変えない。
        store.resolve_task(evicted, &agent("agent_2"), PlanTaskState::Answered, 1);
        store.finish_wave(evicted, 1, 1);
        assert!(store.list().iter().all(|w| w.plan_id != evicted));
    }

    #[test]
    fn records_serialize_as_camel_case() {
        let mut store = PlanWaveStore::default();
        let id = store.begin_wave(agent("agent_1"), 1, &[(agent("agent_2"), 7)], 55);
        store.resolve_task(id, &agent("agent_2"), PlanTaskState::HandedOff, 3);

        let json = serde_json::to_value(&store.list()[0]).unwrap();
        assert_eq!(json["planId"], 1);
        assert_eq!(json["startedAtMs"], 55);
        assert_eq!(json["tasks"][0]["msgChars"], 7);
        // 分類の値は snake_case（data_contract の enums と同じ流儀）。
        assert_eq!(json["tasks"][0]["state"], "handed_off");
    }
}
