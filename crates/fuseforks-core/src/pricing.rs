//! モデルごとの単価から**おおよその金額**を出す純機構（Spec 41）。
//!
//! **[`crate::budget`] を呼ばない。** あちらは歯止めの重み（キャッシュ書き込みも
//! 1.0×）で、こちらは請求の推定。**同じ関数にすると、片方を直すともう片方が動く**
//! （Spec 40 D3 で分けたばかりの区別）。
//!
//! 掛ける相手は [`crate::stats::AgentStats`] の行 = **(個体, モデル) ごとの切片**。
//! **単価はモデルごとに違うので、行がモデルで割れていないと掛けられない**
//! （Spec 39 rev4 で `by_agent` の鍵を `(agentId, model)` にしたのが前提）。

use serde::{Deserialize, Serialize};

use crate::stats::{AgentStats, StatsSlice};

/// 100 万トークンあたりの単価（USD）。**未設定は「無い」であって 0 ではない。**
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rates {
    /// 入力。**これと [`Self::output`] が揃って初めて「単価登録済み」。**
    pub input: Option<f64>,
    /// 出力。思考ぶんもここで数える（`reasoning` は `completion` の内数）。
    pub output: Option<f64>,
    /// キャッシュ読み。未設定なら [`Self::input`] へ落ちる。
    pub cache_read: Option<f64>,
    /// キャッシュ書き込み。未設定なら [`Self::input`] へ落ちる。
    pub cache_write: Option<f64>,
    /// うち 1 時間 TTL。未設定なら [`Self::cache_write`] へ落ちる。
    pub cache_write_1h: Option<f64>,
}

/// 落とす規則を通したあとの単価。**全部埋まっている。**
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resolved {
    /// 入力。
    pub input: f64,
    /// 出力。
    pub output: f64,
    /// キャッシュ読み。
    pub cache_read: f64,
    /// キャッシュ書き込み（1 時間ぶんを除いた残り）。
    pub cache_write: f64,
    /// 1 時間 TTL のキャッシュ書き込み。
    pub cache_write_1h: f64,
}

/// **未設定は「無視」ではなく「1 段上へ落とす」**（Spec 41 D1。出自は otari）。
///
/// `cache_write_1h → cache_write → input` / `cache_read → input`。
/// **落ちたトークンは素の入力単価で数えられるので、表が不完全でも
/// 合計トークンは失われない。**
///
/// `input` と `output` が**両方**揃っていなければ [`None`] = 「単価未登録」。
/// **片方で他方を埋めない** — 出力を入力単価で埋めると桁が 5 倍違う嘘になる。
#[must_use]
pub fn resolve(r: &Rates) -> Option<Resolved> {
    let (input, output) = (r.input?, r.output?);
    let cache_write = r.cache_write.unwrap_or(input);
    Some(Resolved {
        input,
        output,
        cache_read: r.cache_read.unwrap_or(input),
        cache_write,
        cache_write_1h: r.cache_write_1h.unwrap_or(cache_write),
    })
}

/// 1 切片ぶんの金額（USD）。
///
/// **単価を先に解決してからトークンを掛ける**（Spec 41 D5）— 引き算とフォールバックの
/// 順序で結果が変わるため。引き算は **`saturating_sub` でクランプ**する:
/// `cache_write_1h <= cache_write` は decode で強制しておらず（Spec 40 が検定として
/// 残す判断をした）、不整合な記録なら負になりうる。**負の金額を出すより 0 へ倒す。**
///
/// `reasoning` は掛けない — `completion` の内数で、出力単価に既に乗っている。
#[must_use]
pub fn cost_of(slice: &StatsSlice, r: &Resolved) -> f64 {
    let fresh = slice
        .prompt
        .saturating_sub(slice.cached)
        .saturating_sub(slice.cache_write);
    let write_base = slice.cache_write.saturating_sub(slice.cache_write_1h);
    let per = |tokens: u64, rate: f64| (tokens as f64) * rate / 1_000_000.0;
    per(fresh, r.input)
        + per(slice.cached, r.cache_read)
        + per(write_base, r.cache_write)
        + per(slice.cache_write_1h, r.cache_write_1h)
        + per(slice.completion, r.output)
}

/// 金額の要約（画面の 2 行ぶん。Spec 41 D4）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostSummary {
    /// 単価が引けた行だけの合計（USD）。**近似**。
    pub total_usd: f64,
    /// 単価が引けた行数。
    pub priced_rows: u64,
    /// 全行数。**引けなかった行も数える** — これが無いと部分合計が全体に見える。
    pub total_rows: u64,
    /// **素のトークン**での被覆率の分子（`prompt + completion`）。
    ///
    /// **実効トークンで数えない** — 呼ぶと D5 の「混ぜない」を被覆率の側から破る。
    pub priced_tokens: u64,
    /// 被覆率の分母。**単価が無い行のトークンも数える。**
    pub total_tokens: u64,
    /// 使った単価のうち**最も古い**時点。**最新ではない** — 読み手が知りたいのは
    /// 「どれだけ古びているか」で、そこは**一番古い行が決める**。
    pub as_of: Option<String>,
}

impl CostSummary {
    /// 単価が 1 行も引けなければ、金額の行そのものを画面へ出さない（D4）。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.priced_rows == 0
    }
}

/// `by_agent` の行を、モデル名で単価を引きながら畳む。
///
/// `lookup` は**モデル名 → (単価, その単価の時点)**。引けない行は合計から外れ、
/// **外れたことが `priced_rows` / `priced_tokens` に出る** — これが無いと
/// **部分合計が全体の合計に見える**（Spec 39 D6 と同型）。
#[must_use]
pub fn summarize<F>(rows: &[AgentStats], lookup: F) -> CostSummary
where
    F: Fn(&str) -> Option<(Rates, Option<String>)>,
{
    let mut s = CostSummary {
        total_usd: 0.0,
        priced_rows: 0,
        total_rows: 0,
        priced_tokens: 0,
        total_tokens: 0,
        as_of: None,
    };
    for row in rows {
        let tokens = row.slice.prompt.saturating_add(row.slice.completion);
        s.total_rows += 1;
        s.total_tokens = s.total_tokens.saturating_add(tokens);
        let Some((rates, as_of)) = lookup(&row.model) else {
            continue;
        };
        let Some(resolved) = resolve(&rates) else {
            continue;
        };
        s.priced_rows += 1;
        s.priced_tokens = s.priced_tokens.saturating_add(tokens);
        s.total_usd += cost_of(&row.slice, &resolved);
        if let Some(d) = as_of {
            // 最も古い時点を採る。RFC3339 / YYYY-MM-DD は辞書順が時間順。
            s.as_of = Some(match s.as_of.take() {
                Some(cur) if cur <= d => cur,
                _ => d,
            });
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slice(prompt: u64, cached: u64, write: u64, write_1h: u64, completion: u64) -> StatsSlice {
        StatsSlice {
            turns: 1,
            failed: 0,
            prompt,
            cached,
            completion,
            reasoning: 0,
            cache_write: write,
            cache_write_1h: write_1h,
            effective: 0,
            cache_rate: 0.0,
            output_share: 0.0,
            avg_elapsed_ms: 0,
            avg_tokens_per_turn: 0,
        }
    }

    fn full() -> Rates {
        Rates {
            input: Some(2.0),
            output: Some(10.0),
            cache_read: Some(0.2),
            cache_write: Some(2.5),
            cache_write_1h: Some(4.0),
        }
    }

    fn row(model: &str, s: StatsSlice) -> AgentStats {
        AgentStats {
            agent_id: "a".to_owned(),
            model: model.to_owned(),
            slice: s,
        }
    }

    /// **未設定は無視ではなく 1 段上へ落ちる**（D1）。
    #[test]
    fn missing_rates_fall_up_one_step() {
        let bare = resolve(&Rates {
            input: Some(2.0),
            output: Some(10.0),
            ..Rates::default()
        })
        .expect("入力と出力があれば引ける");
        assert_eq!(bare.cache_read, 2.0, "読みが無ければ入力単価");
        assert_eq!(bare.cache_write, 2.0, "書き込みが無ければ入力単価");
        assert_eq!(bare.cache_write_1h, 2.0, "1h が無ければ書き込み → 入力");

        let stop = resolve(&Rates {
            cache_write_1h: None,
            ..full()
        })
        .unwrap();
        assert_eq!(stop.cache_write_1h, 2.5, "1h が無ければ書き込みで止まる");
    }

    /// 入力か出力が欠けたら「単価未登録」。**片方で埋めない** — 出力を入力単価で
    /// 埋めると桁が 5 倍違う嘘になる。
    #[test]
    fn a_half_filled_table_is_not_a_price() {
        assert!(resolve(&Rates {
            input: Some(2.0),
            ..Rates::default()
        })
        .is_none());
        assert!(resolve(&Rates {
            output: Some(10.0),
            ..Rates::default()
        })
        .is_none());
        assert!(resolve(&Rates::default()).is_none());
    }

    /// **Spec 40 P2 の実測をそのまま掛ける**（claude-sonnet-5 の 13 ラウンド）。
    ///
    /// **3 欄の表との差が本 Spec の存在理由**で、実測では 1.61 倍だった。
    #[test]
    fn the_measured_anthropic_rounds_cost_more_than_a_three_rate_table() {
        let s = slice(525_818, 455_850, 69_942, 69_942, 0);
        let five = cost_of(&s, &resolve(&full()).unwrap());
        let three = cost_of(
            &s,
            &resolve(&Rates {
                cache_write: None,
                cache_write_1h: None,
                ..full()
            })
            .unwrap(),
        );
        let ratio = five / three;
        assert!(
            (ratio - 1.61).abs() < 0.01,
            "5 欄 {five} / 3 欄 {three} = {ratio} 倍（実測 1.61）"
        );
    }

    /// 記録が不整合でも**負の金額を出さない**（D5 のクランプ）。
    #[test]
    fn a_broken_record_never_produces_a_negative_cost() {
        let s = slice(100, 90, 50, 80, 10);
        let cost = cost_of(&s, &resolve(&full()).unwrap());
        assert!(cost >= 0.0, "負になった: {cost}");
    }

    /// **引けなかった行は合計から外れ、外れたことが数に出る**（D4）。
    #[test]
    fn unpriced_rows_are_excluded_and_counted() {
        let rows = vec![
            row("priced", slice(1_000, 0, 0, 0, 100)),
            row("unknown", slice(4_000, 0, 0, 0, 0)),
        ];
        let s = summarize(&rows, |m| {
            (m == "priced").then(|| (full(), Some("2026-08-18".to_owned())))
        });
        assert_eq!((s.priced_rows, s.total_rows), (1, 2));
        assert_eq!((s.priced_tokens, s.total_tokens), (1_100, 5_100));
        assert!(!s.is_empty());
        assert!((s.total_usd - 0.003).abs() < 1e-9, "{}", s.total_usd);
    }

    /// 1 行も引けなければ画面に金額の行を出さない（0 と書かない）。
    #[test]
    fn a_village_without_any_price_reports_nothing() {
        let rows = vec![row("unknown", slice(1_000, 0, 0, 0, 100))];
        let s = summarize(&rows, |_| None);
        assert!(s.is_empty());
        assert_eq!(s.total_usd, 0.0);
        assert_eq!(s.total_tokens, 1_100, "被覆率の分母は引けなくても数える");
    }

    /// `as_of` は**最も古い**時点を採る。
    #[test]
    fn as_of_takes_the_oldest_price() {
        let rows = vec![
            row("new", slice(10, 0, 0, 0, 1)),
            row("old", slice(10, 0, 0, 0, 1)),
        ];
        let s = summarize(&rows, |m| {
            let d = if m == "old" { "2026-06-01" } else { "2026-08-18" };
            Some((full(), Some(d.to_owned())))
        });
        assert_eq!(s.as_of.as_deref(), Some("2026-06-01"));
    }

    /// **`pricing` は `budget` を呼ばない**（Spec 41 D5 の凍結を機械で留める）。
    ///
    /// 文章だけだと、次に読む人が「実効トークンがあるのだから使えばいい」と繋ぐ。
    /// **繋ぐと、歯止めの重みを直したときに金額が動く**。
    #[test]
    fn pricing_never_reaches_into_the_budget_weights() {
        // **実行時に読む**（村の作法 — `budget_reserve_wiring.rs` と同じ形）。
        // `include_str!` はコンパイル時に焼くので、ファイルを戻しても古い内容で
        // 判定が残りうる（実際に踏んだ）。
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/pricing.rs");
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
        let body = src.split("#[cfg(test)]").next().expect("テストより前の本体");
        // **コメントを落としてから見る。** この doc 自身が「budget を呼ばない」と
        // 書いており、素朴に含有を見ると自分の説明文で赤くなる（実際に踏んだ）。
        let code: String = body
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !code.contains("budget"),
            "pricing.rs の本体が budget を参照している（D5 の「混ぜない」が破れた）"
        );
    }
}
