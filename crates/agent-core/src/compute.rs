//! CPU バウンド処理と、Tokio ↔ Rayon の橋渡し。
//!
//! # なぜ二つのランタイムを併用するのか
//!
//! エージェント本体は **I/O バウンド**（LLM への HTTP 待ち）なので Tokio が担当する。
//! Rayon のスレッドプールは物理コア数に固定されるため、そこでネットワークを待つと
//! 同時稼働エージェント数がコア数で頭打ちになる。
//!
//! 一方で RAG のベクトル検索やログ集計は純粋な **CPU バウンド**で、
//! これを Tokio のワーカースレッド上で回すと、その間 IPC コマンドも
//! エージェントの応答処理も進まなくなる（UI が固まる）。
//!
//! そこで役割を割る:
//! - Tokio: エージェントのライフサイクル、メッセージ配送、LLM 呼び出し
//! - Rayon: ベクトル類似度、集計、その他の純計算
//!
//! 橋渡しは [`spawn_rayon`] が担う。`oneshot` チャネルで結果を受け取るので、
//! **Tokio 側のスレッドはブロックされず**、Rayon 側も async を知らずに済む。

use std::collections::HashMap;

use rayon::prelude::*;

use crate::error::{CoreError, CoreResult};
use crate::model::{AgentId, AgentMessage, Endpoint};

/// CPU バウンドのクロージャを Rayon スレッドプールへ逃がし、結果を非同期に待つ。
///
/// `tokio::task::spawn_blocking` との違いは、逃がした先が **Rayon のプール**である点。
/// クロージャの中でさらに `par_iter` を使うと、そのプール内でネストした並列化が効く。
///
/// # Errors
/// Rayon ワーカーが結果を返す前に落ちた場合 [`CoreError::Compute`] を返す。
///
/// # Examples
/// ```
/// # use agent_core::compute::spawn_rayon;
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let sum = spawn_rayon(|| (1..=1000u64).sum::<u64>()).await.unwrap();
/// assert_eq!(sum, 500_500);
/// # }
/// ```
pub async fn spawn_rayon<F, T>(task: F) -> CoreResult<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();

    rayon::spawn(move || {
        // 受信側が既に落ちている場合（呼び出しがキャンセルされた等）は送信失敗を無視する。
        let _ = tx.send(task());
    });

    rx.await
        .map_err(|_| CoreError::Compute("Rayon ワーカーが結果を返す前に終了しました".to_owned()))
}

/// 2 本のベクトルのコサイン類似度。
///
/// 次元が異なる、またはどちらかがゼロベクトルの場合は 0.0 を返す。
/// ここでパニックさせると、壊れた 1 件のインデックスが検索全体を落とすことになる。
///
/// # 精度について
/// `f32` の仮数部は 24 ビットなので、成分の絶対値が 4000 を超えるあたりから
/// 二乗和の精度が落ちて順位が入れ替わりうる。埋め込みベクトルは正規化済みの
/// 小さい値で来る前提であり、生のスケールが大きい値を入れる用途では `f64` を検討すること。
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let (mut dot, mut norm_a, mut norm_b) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a.sqrt() * norm_b.sqrt())
}

/// 類似度つきの検索結果 1 件。
#[derive(Debug, Clone, PartialEq)]
pub struct Scored<T> {
    /// 元の要素。
    pub item: T,
    /// コサイン類似度。
    pub score: f32,
}

/// コーパス全体を走査して上位 `k` 件を返す（Rayon 並列）。
///
/// 件数が増えるほど並列化が効く。逐次実装との差が出るのは概ね数千件以上からで、
/// それ未満でも Rayon のワークスティーリングは小さいオーバーヘッドで済む。
///
/// # Arguments
/// * `query` - 問い合わせベクトル
/// * `corpus` - `(要素, 埋め込みベクトル)` の並び
/// * `k` - 返す件数
pub fn top_k_similar<T: Clone + Send + Sync>(
    query: &[f32],
    corpus: &[(T, Vec<f32>)],
    k: usize,
) -> Vec<Scored<T>> {
    if k == 0 || corpus.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<Scored<T>> = corpus
        .par_iter()
        .map(|(item, embedding)| Scored {
            item: item.clone(),
            score: cosine_similarity(query, embedding),
        })
        .collect();

    // 部分ソートで済ませる。全体ソートは k << n のとき無駄が大きい。
    let take = k.min(scored.len());
    scored.select_nth_unstable_by(take - 1, |a, b| {
        b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(take);
    scored.sort_unstable_by(|a, b| {
        b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)
    });
    scored
}

/// メッセージログをエージェント別のトークン消費量に畳み込む（Rayon 並列）。
///
/// 会話ログは放っておくと数万件になり、UI の統計パネルが要求するたびに
/// 逐次で舐めると描画が詰まる。map-reduce で分割集計する。
pub fn aggregate_token_usage(messages: &[AgentMessage]) -> HashMap<AgentId, u64> {
    messages
        .par_iter()
        .filter_map(|m| m.from.agent_id().map(|id| (id.clone(), u64::from(m.tokens))))
        .fold(HashMap::new, |mut acc: HashMap<AgentId, u64>, (id, tokens)| {
            *acc.entry(id).or_insert(0) += tokens;
            acc
        })
        .reduce(HashMap::new, |mut left, right| {
            for (id, tokens) in right {
                *left.entry(id).or_insert(0) += tokens;
            }
            left
        })
}

/// 発話元エンドポイント別の件数を数える（Rayon 並列）。
///
/// トポロジー描画で辺の太さを決めるのに使う。
pub fn count_by_sender(messages: &[AgentMessage]) -> HashMap<AgentId, u64> {
    messages
        .par_iter()
        .filter_map(|m| match &m.from {
            Endpoint::Agent { id } => Some(id.clone()),
            _ => None,
        })
        .fold(HashMap::new, |mut acc: HashMap<AgentId, u64>, id| {
            *acc.entry(id).or_insert(0) += 1;
            acc
        })
        .reduce(HashMap::new, |mut left, right| {
            for (id, count) in right {
                *left.entry(id).or_insert(0) += count;
            }
            left
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_handles_degenerate_inputs_without_panicking() {
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]), 1.0);
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 2.0]), 0.0, "次元不一致");
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 1.0]), 0.0, "ゼロベクトル");
    }

    #[test]
    fn top_k_returns_highest_scores_in_order() {
        let corpus = vec![
            ("遠い", vec![0.0, 1.0]),
            ("近い", vec![1.0, 0.0]),
            ("中間", vec![1.0, 1.0]),
        ];
        let hits = top_k_similar(&[1.0, 0.0], &corpus, 2);

        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].item, "近い");
        assert_eq!(hits[1].item, "中間");
        assert!(hits[0].score > hits[1].score);
    }

    #[test]
    fn top_k_clamps_to_corpus_size() {
        let corpus = vec![("only", vec![1.0])];
        assert_eq!(top_k_similar(&[1.0], &corpus, 10).len(), 1);
        assert!(top_k_similar(&[1.0], &corpus, 0).is_empty());
    }

    #[test]
    fn token_usage_is_grouped_by_sending_agent() {
        let mk = |from: &str, tokens: u32| {
            let mut m = AgentMessage::new(
                Endpoint::Agent {
                    id: AgentId::from(from),
                },
                Endpoint::User,
                "本文",
                0,
            );
            m.tokens = tokens;
            m
        };
        // ユーザー発話は集計対象外であることも同時に確かめる。
        let user_msg = AgentMessage::new(Endpoint::User, Endpoint::System, "指示", 0);
        let log = vec![mk("a", 10), mk("b", 3), mk("a", 5), user_msg];

        let usage = aggregate_token_usage(&log);
        assert_eq!(usage.get(&AgentId::from("a")), Some(&15));
        assert_eq!(usage.get(&AgentId::from("b")), Some(&3));
        assert_eq!(usage.len(), 2);

        let counts = count_by_sender(&log);
        assert_eq!(counts.get(&AgentId::from("a")), Some(&2));
    }

    #[tokio::test]
    async fn spawn_rayon_returns_the_computed_value() {
        // 直交する詰め物 5000 件に、類似度の順序が明確な 3 件を混ぜる。
        let mut corpus: Vec<(&str, Vec<f32>)> =
            (0..5_000).map(|_| ("filler", vec![0.0, 1.0])).collect();
        corpus.push(("near", vec![1.0, 0.0]));
        corpus.push(("mid", vec![1.0, 1.0]));
        corpus.push(("far", vec![1.0, 3.0]));

        let hits = spawn_rayon(move || top_k_similar(&[1.0, 0.0], &corpus, 3))
            .await
            .expect("Rayon 側が結果を返すこと");

        let items: Vec<&str> = hits.iter().map(|h| h.item).collect();
        assert_eq!(items, vec!["near", "mid", "far"]);
    }
}
