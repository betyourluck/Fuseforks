//! CPU バウンド処理と、Tokio ↔ Rayon の橋渡し。
//!
//! # なぜ二つのランタイムを併用するのか
//!
//! エージェント本体は **I/O バウンド**（LLM への HTTP 待ち）なので Tokio が担当する。
//! Rayon のスレッドプールは物理コア数に固定されるため、そこでネットワークを待つと
//! 同時稼働エージェント数がコア数で頭打ちになる。
//!
//! 一方でログ集計は純粋な **CPU バウンド**で、これを Tokio のワーカー
//! スレッド上で回すと、その間 IPC コマンドもエージェントの応答処理も
//! 進まなくなる（UI が固まる）。
//!
//! そこで役割を割る:
//! - Tokio: エージェントのライフサイクル、メッセージ配送、LLM 呼び出し
//! - Rayon: 集計、その他の純計算
//!
//! （ベクトル類似検索は旧同梱 RAG の機構で、Spec 18 が索引を見出しへ
//! 置き換えた際に `rag.rs` ごと撤去した。）
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
/// # use fuseforks_core::compute::spawn_rayon;
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
        // Rayon 側で par_iter を回し、結果が oneshot 経由で戻ることを見る。
        let messages: Vec<AgentMessage> = (0..5_000)
            .map(|i| {
                let mut m = AgentMessage::new(
                    Endpoint::Agent {
                        id: AgentId::from("a"),
                    },
                    Endpoint::User,
                    "本文",
                    0,
                );
                m.tokens = u32::from(i % 2 == 0);
                m
            })
            .collect();

        let usage = spawn_rayon(move || aggregate_token_usage(&messages))
            .await
            .expect("Rayon 側が結果を返すこと");

        assert_eq!(usage.get(&AgentId::from("a")), Some(&2_500));
    }
}
