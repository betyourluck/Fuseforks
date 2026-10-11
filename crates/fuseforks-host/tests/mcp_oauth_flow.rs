//! Spec 67 P2 の結合テスト — 認可の流れ（受け口・`state` の照合・登録・コードの交換・取り消し）。
//!
//! ループバックに認可サーバーのスタブ（メタデータ / 認可口 / 動的登録 / トークン / 失効）を立て、
//! 「ブラウザ」は渡された認可 URL を辿って戻り先を叩く関数で代える。rmcp の実際の
//! `AuthorizationManager`（登録・PKCE・コードの交換）と、コアの二層の置き場を通す。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use fuseforks_core::mcp_oauth::{
    client_key, read_client_record, read_token_record, McpOAuthRegistry,
};
use fuseforks_core::{InMemorySecretStore, SecretStore};
use fuseforks_host::mcp_oauth_flow::{revoke, AuthorizeOutcome, McpOAuthFlows};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Default)]
struct StubState {
    /// 認可口が拒否（`error=access_denied`）で戻すか。
    deny: bool,
    /// 認可口が戻り先をポートまで照合するか（RFC 8252 に従わないサーバー）。
    strict_redirect: bool,
    /// メタデータに PKCE を出すか。
    pkce: bool,
    /// 登録した戻り先（登録順）。
    registered: Vec<String>,
    /// トークン要求の本文。
    token_bodies: Vec<String>,
    /// 失効で受けた token。
    revoked: Vec<String>,
    /// 認可口で受けた戻り先（順に。実際にどのポートで待ったかを見る）。
    authorize_redirects: Vec<String>,
}

type Shared = Arc<Mutex<StubState>>;

async fn read_request(stream: &mut tokio::net::TcpStream) -> Option<(String, String, String)> {
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
    let mut first = head.lines().next()?.split_whitespace();
    let method = first.next()?.to_owned();
    let target = first.next()?.to_owned();
    let length = head
        .lines()
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buffer[header_end..].to_vec();
    while body.len() < length {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    Some((method, target, String::from_utf8_lossy(&body).into_owned()))
}

fn reply(status: &str, extra: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{body}",
        body.len()
    )
}

fn params(raw: &str) -> Vec<(String, String)> {
    url::form_urlencoded::parse(raw.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

fn param(list: &[(String, String)], key: &str) -> Option<String> {
    list.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

fn handle(state: &Shared, port: u16, method: &str, target: &str, body: &str) -> String {
    let origin = format!("http://127.0.0.1:{port}");
    let mut s = state.lock().unwrap();
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    match (method, path) {
        ("GET", p) if p.starts_with("/.well-known/oauth-protected-resource") => reply(
            "200 OK",
            "",
            &serde_json::json!({ "resource": format!("{origin}/mcp"), "authorization_servers": [origin] })
                .to_string(),
        ),
        ("GET", p) if p.starts_with("/.well-known/oauth-authorization-server") => {
            let mut meta = serde_json::json!({
                "issuer": origin,
                "authorization_endpoint": format!("{origin}/authorize"),
                "token_endpoint": format!("{origin}/token"),
                "registration_endpoint": format!("{origin}/register"),
                "revocation_endpoint": format!("{origin}/revoke"),
                "response_types_supported": ["code"],
            });
            if s.pkce {
                meta["code_challenge_methods_supported"] = serde_json::json!(["S256"]);
            }
            reply("200 OK", "", &meta.to_string())
        }
        ("POST", "/mcp") => reply(
            "401 Unauthorized",
            &format!("WWW-Authenticate: Bearer resource_metadata=\"{origin}/.well-known/oauth-protected-resource/mcp\"\r\n"),
            "{}",
        ),
        ("POST", "/register") => {
            let request: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
            let redirect = request["redirect_uris"][0].as_str().unwrap_or("").to_owned();
            s.registered.push(redirect.clone());
            let client_id = format!("client-{}", s.registered.len());
            reply(
                "201 Created",
                "",
                &serde_json::json!({ "client_id": client_id, "redirect_uris": [redirect] }).to_string(),
            )
        }
        ("GET", "/authorize") => {
            let q = params(query);
            let redirect = param(&q, "redirect_uri").unwrap_or_default();
            let state_value = param(&q, "state").unwrap_or_default();
            s.authorize_redirects.push(redirect.clone());
            if s.strict_redirect && !s.registered.contains(&redirect) {
                return reply("400 Bad Request", "", "{}");
            }
            let location = if s.deny {
                format!("{redirect}?error=access_denied&state={state_value}")
            } else {
                format!("{redirect}?code=the-code&state={state_value}")
            };
            reply("302 Found", &format!("Location: {location}\r\n"), "")
        }
        ("POST", "/token") => {
            s.token_bodies.push(body.to_owned());
            let form = params(body);
            if param(&form, "grant_type").as_deref() == Some("authorization_code")
                && param(&form, "code").as_deref() != Some("the-code")
            {
                return reply("400 Bad Request", "", r#"{"error":"invalid_grant"}"#);
            }
            reply(
                "200 OK",
                "",
                &serde_json::json!({
                    "access_token": "at-1", "token_type": "Bearer", "expires_in": 3600, "refresh_token": "rt-issued"
                })
                .to_string(),
            )
        }
        ("POST", "/revoke") => {
            if let Some(token) = param(&params(body), "token") {
                s.revoked.push(token);
            }
            reply("200 OK", "", "{}")
        }
        _ => reply("404 Not Found", "", "{}"),
    }
}

async fn spawn_stub(state: Shared) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let state = Arc::clone(&state);
            tokio::spawn(async move {
                if let Some((method, target, body)) = read_request(&mut stream).await {
                    let answer = handle(&state, port, &method, &target, &body);
                    let _ = stream.write_all(answer.as_bytes()).await;
                }
                let _ = stream.shutdown().await;
            });
        }
    });
    port
}

fn stub() -> Shared {
    Arc::new(Mutex::new(StubState {
        pkce: true,
        ..Default::default()
    }))
}

/// 生の HTTP GET（リダイレクトは辿らない）。応答の状態行と Location を返す。
async fn raw_get(url: &str) -> (u16, Option<String>) {
    let parsed = url::Url::parse(url).unwrap();
    let port = parsed.port().unwrap();
    let target = match parsed.query() {
        Some(q) => format!("{}?{q}", parsed.path()),
        None => parsed.path().to_owned(),
    };
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let request = format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response).await;
    let status = response
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let location = response
        .lines()
        .find_map(|l| l.strip_prefix("Location: ").map(str::to_owned));
    (status, location)
}

/// ブラウザ役。認可 URL を開き、302 の Location（戻り先）を叩く。`before` があれば、同じ受け口へ
/// state の違う要求を先に送る。受け口が返した状態は `seen` に積む — **テスト本体で確かめる**
/// （spawn したタスクの中の assert は本体へ伝わらない）。
fn browser_seen(
    before: Option<&'static str>,
    seen: Arc<Mutex<Vec<u16>>>,
) -> impl Fn(&str) -> Result<(), String> + Send + Sync {
    move |auth_url: &str| {
        let auth_url = auth_url.to_owned();
        let seen = Arc::clone(&seen);
        tokio::spawn(async move {
            let (_, location) = raw_get(&auth_url).await;
            let Some(location) = location else {
                return;
            };
            if let Some(wrong) = before {
                let parsed = url::Url::parse(&location).unwrap();
                let bogus = format!(
                    "http://127.0.0.1:{}/callback?code=forged-code&state={wrong}",
                    parsed.port().unwrap()
                );
                let (status, _) = raw_get(&bogus).await;
                seen.lock().unwrap().push(status);
            }
            let (status, _) = raw_get(&location).await;
            seen.lock().unwrap().push(status);
        });
        Ok(())
    }
}

fn browser(before: Option<&'static str>) -> impl Fn(&str) -> Result<(), String> + Send + Sync {
    browser_seen(before, Arc::new(Mutex::new(Vec::new())))
}

fn idle(_: &str) -> Result<(), String> {
    Ok(())
}

struct Fixture {
    state: Shared,
    url: String,
    secrets: Arc<InMemorySecretStore>,
    registry: McpOAuthRegistry,
    flows: McpOAuthFlows,
}

async fn fixture() -> Fixture {
    let state = stub();
    let port = spawn_stub(Arc::clone(&state)).await;
    let secrets = Arc::new(InMemorySecretStore::new());
    Fixture {
        state,
        url: format!("http://127.0.0.1:{port}/mcp"),
        registry: McpOAuthRegistry::new(secrets.clone()),
        secrets,
        flows: McpOAuthFlows::new(),
    }
}

impl Fixture {
    async fn authorize(&self, open: &(dyn Fn(&str) -> Result<(), String> + Send + Sync), timeout: Duration) -> AuthorizeOutcome {
        self.flows
            .authorize("tv", &self.url, self.secrets.clone(), &self.registry, open, timeout)
            .await
    }

    fn issuer(&self) -> String {
        self.url.trim_end_matches("/mcp").to_owned()
    }
}

#[tokio::test]
async fn the_flow_registers_once_exchanges_the_code_and_saves_the_record() {
    let f = fixture().await;
    let outcome = f.authorize(&browser(None), Duration::from_secs(10)).await;
    assert_eq!(outcome, AuthorizeOutcome::Authorized);

    let record = read_token_record(f.secrets.as_ref(), &f.url).expect("記録が保存される");
    assert_eq!(record.refresh_token, "rt-issued");
    assert_eq!(record.issuer, f.issuer());
    let client = read_client_record(f.secrets.as_ref(), &f.issuer()).expect("登録の記録");
    assert_eq!(client.client_id, "client-1");
    {
        let s = f.state.lock().unwrap();
        assert_eq!(s.registered.len(), 1);
        let body = &s.token_bodies[0];
        assert!(body.contains("grant_type=authorization_code"), "{body}");
        assert!(body.contains("code_verifier="), "PKCE の verifier を送る: {body}");
        assert!(body.contains(&format!("resource={}", url::form_urlencoded::byte_serialize(f.url.as_bytes()).collect::<String>())), "{body}");
    }

    // 2 回目は登録を使い回し、登録したときのポートを先に試す（D2・D3）。
    let outcome = f.authorize(&browser(None), Duration::from_secs(10)).await;
    assert_eq!(outcome, AuthorizeOutcome::Authorized);
    let s = f.state.lock().unwrap();
    assert_eq!(s.registered.len(), 1, "登録は認可サーバーごとに 1 件");
    assert_eq!(
        s.authorize_redirects,
        vec![client.redirect_uri.clone(), client.redirect_uri.clone()],
        "2 回目も登録したときのポートで待った"
    );
}

#[tokio::test]
async fn a_callback_with_a_wrong_state_gets_400_and_the_wait_continues() {
    let f = fixture().await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let outcome = f
        .authorize(&browser_seen(Some("forged"), Arc::clone(&seen)), Duration::from_secs(10))
        .await;
    assert_eq!(outcome, AuthorizeOutcome::Authorized, "本物の戻りで完了する");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        *seen.lock().unwrap(),
        vec![400, 200],
        "state の違う戻りは 400、本物は 200（受け口は開いたまま待ち続けた）"
    );
}

#[tokio::test]
async fn cancelling_closes_the_listener_and_releases_the_port() {
    let f = Arc::new(fixture().await);
    let canceller = Arc::clone(&f);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        canceller.flows.cancel(&canceller.url);
    });
    let outcome = f.authorize(&idle, Duration::from_secs(10)).await;
    assert_eq!(outcome, AuthorizeOutcome::Cancelled);
    let redirect = read_client_record(f.secrets.as_ref(), &f.issuer()).unwrap().redirect_uri;
    let port = url::Url::parse(&redirect).unwrap().port().unwrap();
    tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("キャンセルの後はポートが解放されている");
    assert!(read_token_record(f.secrets.as_ref(), &f.url).is_none());
}

#[tokio::test]
async fn the_wait_times_out() {
    let f = fixture().await;
    let outcome = f.authorize(&idle, Duration::from_millis(300)).await;
    assert_eq!(outcome, AuthorizeOutcome::TimedOut);
}

#[tokio::test]
async fn a_denied_authorization_saves_nothing() {
    let f = fixture().await;
    f.state.lock().unwrap().deny = true;
    let outcome = f.authorize(&browser(None), Duration::from_secs(10)).await;
    assert_eq!(outcome, AuthorizeOutcome::Denied);
    assert!(read_token_record(f.secrets.as_ref(), &f.url).is_none());
    assert!(f.state.lock().unwrap().token_bodies.is_empty(), "コードの交換へ進まない");
}

#[tokio::test]
async fn a_strict_server_gets_one_reregistration_when_the_port_changes() {
    let f = fixture().await;
    assert_eq!(f.authorize(&browser(None), Duration::from_secs(10)).await, AuthorizeOutcome::Authorized);
    f.state.lock().unwrap().strict_redirect = true;
    // 登録したポートを塞ぎ、別のポートで受けさせる。
    let first = read_client_record(f.secrets.as_ref(), &f.issuer()).unwrap();
    let port = url::Url::parse(&first.redirect_uri).unwrap().port().unwrap();
    let _hold = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.unwrap();

    let outcome = f.authorize(&browser(None), Duration::from_secs(10)).await;
    assert_eq!(outcome, AuthorizeOutcome::Authorized);
    assert_eq!(f.state.lock().unwrap().registered.len(), 2, "その認可の中で 1 回だけ登録し直す");
    let second = read_client_record(f.secrets.as_ref(), &f.issuer()).unwrap();
    assert_ne!(second.redirect_uri, first.redirect_uri);
}

#[tokio::test]
async fn an_authorization_server_without_pkce_is_refused_before_the_browser() {
    let f = fixture().await;
    f.state.lock().unwrap().pkce = false;
    let opened = Arc::new(Mutex::new(false));
    let flag = Arc::clone(&opened);
    let open = move |_: &str| {
        *flag.lock().unwrap() = true;
        Ok(())
    };
    let outcome = f.authorize(&open, Duration::from_secs(10)).await;
    assert!(matches!(outcome, AuthorizeOutcome::Failed { message } if message.contains("PKCE")));
    assert!(!*opened.lock().unwrap(), "ブラウザを開かない");
    assert!(f.secrets.get(&client_key(&f.issuer())).unwrap().is_none(), "登録もしない");
}

#[tokio::test]
async fn revoking_clears_the_record_and_asks_the_server() {
    let f = fixture().await;
    assert_eq!(f.authorize(&browser(None), Duration::from_secs(10)).await, AuthorizeOutcome::Authorized);
    let outcome = revoke("tv", &f.url, f.secrets.clone(), &f.registry).await;
    assert!(outcome.local_cleared);
    assert!(outcome.remote_revoked);
    assert!(read_token_record(f.secrets.as_ref(), &f.url).is_none());
    assert_eq!(f.state.lock().unwrap().revoked, vec!["rt-issued"]);
    assert_eq!(f.registry.state_of(&f.url), fuseforks_core::mcp_oauth::McpAuthState::Required);
}
