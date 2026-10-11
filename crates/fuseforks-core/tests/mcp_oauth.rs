//! Spec 67 P1 の結合テスト — リモート MCP を OAuth の認可で繋ぐ（接続の時点）。
//!
//! ループバックに、保護リソースのメタデータ・認可サーバー（メタデータとトークン）・MCP サーバー
//! （Bearer を検査する initialize / tools/list）を 1 つで受ける最小の HTTP スタブを立て、**rmcp の
//! 実際の経路**（`AuthorizationManager` のメタデータ探索 → `get_access_token` → リフレッシュ →
//! Streamable HTTP の接続）を通す。スタブの水準は `mcp_http_secrecy.rs` と同じ（tokio の net）。
//!
//! 留めるもの:
//! - OS にリフレッシュトークンだけがある状態から、**トークン要求がちょうど 1 回**で繋がる
//!   （D4 — rmcp が「期限 0 の応答」を期限切れと読む、という依存をここで留める）
//! - 記録が無ければ `required` で、**スタブへの要求は 0 本**（D6 — ネットワークに出ない）
//! - 2 体が同じ接続先へ同時に繋いでもリフレッシュは 1 回（D5）
//! - リフレッシュが拒まれたら `required`、以後この実行では試みない（D6）
//! - PKCE を名乗らない認可サーバーでは進まない（D3）/ スコープ不足は `required`（D4）
//! - 記録が上限を超えると `token_too_large` で止まり、古い記録を壊さない（D4）

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use fuseforks_core::mcp::{McpConfig, McpManager};
use fuseforks_core::mcp_oauth::{
    client_key, read_token_record, token_key, write_token_record, ClientRecord, McpAuthState,
    McpOAuthRegistry, TokenRecord,
};
use fuseforks_core::{InMemorySecretStore, SecretStore};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// スタブの振る舞いと、受けた要求の記録。
#[derive(Default)]
struct StubState {
    /// 受理するリフレッシュトークン → 返す新しいリフレッシュトークン。
    accept: Vec<(String, String)>,
    /// 認可サーバーのメタデータに `code_challenge_methods_supported` を出すか。
    pkce: bool,
    /// 保護リソースのメタデータの `scopes_supported`。
    resource_scopes: Option<Vec<String>>,
    /// 発行したアクセストークン（MCP サーバーが受け付ける）。
    issued: BTreeSet<String>,
    /// トークン要求で受けたリフレッシュトークン（順に）。
    refreshes: Vec<String>,
    /// トークン要求で受けた `resource`。
    resources: Vec<String>,
    /// 全要求の数（パスを問わない）。
    hits: usize,
    /// MCP のエンドポイントへの要求の数。
    mcp_hits: usize,
}

type Shared = Arc<Mutex<StubState>>;

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> Option<Request> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(pos) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?.to_owned();
    let path = first.next()?.to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let length = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buffer[header_end..].to_vec();
    while body.len() < length {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    Some(Request {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

fn response(status: &str, content_type: &str, extra: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{body}",
        body.len()
    )
}

fn form_value(body: &str, key: &str) -> Option<String> {
    body.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| {
            url::form_urlencoded::parse(format!("x={v}").as_bytes())
                .next()
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default()
        })
    })
}

fn handle(state: &Shared, port: u16, request: &Request) -> String {
    let origin = format!("http://127.0.0.1:{port}");
    let mut s = state.lock().unwrap();
    s.hits += 1;
    // www.tradingview.com の実測（Spec 67 P4）— User-Agent の無い要求は 403。
    if !request
        .headers
        .iter()
        .any(|(k, v)| k == "user-agent" && v == "Fuseforks")
    {
        return response("403 Forbidden", "application/json", "", "{}");
    }
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", path) if path.starts_with("/.well-known/oauth-protected-resource") => {
            let mut body = serde_json::json!({
                "resource": format!("{origin}/mcp"),
                "authorization_servers": [origin.clone()],
            });
            if let Some(scopes) = &s.resource_scopes {
                body["scopes_supported"] = serde_json::json!(scopes);
            }
            response("200 OK", "application/json", "", &body.to_string())
        }
        ("GET", path) if path.starts_with("/.well-known/oauth-authorization-server") => {
            let mut body = serde_json::json!({
                "issuer": origin.clone(),
                "authorization_endpoint": format!("{origin}/authorize"),
                "token_endpoint": format!("{origin}/token"),
                "response_types_supported": ["code"],
                "grant_types_supported": ["authorization_code", "refresh_token"],
                "token_endpoint_auth_methods_supported": ["none"],
            });
            if s.pkce {
                body["code_challenge_methods_supported"] = serde_json::json!(["S256"]);
            }
            response("200 OK", "application/json", "", &body.to_string())
        }
        ("POST", "/token") => {
            let refresh = form_value(&request.body, "refresh_token").unwrap_or_default();
            s.refreshes.push(refresh.clone());
            s.resources.push(form_value(&request.body, "resource").unwrap_or_default());
            let next = s.accept.iter().find(|(rt, _)| rt == &refresh).map(|(_, next)| next.clone());
            match next {
                Some(next) => {
                    let access = format!("at-{}", s.refreshes.len());
                    s.issued.insert(access.clone());
                    let body = serde_json::json!({
                        "access_token": access,
                        "token_type": "Bearer",
                        "expires_in": 3600,
                        "refresh_token": next,
                    });
                    response("200 OK", "application/json", "", &body.to_string())
                }
                None => response(
                    "400 Bad Request",
                    "application/json",
                    "",
                    r#"{"error":"invalid_grant"}"#,
                ),
            }
        }
        (method, "/mcp") => {
            s.mcp_hits += 1;
            let bearer = request
                .headers
                .iter()
                .find(|(k, _)| k == "authorization")
                .and_then(|(_, v)| v.strip_prefix("Bearer "))
                .map(str::to_owned);
            if !bearer.is_some_and(|token| s.issued.contains(&token)) {
                let extra = format!(
                    "WWW-Authenticate: Bearer resource_metadata=\"{origin}/.well-known/oauth-protected-resource/mcp\"\r\n"
                );
                return response("401 Unauthorized", "application/json", &extra, "{}");
            }
            match method {
                "GET" => response("405 Method Not Allowed", "text/plain", "", ""),
                "DELETE" => response("200 OK", "text/plain", "", ""),
                _ => {
                    let message: serde_json::Value =
                        serde_json::from_str(&request.body).unwrap_or_default();
                    let id = message.get("id").cloned();
                    let method = message.get("method").and_then(|m| m.as_str()).unwrap_or("");
                    let Some(id) = id else {
                        return response("202 Accepted", "text/plain", "", "");
                    };
                    let result = match method {
                        "initialize" => serde_json::json!({
                            "protocolVersion": "2025-06-18",
                            "capabilities": { "tools": {} },
                            "serverInfo": { "name": "stub", "version": "0" },
                        }),
                        "tools/list" => serde_json::json!({
                            "tools": [{
                                "name": "ping",
                                "description": "返事をする",
                                "inputSchema": { "type": "object" },
                            }],
                        }),
                        _ => serde_json::json!({}),
                    };
                    let body = serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result });
                    response("200 OK", "application/json", "", &body.to_string())
                }
            }
        }
        _ => response("404 Not Found", "text/plain", "", ""),
    }
}

async fn spawn_stub(state: Shared) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("ループバックへ bind できること");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let state = Arc::clone(&state);
            tokio::spawn(async move {
                if let Some(request) = read_request(&mut stream).await {
                    let reply = handle(&state, port, &request);
                    let _ = stream.write_all(reply.as_bytes()).await;
                }
                let _ = stream.shutdown().await;
            });
        }
    });
    port
}

fn stub(accept: &[(&str, &str)]) -> Shared {
    Arc::new(Mutex::new(StubState {
        accept: accept.iter().map(|(a, b)| ((*a).to_owned(), (*b).to_owned())).collect(),
        pkce: true,
        ..Default::default()
    }))
}

fn oauth_config(port: u16) -> McpConfig {
    let raw = format!(
        r#"{{ "mcpServers": {{ "tv": {{
            "type": "http",
            "url": "http://127.0.0.1:{port}/mcp",
            "auth": "oauth"
        }} }} }}"#
    );
    serde_json::from_str(&raw).expect("受理されること")
}

/// OS に「認可済み」の記録を置く（P2 の認可の流れが書くものと同じ形）。
fn seed(secrets: &dyn SecretStore, port: u16, refresh_token: &str, scopes: &[&str]) {
    let resource = format!("http://127.0.0.1:{port}/mcp");
    let issuer = format!("http://127.0.0.1:{port}");
    write_token_record(
        secrets,
        &resource,
        &TokenRecord {
            issuer: issuer.clone(),
            refresh_token: refresh_token.to_owned(),
            scopes: scopes.iter().map(|s| (*s).to_owned()).collect(),
            received_at: 1,
        },
    )
    .unwrap();
    let client = ClientRecord {
        client_id: "client-1".to_owned(),
        redirect_uri: "http://127.0.0.1:53682/callback".to_owned(),
    };
    secrets
        .set(&client_key(&issuer), &serde_json::to_string(&client).unwrap())
        .unwrap();
}

#[tokio::test]
async fn a_saved_refresh_token_connects_with_exactly_one_refresh() {
    let state = stub(&[("rt-1", "rt-2")]);
    let port = spawn_stub(Arc::clone(&state)).await;
    let secrets = Arc::new(InMemorySecretStore::new());
    seed(secrets.as_ref(), port, "rt-1", &[]);
    let registry = McpOAuthRegistry::new(secrets.clone());

    let manager = McpManager::connect_all(&oauth_config(port), secrets.as_ref(), &registry).await;
    let status = manager.statuses().first().unwrap().clone();
    assert!(status.connected, "繋がること: {:?}", status.error);
    assert_eq!(status.auth, McpAuthState::Authorized);
    assert!(status.tools.iter().any(|t| t.ends_with("ping")), "{:?}", status.tools);

    {
        let s = state.lock().unwrap();
        assert_eq!(s.refreshes, vec!["rt-1"], "トークン要求はちょうど 1 回");
        assert_eq!(
            s.resources,
            vec![format!("http://127.0.0.1:{port}/mcp")],
            "resource は正準形の URL（RFC 8707）"
        );
    }
    let record = read_token_record(secrets.as_ref(), &format!("http://127.0.0.1:{port}/mcp")).unwrap();
    assert_eq!(record.refresh_token, "rt-2", "回ったリフレッシュトークンが OS へ書き戻される");
    manager.shutdown().await;
}

#[tokio::test]
async fn without_a_record_the_entry_is_required_and_nothing_is_sent() {
    let state = stub(&[]);
    let port = spawn_stub(Arc::clone(&state)).await;
    let secrets = Arc::new(InMemorySecretStore::new());
    let registry = McpOAuthRegistry::new(secrets.clone());

    let manager = McpManager::connect_all(&oauth_config(port), secrets.as_ref(), &registry).await;
    let status = manager.statuses().first().unwrap().clone();
    assert!(!status.connected);
    assert_eq!(status.auth, McpAuthState::Required);
    let error = status.error.unwrap();
    assert!(error.contains("認可が必要"), "{error}");
    assert!(error.contains("ブラウザを開きません"), "{error}");
    assert_eq!(state.lock().unwrap().hits, 0, "ネットワークに出ない");
    manager.shutdown().await;
}

#[tokio::test]
async fn two_agents_on_the_same_server_refresh_once() {
    let state = stub(&[("rt-1", "rt-2")]);
    let port = spawn_stub(Arc::clone(&state)).await;
    let secrets = Arc::new(InMemorySecretStore::new());
    seed(secrets.as_ref(), port, "rt-1", &[]);
    let registry = McpOAuthRegistry::new(secrets.clone());
    let config = oauth_config(port);

    let (a, b) = tokio::join!(
        McpManager::connect_all(&config, secrets.as_ref(), &registry),
        McpManager::connect_all(&config, secrets.as_ref(), &registry),
    );
    assert!(a.statuses()[0].connected, "{:?}", a.statuses()[0].error);
    assert!(b.statuses()[0].connected, "{:?}", b.statuses()[0].error);
    assert_eq!(
        state.lock().unwrap().refreshes,
        vec!["rt-1"],
        "接続先ごとの管理者が 1 つなのでリフレッシュは 1 回"
    );
    a.shutdown().await;
    b.shutdown().await;
}

#[tokio::test]
async fn a_rejected_refresh_marks_the_server_required_for_this_run() {
    let state = stub(&[]);
    let port = spawn_stub(Arc::clone(&state)).await;
    let secrets = Arc::new(InMemorySecretStore::new());
    seed(secrets.as_ref(), port, "rt-revoked", &[]);
    let registry = McpOAuthRegistry::new(secrets.clone());
    let config = oauth_config(port);

    let first = McpManager::connect_all(&config, secrets.as_ref(), &registry).await;
    let status = first.statuses()[0].clone();
    assert!(!status.connected);
    assert_eq!(status.auth, McpAuthState::Required);
    assert!(status.error.unwrap().contains("認可が切れました"));
    assert_eq!(state.lock().unwrap().refreshes.len(), 1);
    first.shutdown().await;

    let second = McpManager::connect_all(&config, secrets.as_ref(), &registry).await;
    assert!(!second.statuses()[0].connected);
    assert_eq!(
        state.lock().unwrap().refreshes.len(),
        1,
        "この実行では拒まれた接続先へリフレッシュを繰り返さない"
    );
    second.shutdown().await;
}

#[tokio::test]
async fn an_authorization_server_without_pkce_is_refused() {
    let state = stub(&[("rt-1", "rt-2")]);
    state.lock().unwrap().pkce = false;
    let port = spawn_stub(Arc::clone(&state)).await;
    let secrets = Arc::new(InMemorySecretStore::new());
    seed(secrets.as_ref(), port, "rt-1", &[]);
    let registry = McpOAuthRegistry::new(secrets.clone());

    let manager = McpManager::connect_all(&oauth_config(port), secrets.as_ref(), &registry).await;
    let status = manager.statuses()[0].clone();
    assert!(!status.connected);
    assert!(status.error.unwrap().contains("PKCE"));
    assert!(state.lock().unwrap().refreshes.is_empty(), "トークン要求へ進まない");
    manager.shutdown().await;
}

#[tokio::test]
async fn missing_resource_scopes_make_the_server_required() {
    let state = stub(&[("rt-1", "rt-2")]);
    state.lock().unwrap().resource_scopes = Some(vec!["mcp:tools".to_owned()]);
    let port = spawn_stub(Arc::clone(&state)).await;
    let secrets = Arc::new(InMemorySecretStore::new());
    seed(secrets.as_ref(), port, "rt-1", &[]);
    let registry = McpOAuthRegistry::new(secrets.clone());

    let manager = McpManager::connect_all(&oauth_config(port), secrets.as_ref(), &registry).await;
    let status = manager.statuses()[0].clone();
    assert!(!status.connected);
    assert_eq!(status.auth, McpAuthState::Required);
    assert!(status.error.unwrap().contains("スコープ"));
    assert!(state.lock().unwrap().refreshes.is_empty(), "古いスコープのまま使い回さない");
    manager.shutdown().await;
}

#[tokio::test]
async fn a_record_over_the_limit_stops_with_token_too_large_and_keeps_the_old_record() {
    let huge = "r".repeat(2000);
    let state = stub(&[("rt-1", huge.as_str())]);
    let port = spawn_stub(Arc::clone(&state)).await;
    let secrets = Arc::new(InMemorySecretStore::new());
    seed(secrets.as_ref(), port, "rt-1", &[]);
    let registry = McpOAuthRegistry::new(secrets.clone());

    let manager = McpManager::connect_all(&oauth_config(port), secrets.as_ref(), &registry).await;
    let status = manager.statuses()[0].clone();
    assert!(!status.connected);
    let error = status.error.unwrap();
    assert!(error.contains("token_too_large"), "{error}");
    let record = read_token_record(secrets.as_ref(), &format!("http://127.0.0.1:{port}/mcp")).unwrap();
    assert_eq!(record.refresh_token, "rt-1", "上限を超えた記録で古い記録を壊さない");
    assert!(secrets.get(&token_key(&format!("http://127.0.0.1:{port}/mcp"))).unwrap().is_some());
    manager.shutdown().await;
}
