//! 統計画面の集計（Spec 39 D3）— **純関数だけ**。
//!
//! 契約は `data_contract.yaml` の `stats_contract` が正。ここには I/O も時計も無い —
//! `sessions.redb` を読んで [`TurnRecord`] を集めるのは呼び出し側（`Orchestrator::session_stats`）
//! の仕事で、本モジュールは集めた列を [`StatsReport`] へ畳むだけ。`budget.rs` と同じ形。
//!
//! # 原本と単位
//!
//! 原本は [`crate::session_store::Record::Turn`]（ターン 1 本の使用量）。単位はトークンで、
//! **通貨には換算しない**（2026-08-15 利用者裁定 — 価格表は各社の改定に追従できず、同じ
//! モデルでもキャッシュの帯で単価が変わる。#104）。「いくら払ったか」の比較可能な 1 つの
//! 数字は Spec 11 の重みで畳んだ**実効トークン**で、その重みは [`crate::budget`] の 1 実装
//! （ここで再計算しない — 2 箇所目を作ると片方が古い重みのまま通り続ける）。
//!
//! # 遡れない
//!
//! この版より前の会話には `Turn` が無い。`recorded_since` が `None` のスコープは
//! 「記録が無い」のであって「払っていない」のではない — 呼び出し側は 0 の表を出さない
//! （`stats_contract`）。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::budget::effective_tokens;
use crate::llm::Usage;
use crate::session_store::{SessionSummary, TurnRecord, TurnStop};

/// `series` に残す末尾の件数（`MESSAGE_LIMIT` と同数）。落とした件数は
/// [`StatsSeries::dropped`] に出す — **溢れは数える**（#72）。
pub const SERIES_LIMIT: usize = 500;

/// 集計の範囲。閉じた列挙 2 値。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StatsScope {
    /// 1 会話。
    #[serde(rename_all = "camelCase")]
    Session {
        /// 会話の ID。
        session_id: String,
    },
    /// この村の全会話。
    All,
}

/// 使用量の 1 切片（村全体 / 個体別で同じ形）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StatsSlice {
    /// ターン数（`Turn` の件数）。
    pub turns: u64,
    /// うち、払ったが答えが無かったターン（[`TurnStop::is_failure`]）。
    pub failed: u64,
    /// 入力トークンの合計。
    pub prompt: u64,
    /// 入力のうちキャッシュから読んだぶんの合計。
    pub cached: u64,
    /// 出力トークンの合計。
    pub completion: u64,
    /// 出力のうち思考のぶんの合計（`completion` の内数）。
    pub reasoning: u64,
    /// 入力のうちキャッシュへ**書き込んだ**ぶんの合計（`prompt` の内数。Spec 40）。
    ///
    /// **素の新規入力は `prompt - cached - cache_write`。** 実測ではこれが
    /// 1 ラウンドあたり数トークンで、**「未キャッシュ」の実体はほぼ全部が
    /// 書き込み**だった（Anthropic 100.0% / OpenAI 互換 99.8%）。
    ///
    /// **[`Self::effective`] には 1.0× で入っている**（Spec 40 D3）。実際の課金は
    /// TTL とワイヤ次第で 1.25〜2.0× なので、**金額が要るならこの生の数へ外部の
    /// 単価表を当てる** — 実効トークンは歯止めの単位であって通貨ではない。
    pub cache_write: u64,
    /// うち 1 時間 TTL のぶん（`cache_write` の部分集合）。
    ///
    /// TTL 別の内訳を返すのは Anthropic だけ。**OpenAI 形の 0 は相手の TTL に
    /// ついての観測ではない**（decode が 0 を焼いている）。
    pub cache_write_1h: u64,
    /// 実効トークンの合計（Spec 11 の重み — `budget.rs` の 1 実装）。
    pub effective: u64,
    /// `prompt > 0 ? cached / prompt : 0`（AgentCard と同じ分母）。
    pub cache_rate: f64,
    /// `(prompt + completion) > 0 ? completion / (prompt + completion) : 0`。
    pub output_share: f64,
    /// 経過時間の算術平均（ms）。ターンが無ければ 0。
    pub avg_elapsed_ms: u64,
    /// `(prompt + completion) / turns` の算術平均。ターンが無ければ 0。
    pub avg_tokens_per_turn: u64,
}

/// **(個体, モデル)** 別の 1 行。
///
/// 1 個体が複数行になりうる — テンプレートを差し替えた個体は、モデルごとに割れる。
/// 畳まないのは**単価がモデルごとに違う**ため（外部の価格表を当てて金額を出す用途で、
/// 個体だけで畳むと切り替え前のターンまで最後のモデルの単価で計算される）。
/// `agent_id` は `by_agent` の鍵として一意ではないので、**表示の鍵にも使えない**。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStats {
    /// 個体。**一意ではない**（同じ個体がモデルごとに複数行を持つ）。
    pub agent_id: String,
    /// この行のモデル名。`TurnRecord.model` = テンプレートの `model`。
    pub model: String,
    /// 使用量。
    #[serde(flatten)]
    pub slice: StatsSlice,
}

/// 終わり方の内訳の 1 行。`failed` は CODE ごとに分ける。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StopCount {
    /// [`TurnStop`] の種別名（`completed` / `repeat` / `tool_limit` / `failed` /
    /// `interrupted` / `budget_exhausted` / `reserve_short`）。
    pub stop: String,
    /// `failed` のとき、エラーのコード。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// 件数。
    pub count: u64,
}

/// 時系列の 1 点（ターン 1 本）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesPoint {
    /// ターン開始の壁時計。
    pub ts_ms: u64,
    /// 個体。
    pub agent_id: String,
    /// 実効トークン。
    pub effective: u64,
    /// 入力。
    pub prompt: u64,
    /// 出力。
    pub completion: u64,
    /// 終わり方。
    pub stop: TurnStop,
}

/// 時系列（`session` スコープだけ。`all` では出さない — 会話をまたいだ末尾 N 件は
/// 古い会話が丸ごと消えた列になり、棒を見て「その会話は払っていない」と読める）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsSeries {
    /// 末尾 [`SERIES_LIMIT`] 件。開始時刻の昇順。
    pub points: Vec<SeriesPoint>,
    /// 上限で落とした件数。
    pub dropped: u32,
}

/// 会話ごとの合計（`all` の主役の表。`session` では 1 件）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStats {
    /// 会話の ID。
    pub session_id: String,
    /// 表題。
    pub title: String,
    /// 分岐元。**セッションの属性**であってターンの属性ではない（`SessionMeta.parent_id`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<String>,
    /// ターン数。
    pub turns: u64,
    /// 実効トークンの合計。
    pub effective: u64,
}

/// スコープの説明。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsScopeMeta {
    /// スコープ内で最初の `Turn` の開始時刻。**無ければ `None`** — 呼び出し側は
    /// 0 の表を出さず「記録が無い」と言う（D6）。
    pub recorded_since: Option<u64>,
    /// 会話ごとの合計。`all` では `list_sessions` の並び（更新の新しい順）。
    pub sessions: Vec<SessionStats>,
}

/// 集計の結果（IPC `session_stats` の戻り）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsReport {
    /// 集計の範囲。
    pub scope: StatsScope,
    /// スコープの説明。
    pub scope_meta: StatsScopeMeta,
    /// スコープ全体。
    pub totals: StatsSlice,
    /// **(個体, モデル)** 別。実効トークンの多い順、同点は id 順 → モデル名順。
    pub by_agent: Vec<AgentStats>,
    /// 終わり方の内訳。件数の多い順、同点は種別名 → コード順。
    pub by_stop: Vec<StopCount>,
    /// 時系列（`session` のみ）。
    pub series: Option<StatsSeries>,
}

/// `TurnRecord` の使用量を `Usage` の形へ（実効の重み関数が `Usage` を受けるため）。
fn usage_of(turn: &TurnRecord) -> Usage {
    Usage {
        prompt: turn.prompt,
        completion: turn.completion,
        cache_read: turn.cached,
        // **P1 では 0 固定**（Spec 40）。`TurnRecord` に欄が生えるのは P3。
        // **P3 の後も 0 のまま渡す** — D3 で「実効トークンは書き込みを 1.0× で
        // 数える」と決めているので、渡すと天井の意味が黙って変わる。
        cache_write: 0,
        cache_write_1h: 0,
        reasoning: turn.reasoning,
    }
}

/// ターン 1 本の実効トークン。**`budget.rs` の 1 実装を呼ぶ**（`reasoning` は
/// `completion` の内数なので、重み関数の中でも足していない）。
fn effective_of(turn: &TurnRecord) -> u64 {
    effective_tokens(&usage_of(turn))
}

/// 切片を積む。
#[derive(Default)]
struct SliceAcc {
    turns: u64,
    failed: u64,
    prompt: u64,
    cached: u64,
    completion: u64,
    reasoning: u64,
    cache_write: u64,
    cache_write_1h: u64,
    effective: u64,
    elapsed_ms: u64,
}

impl SliceAcc {
    fn push(&mut self, turn: &TurnRecord) {
        self.turns += 1;
        if turn.stop.is_failure() {
            self.failed += 1;
        }
        self.prompt = self.prompt.saturating_add(turn.prompt);
        self.cached = self.cached.saturating_add(turn.cached);
        self.completion = self.completion.saturating_add(turn.completion);
        self.reasoning = self.reasoning.saturating_add(turn.reasoning);
        // **`effective_of` には渡らない**（D3 — 実効は書き込みを 1.0× で数える）。
        // ここは表示のための実数で、外の単価表を当てる側が使う。
        self.cache_write = self.cache_write.saturating_add(turn.cache_write);
        self.cache_write_1h = self.cache_write_1h.saturating_add(turn.cache_write_1h);
        self.effective = self.effective.saturating_add(effective_of(turn));
        self.elapsed_ms = self.elapsed_ms.saturating_add(turn.elapsed_ms);
    }

    fn finish(&self) -> StatsSlice {
        let io = self.prompt.saturating_add(self.completion);
        StatsSlice {
            turns: self.turns,
            failed: self.failed,
            prompt: self.prompt,
            cached: self.cached,
            completion: self.completion,
            reasoning: self.reasoning,
            cache_write: self.cache_write,
            cache_write_1h: self.cache_write_1h,
            effective: self.effective,
            cache_rate: ratio(self.cached, self.prompt),
            output_share: ratio(self.completion, io),
            avg_elapsed_ms: self.elapsed_ms.checked_div(self.turns).unwrap_or(0),
            avg_tokens_per_turn: io.checked_div(self.turns).unwrap_or(0),
        }
    }
}

/// 0 除算をガードした比。**分母 0 は 0**（契約の定義そのもの）。
fn ratio(num: u64, den: u64) -> f64 {
    if den == 0 {
        0.0
    } else {
        num as f64 / den as f64
    }
}

/// [`TurnStop`] の種別名（`serde` の `kind` と同じ綴り）。
fn stop_kind(stop: &TurnStop) -> &'static str {
    match stop {
        TurnStop::Completed => "completed",
        TurnStop::Repeat { .. } => "repeat",
        TurnStop::ToolLimit => "tool_limit",
        TurnStop::Failed { .. } => "failed",
        TurnStop::Interrupted => "interrupted",
        TurnStop::BudgetExhausted => "budget_exhausted",
        TurnStop::ReserveShort => "reserve_short",
    }
}

/// 集計 — **純関数**。
///
/// `turns` は `(session_id, TurnRecord)` の列（順序は問わない。中で開始時刻順に並べる）、
/// `sessions` はスコープに含める会話の一覧（`session` なら 1 件、`all` なら
/// `list_sessions()` の全件）。`turns` に居て `sessions` に居ない会話は
/// `scope_meta.sessions` に**現れない**が、`totals` / `by_agent` / `by_stop` には数える。
#[must_use]
pub fn aggregate(
    turns: &[(String, TurnRecord)],
    sessions: &[SessionSummary],
    scope: StatsScope,
) -> StatsReport {
    // 開始時刻順に並べる（`series` は末尾 N 件）。
    let mut ordered: Vec<&(String, TurnRecord)> = turns.iter().collect();
    ordered.sort_by_key(|(_, t)| t.ts_ms);

    let mut totals = SliceAcc::default();
    // 鍵は **(個体, モデル)**。個体だけで畳むと、テンプレートを差し替えた個体の
    // 全ターンが最後のモデルの下に集まり、単価を掛けたときに金額が狂う。
    let mut by_agent: BTreeMap<(&str, &str), SliceAcc> = BTreeMap::new();
    let mut by_stop: BTreeMap<(&'static str, Option<&str>), u64> = BTreeMap::new();
    let mut by_session: BTreeMap<&str, (u64, u64)> = BTreeMap::new();

    for (session_id, turn) in &ordered {
        totals.push(turn);
        by_agent
            .entry((turn.agent_id.as_str(), turn.model.as_str()))
            .or_default()
            .push(turn);
        let code = match &turn.stop {
            TurnStop::Failed { code } => Some(code.as_str()),
            _ => None,
        };
        *by_stop.entry((stop_kind(&turn.stop), code)).or_insert(0) += 1;
        let s = by_session.entry(session_id.as_str()).or_insert((0, 0));
        s.0 += 1;
        s.1 = s.1.saturating_add(effective_of(turn));
    }

    let mut by_agent: Vec<AgentStats> = by_agent
        .into_iter()
        .map(|((agent_id, model), acc)| AgentStats {
            agent_id: agent_id.to_owned(),
            model: model.to_owned(),
            slice: acc.finish(),
        })
        .collect();
    by_agent.sort_by(|a, b| {
        b.slice
            .effective
            .cmp(&a.slice.effective)
            .then_with(|| a.agent_id.cmp(&b.agent_id))
            .then_with(|| a.model.cmp(&b.model))
    });

    let mut by_stop: Vec<StopCount> = by_stop
        .into_iter()
        .map(|((stop, code), count)| StopCount {
            stop: stop.to_owned(),
            code: code.map(str::to_owned),
            count,
        })
        .collect();
    by_stop.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.stop.cmp(&b.stop))
            .then_with(|| a.code.cmp(&b.code))
    });

    let sessions_meta: Vec<SessionStats> = sessions
        .iter()
        .map(|s| {
            let (turns, effective) = by_session.get(s.id.as_str()).copied().unwrap_or((0, 0));
            SessionStats {
                session_id: s.id.clone(),
                title: s.meta.title.clone(),
                forked_from: s.meta.parent_id.clone(),
                turns,
                effective,
            }
        })
        .collect();

    let series = match &scope {
        StatsScope::Session { .. } => {
            let dropped = ordered.len().saturating_sub(SERIES_LIMIT);
            let points = ordered
                .iter()
                .skip(dropped)
                .map(|(_, t)| SeriesPoint {
                    ts_ms: t.ts_ms,
                    agent_id: t.agent_id.clone(),
                    effective: effective_of(t),
                    prompt: t.prompt,
                    completion: t.completion,
                    stop: t.stop.clone(),
                })
                .collect();
            Some(StatsSeries {
                points,
                dropped: u32::try_from(dropped).unwrap_or(u32::MAX),
            })
        }
        StatsScope::All => None,
    };

    StatsReport {
        scope,
        scope_meta: StatsScopeMeta {
            recorded_since: ordered.first().map(|(_, t)| t.ts_ms),
            sessions: sessions_meta,
        },
        totals: totals.finish(),
        by_agent,
        by_stop,
        series,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_store::SessionMeta;

    fn turn(agent: &str, ts: u64, prompt: u64, cached: u64, completion: u64, stop: TurnStop) -> TurnRecord {
        TurnRecord {
            agent_id: agent.to_owned(),
            ts_ms: ts,
            hop: 0,
            rounds: 1,
            waves: 0,
            stop,
            prompt,
            cached,
            completion,
            reasoning: 0,
            cache_write: 0,
            cache_write_1h: 0,
            model: "m1".to_owned(),
            backend: "b".to_owned(),
            elapsed_ms: 100,
        }
    }

    fn session(id: &str, parent: Option<&str>) -> SessionSummary {
        SessionSummary {
            id: id.to_owned(),
            meta: SessionMeta {
                title: format!("題 {id}"),
                created_at: 0,
                updated_at: 0,
                parent_id: parent.map(str::to_owned),
                forked_at_seq: None,
                record_count: 0,
            },
        }
    }

    fn scope_s() -> StatsScope {
        StatsScope::Session {
            session_id: "s1".into(),
        }
    }

    /// 実効の重みは `budget.rs` の 1 実装と一致し、`reasoning` で二重に足さない。
    #[test]
    fn effective_matches_budget_weights_and_ignores_reasoning() {
        let mut t = turn("a", 1, 1_000, 200, 50, TurnStop::Completed);
        t.reasoning = 40; // completion の内数
        let report = aggregate(&[("s1".into(), t.clone())], &[session("s1", None)], scope_s());
        // (1000 − 200) ×1 + 200 ×0.1 + 50 ×4 = 800 + 20 + 200 = 1,020
        assert_eq!(report.totals.effective, 1_020);
        assert_eq!(
            report.totals.effective,
            crate::budget::effective_tokens(&Usage {
                prompt: 1_000,
                completion: 50,
                cache_read: 200,
                cache_write: 0,
                cache_write_1h: 0,
                reasoning: 40
            }),
            "budget.rs と同じ数字"
        );
        assert_eq!(report.totals.reasoning, 40, "reasoning は別欄で数える（足さない）");
    }

    /// 空入力: `recorded_since = None`、比は 0、平均は 0（0 除算しない）。
    #[test]
    fn empty_input_has_no_recorded_since_and_zero_ratios() {
        let report = aggregate(&[], &[session("s1", None)], scope_s());
        assert_eq!(report.scope_meta.recorded_since, None);
        assert_eq!(report.totals, StatsSlice::default());
        assert!(report.by_agent.is_empty());
        assert!(report.by_stop.is_empty());
        assert_eq!(report.scope_meta.sessions.len(), 1);
        assert_eq!(report.scope_meta.sessions[0].turns, 0);
        let series = report.series.expect("session スコープは series を持つ");
        assert!(series.points.is_empty());
        assert_eq!(series.dropped, 0);
    }

    /// `prompt = 0` でも `cache_rate = 0`（0 除算のガード）。
    #[test]
    fn cache_rate_is_zero_when_prompt_is_zero() {
        let t = turn("a", 1, 0, 0, 10, TurnStop::Completed);
        let report = aggregate(&[("s1".into(), t)], &[session("s1", None)], scope_s());
        assert_eq!(report.totals.cache_rate, 0.0);
        assert_eq!(report.totals.output_share, 1.0);
    }

    /// `by_stop` は `failed` を CODE ごとに分け、`failed` の数えは `is_failure` に従う。
    #[test]
    fn by_stop_splits_failed_by_code_and_counts_failures() {
        let turns = vec![
            ("s1".to_owned(), turn("a", 1, 1, 0, 1, TurnStop::Completed)),
            ("s1".to_owned(), turn("a", 2, 1, 0, 1, TurnStop::Failed { code: "X".into() })),
            ("s1".to_owned(), turn("a", 3, 1, 0, 1, TurnStop::Failed { code: "Y".into() })),
            ("s1".to_owned(), turn("a", 4, 1, 0, 1, TurnStop::Failed { code: "X".into() })),
            ("s1".to_owned(), turn("a", 5, 1, 0, 1, TurnStop::ReserveShort)),
            ("s1".to_owned(), turn("a", 6, 1, 0, 1, TurnStop::ToolLimit)),
        ];
        let report = aggregate(&turns, &[session("s1", None)], scope_s());
        assert_eq!(report.totals.turns, 6);
        assert_eq!(report.totals.failed, 4, "failed ×3 + reserve_short（tool_limit は完走）");
        let stops: Vec<(String, Option<String>, u64)> = report
            .by_stop
            .iter()
            .map(|s| (s.stop.clone(), s.code.clone(), s.count))
            .collect();
        assert_eq!(
            stops,
            vec![
                ("failed".into(), Some("X".into()), 2),
                ("completed".into(), None, 1),
                ("failed".into(), Some("Y".into()), 1),
                ("reserve_short".into(), None, 1),
                ("tool_limit".into(), None, 1),
            ],
            "件数の多い順、同点は種別名 → コード順"
        );
    }

    /// キャッシュ書き込みは**実効には 1.0× で入り、Slice には生の数で出る**（Spec 40）。
    ///
    /// **`effective` が書き込みで動かないこと**を同じテストで留める — D3 の裁定
    /// （重みを変えない）を機械で固定しないと、次に読む人が「請求に合わせる」
    /// 親切心で 2.0× を入れ、既存の村の天井の意味が黙って変わる。
    #[test]
    fn cache_write_is_reported_raw_and_never_reweighted_in_effective() {
        let mut with_write = turn("a", 1, 1_000, 200, 50, TurnStop::Completed);
        with_write.cache_write = 700;
        with_write.cache_write_1h = 700;
        let plain = turn("a", 1, 1_000, 200, 50, TurnStop::Completed);

        let w = aggregate(
            &[("s1".to_owned(), with_write)],
            &[session("s1", None)],
            scope_s(),
        );
        let p = aggregate(&[("s1".to_owned(), plain)], &[session("s1", None)], scope_s());

        assert_eq!(w.totals.cache_write, 700, "生の数はそのまま出る");
        assert_eq!(w.totals.cache_write_1h, 700);
        assert_eq!(p.totals.cache_write, 0);
        assert_eq!(
            w.totals.effective, p.totals.effective,
            "実効は書き込みで動かない（D3 — 重みは 1.0× のまま）"
        );
    }

    /// `by_agent` は **(個体, モデル) ごと**に分かれ、実効の多い順に並ぶ。
    ///
    /// 同じ個体がモデルを切り替えたら行が増える。畳むと、価格表を当てたときに
    /// **全ターンが最後のモデルの単価で計算されて金額が狂う**。
    #[test]
    fn by_agent_splits_by_model_and_orders_by_effective() {
        let on_m2 = |ts: u64| {
            let mut t = turn("a", ts, 10, 0, 0, TurnStop::Completed);
            t.model = "m2".into();
            t
        };
        let turns = vec![
            ("s1".to_owned(), turn("b", 1, 100, 0, 0, TurnStop::Completed)),
            ("s1".to_owned(), turn("a", 2, 10, 0, 0, TurnStop::Completed)),
            ("s1".to_owned(), on_m2(3)),
            ("s1".to_owned(), on_m2(4)),
        ];
        let report = aggregate(&turns, &[session("s1", None)], scope_s());
        let rows: Vec<(&str, &str, u64, u64)> = report
            .by_agent
            .iter()
            .map(|r| {
                (
                    r.agent_id.as_str(),
                    r.model.as_str(),
                    r.slice.turns,
                    r.slice.effective,
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![("b", "m1", 1, 100), ("a", "m2", 2, 20), ("a", "m1", 1, 10)],
            "a は m1 / m2 の 2 行へ割れ、並びは実効の多い順"
        );
        assert_eq!(report.by_agent[2].slice.avg_tokens_per_turn, 10);
        assert_eq!(
            report.totals.turns, 4,
            "割っても totals は変わらない（同じターンを 2 度数えない）"
        );
    }

    /// 実効が同点なら id 順、id も同じならモデル名順。
    ///
    /// 同じ個体の複数行が並ぶのは本 Spec で初めて起きるので、その中の順序を固定する。
    #[test]
    fn by_agent_breaks_ties_by_agent_then_model() {
        let with_model = |ts: u64, model: &str| {
            let mut t = turn("a", ts, 10, 0, 0, TurnStop::Completed);
            t.model = model.to_owned();
            t
        };
        let turns = vec![
            ("s1".to_owned(), with_model(1, "m2")),
            ("s1".to_owned(), with_model(2, "m0")),
            ("s1".to_owned(), turn("a", 3, 10, 0, 0, TurnStop::Completed)),
        ];
        let report = aggregate(&turns, &[session("s1", None)], scope_s());
        let models: Vec<&str> = report.by_agent.iter().map(|r| r.model.as_str()).collect();
        assert_eq!(models, vec!["m0", "m1", "m2"]);
    }

    /// `series` は開始時刻の昇順で末尾 N 件、落とした件数を数える。`all` では出さない。
    #[test]
    fn series_keeps_the_tail_and_counts_the_dropped() {
        let turns: Vec<(String, TurnRecord)> = (0..(SERIES_LIMIT as u64 + 7))
            .rev() // 逆順で渡しても中で並べる
            .map(|i| ("s1".to_owned(), turn("a", i, 1, 0, 0, TurnStop::Completed)))
            .collect();
        let report = aggregate(&turns, &[session("s1", None)], scope_s());
        let series = report.series.expect("session は series を持つ");
        assert_eq!(series.points.len(), SERIES_LIMIT);
        assert_eq!(series.dropped, 7);
        assert_eq!(series.points[0].ts_ms, 7, "落ちるのは古い側");
        assert_eq!(series.points.last().unwrap().ts_ms, SERIES_LIMIT as u64 + 6);
        assert_eq!(report.scope_meta.recorded_since, Some(0), "recorded_since は落とした側も含む最古");

        let all = aggregate(&turns, &[session("s1", None)], StatsScope::All);
        assert!(all.series.is_none(), "all では series を出さない");
    }

    /// `all`: 会話ごとの合計と `forked_from`。`turns` に無い会話も 0 で並ぶ。
    #[test]
    fn all_scope_lists_every_session_with_its_totals_and_fork_parent() {
        let turns = vec![
            ("s1".to_owned(), turn("a", 1, 10, 0, 0, TurnStop::Completed)),
            ("s2".to_owned(), turn("a", 2, 20, 0, 0, TurnStop::Completed)),
            ("s2".to_owned(), turn("b", 3, 30, 0, 0, TurnStop::Completed)),
        ];
        let sessions = vec![session("s2", Some("s1")), session("s1", None), session("s3", None)];
        let report = aggregate(&turns, &sessions, StatsScope::All);
        assert_eq!(report.totals.turns, 3);
        assert_eq!(report.totals.effective, 60);
        let rows: Vec<(String, Option<String>, u64, u64)> = report
            .scope_meta
            .sessions
            .iter()
            .map(|s| (s.session_id.clone(), s.forked_from.clone(), s.turns, s.effective))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("s2".into(), Some("s1".into()), 2, 50),
                ("s1".into(), None, 1, 10),
                ("s3".into(), None, 0, 0),
            ],
            "渡した並びのまま。分岐元は SessionMeta から"
        );
    }

    /// ワイヤ形: `scope` は kind タグ、`by_agent` は Slice を平坦に持つ、`code` は無ければ出ない。
    #[test]
    fn wire_shape_is_camel_case_and_flat() {
        let turns = vec![("s1".to_owned(), turn("a", 1, 10, 2, 3, TurnStop::Completed))];
        let report = aggregate(&turns, &[session("s1", None)], scope_s());
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["scope"]["kind"], "session");
        assert_eq!(json["scope"]["sessionId"], "s1");
        assert_eq!(json["scopeMeta"]["recordedSince"], 1);
        assert_eq!(json["byAgent"][0]["agentId"], "a");
        // (10 − 2) ×1 + 2 ×0.1 + 3 ×4 = 20.2 → 切り上げ 21（budget.rs の保守側の丸め）
        assert_eq!(json["byAgent"][0]["effective"], 21, "Slice の欄は平坦");
        assert_eq!(json["byStop"][0]["stop"], "completed");
        assert!(json["byStop"][0].get("code").is_none(), "code は無ければ出ない");
        assert_eq!(json["series"]["dropped"], 0);
        assert_eq!(json["series"]["points"][0]["stop"]["kind"], "completed");
        let all = serde_json::to_value(aggregate(&turns, &[], StatsScope::All)).unwrap();
        assert_eq!(all["scope"]["kind"], "all");
        assert!(all["series"].is_null());
    }
}
