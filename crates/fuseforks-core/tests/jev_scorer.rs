//! `JevScorer` の束ね・並列・合流を、HTTP 経路ごと踏む（Spec 59 P1）。
//!
//! # なぜスタブを立てるか
//!
//! `encode` / `decode` は純関数なので単体で留まるが、**束ねを何本に割り、
//! どの答えをどの段落へ戻し、一部だけ失敗したときに何を残すか**は
//! `ParagraphScorer::score` を通さないと分からない。ここが壊れると
//! **採点が 1 つ後ろの段落に付く**形の事故になり、型でも lint でも落ちない。
//!
//! 実鍵の live テストは `jev.rs` に `#[ignore]` で 1 本あるが、あれは
//! 「Jev が答えを返すか」しか見ない（1 束ね・成功だけ）。合流と部分失敗は
//! 実鍵では**狙って起こせない**ので、スタブの側が唯一の検証経路になる。
//!
//! # スタブの作り
//!
//! `attachment_fallback.rs` と同じ最小 HTTP/1.1。受け取った本文から
//! `input.state.chunks` の鍵を読み、方針に従って答えるか 402 を返す。

use std::sync::{Arc, Mutex};

use fuseforks_core::jev::{JevConfig, JevScorer, DEFAULT_MODEL};
use fuseforks_core::prune::{ParagraphScorer, ScoreError};

/// スタブの答え方。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Policy {
    /// 全部に答える。
    AnswerAll,
    /// `c0000` を含む束ねだけ 402（**再送しない失敗**なので待たずに済む）。
    RejectFirstBatch,
    /// 何を送っても 402。
    RejectEverything,
}

/// 受け取った本文の記録（何回・どの鍵で送ったか）。
type Bodies = Arc<Mutex<Vec<String>>>;

/// ループバックに Jev のスタブを立て、`(base_url, 記録)` を返す。
async fn spawn_stub(policy: Policy) -> (String, Bodies) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let bodies: Bodies = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&bodies);

    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let recorded = Arc::clone(&recorded);
            tokio::spawn(async move {
                let mut raw = Vec::new();
                let mut buf = [0u8; 8192];
                let body = loop {
                    let Ok(n) = socket.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&raw).into_owned();
                    let Some(head_end) = text.find("\r\n\r\n") else {
                        continue;
                    };
                    let len: usize = text[..head_end]
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case("content-length")
                                .then(|| v.trim().parse().ok())?
                        })
                        .unwrap_or(0);
                    let body = &text[head_end + 4..];
                    if body.len() >= len {
                        break body.to_owned();
                    }
                };
                recorded.lock().unwrap().push(body.clone());

                let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
                let keys: Vec<String> = parsed["input"]["state"]["chunks"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .cloned()
                    .collect();
                let reject = match policy {
                    Policy::AnswerAll => false,
                    Policy::RejectFirstBatch => keys.iter().any(|k| k == "c0000"),
                    Policy::RejectEverything => true,
                };

                let (status, payload) = if reject {
                    (
                        "402 Payment Required",
                        r#"{"success":false,"errors":[{"message":"Insufficient balance"}]}"#
                            .to_owned(),
                    )
                } else {
                    // 鍵ごとに **index から決まる値**を返す。どの答えがどの段落へ
                    // 戻ったかを、テスト側で計算して突き合わせられる。
                    let answers: Vec<String> = keys
                        .iter()
                        .map(|k| {
                            let i: usize = k.trim_start_matches('c').parse().unwrap();
                            let score = f64::from(u32::try_from(i).unwrap()) / 100.0;
                            format!(r#""{k}":{{"type":"noul","noul":{score}}}"#)
                        })
                        .collect();
                    (
                        "200 OK",
                        format!(
                            r#"{{"result":{{"result":{{"model":"jev-stub","answers":{{{}}},"usage":{{"input_tokens":{}}}}}}},"success":true}}"#,
                            answers.join(","),
                            keys.len()
                        ),
                    )
                };
                let resp = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = socket.write_all(resp.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });

    (format!("http://127.0.0.1:{port}"), bodies)
}

fn scorer(base_url: &str) -> JevScorer {
    JevScorer::new(JevConfig {
        account_id: "acct".into(),
        api_token: "token".into(),
        base_url: base_url.to_owned(),
        model: DEFAULT_MODEL.into(),
        threshold: 0.2,
    })
    .unwrap()
}

/// 20 段落。`BATCH_PARAGRAPHS` が 16 なので **16 + 4 の 2 束ね**に割れる。
fn twenty() -> Vec<String> {
    (0..20).map(|i| format!("段落 {i} の本文。")).collect()
}

/// 2 束ねに割れ、**答えが元の段落へ戻る**。
///
/// スコアは index から決まる値なので、1 つでもずれると気づける
/// （束ねの 2 本目は `c0016`〜`c0019` = 0.16〜0.19）。
#[tokio::test]
async fn two_batches_merge_back_into_the_original_order() {
    let (base_url, bodies) = spawn_stub(Policy::AnswerAll).await;
    let owned = twenty();
    let paragraphs: Vec<&str> = owned.iter().map(String::as_str).collect();

    let got = scorer(&base_url).score("依頼文", &paragraphs).await.unwrap();

    assert_eq!(got.scores.len(), 20);
    for (i, score) in got.scores.iter().enumerate() {
        let want = i as f32 / 100.0;
        let have = score.unwrap_or_else(|| panic!("段落 {i} が採点されていない"));
        assert!(
            (have - want).abs() < 1e-4,
            "段落 {i} の答えが {have}（期待 {want}）— 束ねの合流がずれている"
        );
    }
    assert_eq!(got.calls, 2, "16 + 4 の 2 束ね");
    assert_eq!(got.tokens, 20, "スタブは鍵の数を input_tokens で返す");
    assert_eq!(bodies.lock().unwrap().len(), 2);
}

/// **一部の束ねだけ失敗したら、その段落を残して続ける**（`outcome=ok`）。
///
/// 402 は再送しない分類なので、失敗した束ねも 1 呼び出しで終わる。
#[tokio::test]
async fn a_failed_batch_leaves_its_paragraphs_unscored() {
    let (base_url, _) = spawn_stub(Policy::RejectFirstBatch).await;
    let owned = twenty();
    let paragraphs: Vec<&str> = owned.iter().map(String::as_str).collect();

    let got = scorer(&base_url).score("依頼文", &paragraphs).await.unwrap();

    assert!(
        got.scores[..16].iter().all(Option::is_none),
        "落ちた束ねの段落は None（= 残す）"
    );
    assert!(
        got.scores[16..].iter().all(Option::is_some),
        "通った束ねの段落は採点される"
    );
    assert_eq!(got.calls, 2, "失敗した束ねも外へ出ているので数える");
}

/// **全部失敗したら `Failed`。** 「1 つも落ちなかった」と畳まない — 畳むと
/// ログの `outcome` が `all_dropped` になり、後から区別できない（#72 の規律）。
#[tokio::test]
async fn every_batch_failing_is_reported_as_a_failure() {
    let (base_url, _) = spawn_stub(Policy::RejectEverything).await;
    let owned = twenty();
    let paragraphs: Vec<&str> = owned.iter().map(String::as_str).collect();

    let err = scorer(&base_url)
        .score("依頼文", &paragraphs)
        .await
        .unwrap_err();

    assert!(matches!(err, ScoreError::Failed(_)), "{err:?}");
    assert_eq!(err.label(), "failed");
}

/// **採点する段落が 0 件なら 1 バイトも外へ出ない**（D10 / `calls=0`）。
#[tokio::test]
async fn nothing_is_sent_when_there_is_nothing_to_score() {
    let (base_url, bodies) = spawn_stub(Policy::AnswerAll).await;

    let got = scorer(&base_url).score("依頼文", &[]).await.unwrap();

    assert_eq!(got.calls, 0);
    assert!(got.scores.is_empty());
    assert!(
        bodies.lock().unwrap().is_empty(),
        "接続そのものが起きていない"
    );
}
