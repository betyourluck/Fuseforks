//! 扉のワイヤ側の結合テスト（Spec 25 P2）。
//!
//! **合鍵の検査は「実際に HTTP を話して」確かめる。** 述語の単体テストは
//! `mcp_server.rs` にあるが、それが**要求の経路に本当に挟まっているか**は
//! 層を組み立てないと分からない — middleware を張り忘れても述語のテストは緑のまま。
//!
//! ここではオーケストレーターを起こさず、`ask_concordia` の中身にも触れない。
//! 見るのは**扉の前で落ちるか通るか**だけ。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt as _;

/// 扉と同じ形（合鍵の層 + 内側）を組み立てる。
///
/// 内側は「通れば 200」を返すだけの替え玉。**本物の MCP サービスを載せない**のは、
/// ここで見たいのが認証層の有無であって MCP の会話ではないため
/// （載せると initialize のハンドシェイクが必要になり、何が落ちたのか読めなくなる）。
fn door(token: &str) -> axum::Router {
    axum::Router::new()
        .route("/mcp", axum::routing::any(|| async { "inside" }))
        .layer(axum::middleware::from_fn_with_state(
            Arc::new(token.to_owned()),
            fuseforks_lib::mcp_server::require_bearer_token,
        ))
}

async fn status_for(auth: Option<&str>) -> StatusCode {
    let mut request = Request::builder().uri("/mcp");
    if let Some(value) = auth {
        request = request.header(axum::http::header::AUTHORIZATION, value);
    }
    door("s3cret-token")
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

/// S4 — 合鍵が違う呼び出しは扉の前で落ちる。
#[tokio::test]
async fn wrong_or_missing_token_is_rejected_before_the_door() {
    assert_eq!(
        status_for(None).await,
        StatusCode::UNAUTHORIZED,
        "ヘッダが無い"
    );
    assert_eq!(
        status_for(Some("Bearer wrong-token")).await,
        StatusCode::UNAUTHORIZED,
        "鍵が違う"
    );
    assert_eq!(
        status_for(Some("Bearer s3cret")).await,
        StatusCode::UNAUTHORIZED,
        "**前方一致では通らない**"
    );
    assert_eq!(
        status_for(Some("s3cret-token")).await,
        StatusCode::UNAUTHORIZED,
        "`Bearer ` の無い裸の値は採らない"
    );
    assert_eq!(
        status_for(Some("Basic s3cret-token")).await,
        StatusCode::UNAUTHORIZED,
        "別の認証方式は通さない"
    );
}

/// 正しい合鍵なら内側へ届く。**拒否だけを固定すると、全部拒否する実装でも
/// 緑になる** — 通る側と対で見る。
#[tokio::test]
async fn correct_token_reaches_the_inside() {
    assert_eq!(status_for(Some("Bearer s3cret-token")).await, StatusCode::OK);
}
