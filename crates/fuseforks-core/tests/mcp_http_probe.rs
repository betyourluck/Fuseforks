//! Spec 47 P0 の probe（使い捨て・`#[ignore]`）。
//!
//! 実物のリモート MCP サーバーへ rmcp の Streamable HTTP クライアントで
//! initialize → tools/list を通す。確認するのは 3 点:
//! (1) feature `transport-streamable-http-client-reqwest` で本当に繋がるか
//! (2) `Mcp-Session-Id` の往復が transport 内で完結するか（initialize の後の
//!     tools/list が通れば、セッションの継続はワーカーが持っている）
//! (3) 認証は `auth_header(素のトークン)` — reqwest の `bearer_auth` が
//!     `Bearer ` を付ける（実装読みの裏取り）
//!
//! 実行（トークンは環境変数で注入。スクリプトにもログにも値を残さない —
//! `failures.md` #71）:
//!
//! ```powershell
//! $env:ELYTH_TOKEN = "<トークン>"
//! cargo test -p fuseforks-core --test mcp_http_probe -- --ignored --nocapture
//! ```

use rmcp::ServiceExt;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

#[tokio::test]
#[ignore = "外部接続と実トークンが要る P0 probe"]
async fn remote_initialize_and_list_tools() {
    let url = std::env::var("ELYTH_URL")
        .unwrap_or_else(|_| "https://elythworld.com/api/mcp/remote".to_owned());
    let mut config = StreamableHttpClientTransportConfig::with_uri(url);
    match std::env::var("ELYTH_TOKEN") {
        // 素のトークンを渡す（Bearer は reqwest 側が付ける）。値は出力しない。
        Ok(token) => config = config.auth_header(token.trim().trim_start_matches("Bearer ")),
        Err(_) => eprintln!("ELYTH_TOKEN 未設定 — 認証なしで撃つ（401 の観測も probe の内）"),
    }

    // `from_config` は rmcp が内部で自前の reqwest(0.13) Client を作る。
    // **こちらから reqwest の型を名指ししない** — ワークスペースの reqwest は
    // 0.12（LLM クライアント）で、rmcp のは 0.13。型を名指しすると版が衝突する。
    let transport = StreamableHttpClientTransport::from_config(config);
    let service = ().serve(transport).await.expect("initialize が通ること");
    eprintln!("server info: {:?}", service.peer_info());

    let tools = service.list_all_tools().await.expect("tools/list が通ること");
    let names: Vec<_> = tools.iter().map(|tool| tool.name.clone()).collect();
    eprintln!("tools ({}): {names:?}", names.len());
    assert!(!names.is_empty(), "ツールが 1 本以上返ること");

    service.cancel().await.ok();
}
