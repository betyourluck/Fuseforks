//! リモート MCP の OAuth の認可の流れ（Spec 67 P2。契約は `data_contract.yaml` の `mcp_oauth_contract`）。
//!
//! コア（`fuseforks_core::mcp_oauth`）が持つのは**接続の時点**の認可（保存したリフレッシュトークンから
//! アクセストークンを得る）。ここが持つのは、人が「認可する」を押してから記録が保存されるまで —
//! ループバックの受け口・ブラウザを開く・`state` の照合・コードの交換・クライアント登録 — と、取り消し。
//! **ブラウザを開くのは呼び出し側が渡す関数**（GUI は opener）で、この層は Tauri を知らない。
//!
//! # 受け口の閉じ方（D2）
//!
//! 受け口は `127.0.0.1` だけで待ち、次のどれかで閉じる: (a) パス `/callback` で `state` が一致する要求を
//! 1 本受けた（認可コードでも認可サーバーのエラーでも）(b) 待ちの上限を超えた (c) キャンセルされた
//! (d) 同じ接続先の認可をもう一度始めた。(a) に当たらない要求には 400 を返して待ち続ける。
//! どの経路でもリスナーは関数を抜けるときに drop され、ポートは解放される。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fuseforks_core::mcp_oauth::{
    canonical_resource, client_key, fetch_resource_scopes, read_client_record, read_token_record,
    select_scopes, ClientRecord, KeyringCredentialStore, McpOAuthRegistry,
};
use fuseforks_core::SecretStore;
use rmcp::transport::auth::{AuthorizationManager, AuthorizationMetadata, OAuthClientConfig};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

/// 待ちの上限の既定（D2 — 二段階認証を挟んでも足りる長さ）。
pub const AUTHORIZE_TIMEOUT: Duration = Duration::from_secs(600);

/// 動的登録の `client_name`（D3 — 端末名・利用者名を足さない）。
const CLIENT_NAME: &str = "Fuseforks";

/// メタデータの取得などの 1 回の HTTP の上限。
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

/// 認可の結末（IPC へそのまま出す）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AuthorizeOutcome {
    /// 記録を保存した。
    Authorized,
    /// 利用者がキャンセルした、または同じ接続先の認可がもう一度始まった。
    Cancelled,
    /// 待ちの上限を超えた。
    TimedOut,
    /// 認可サーバーが拒否を返した（利用者が承認しなかった等）。
    Denied,
    /// それ以外の失敗（相手の応答本文は載せない）。
    Failed {
        /// 分類の文。
        message: String,
    },
}

impl AuthorizeOutcome {
    /// 計器の `outcome=`（D10）。
    fn label(&self) -> &'static str {
        match self {
            Self::Authorized => "authorized",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timeout",
            Self::Denied => "denied",
            Self::Failed { .. } => "error",
        }
    }
}

/// 取り消しの結末。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevokeOutcome {
    /// 手元の記録は消した（常に真 — 失効の要求の成否に関わらず手元に残さない。D7）。
    pub local_cleared: bool,
    /// 認可サーバーへの失効の要求が通ったか（`revocation_endpoint` が無ければ偽）。
    pub remote_revoked: bool,
}

/// 進行中の認可（接続先ごとに 1 つ）。
#[derive(Default)]
pub struct McpOAuthFlows {
    inflight: Mutex<HashMap<String, (u64, CancellationToken)>>,
    next_id: AtomicU64,
}

impl McpOAuthFlows {
    /// 作る（GUI のプロセスに 1 つ）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 進行中の認可をキャンセルする（D2 の c）。無ければ何もしない。
    pub fn cancel(&self, url: &str) {
        let Some(resource) = canonical_resource(url) else {
            return;
        };
        if let Some((_, token)) = self.lock().remove(&resource) {
            token.cancel();
        }
    }

    /// 認可する（D7）。`open` は認可 URL をブラウザで開く関数。`server` は計器の名前。
    ///
    /// 戻るのは受け口が閉じたとき（認可・拒否・時間切れ・キャンセル）。成功したら `registry` の拒否の印を
    /// 外し、古い管理者を捨てる — 繋ぎ直しは呼び出し側（GUI）がオーケストレーターへ頼む。
    pub async fn authorize(
        &self,
        server: &str,
        url: &str,
        secrets: Arc<dyn SecretStore>,
        registry: &McpOAuthRegistry,
        open: &(dyn Fn(&str) -> Result<(), String> + Send + Sync),
        timeout: Duration,
    ) -> AuthorizeOutcome {
        let Some(resource) = canonical_resource(url) else {
            return failed("url を正準形にできません");
        };
        // (d) 同じ接続先の認可をもう一度始めた — 古い受け口を閉じてから新しく開く。
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let token = CancellationToken::new();
        if let Some((_, previous)) = self.lock().insert(resource.clone(), (id, token.clone())) {
            previous.cancel();
        }
        let outcome = run_authorize(server, url, &resource, secrets, &token, open, timeout).await;
        {
            let mut inflight = self.lock();
            if inflight.get(&resource).is_some_and(|(current, _)| *current == id) {
                inflight.remove(&resource);
            }
        }
        if outcome == AuthorizeOutcome::Authorized {
            registry.authorized(url).await;
        }
        fuseforks_core::note!(
            "mcp oauth: server={server} resource={resource} outcome={}",
            outcome.label()
        );
        outcome
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, (u64, CancellationToken)>> {
        self.inflight.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn failed(message: &str) -> AuthorizeOutcome {
    AuthorizeOutcome::Failed {
        message: message.to_owned(),
    }
}

/// 認可の本体。受け口は関数を抜けるときに drop されてポートが解放される。
async fn run_authorize(
    server: &str,
    url: &str,
    resource: &str,
    secrets: Arc<dyn SecretStore>,
    cancel: &CancellationToken,
    open: &(dyn Fn(&str) -> Result<(), String> + Send + Sync),
    timeout: Duration,
) -> AuthorizeOutcome {
    // 1. メタデータ。PKCE（S256）を名乗らなければ進まない（仕様の MUST — D3）。
    let mut manager = match fuseforks_core::mcp_oauth::new_manager(resource).await {
        Ok(manager) => manager,
        Err(err) => return failed(&fuseforks_core::mcp_oauth::auth_error_label(&err)),
    };
    let metadata = match tokio::time::timeout(HTTP_TIMEOUT, manager.discover_metadata()).await {
        Ok(Ok(metadata)) => metadata,
        Ok(Err(err)) => return failed(&fuseforks_core::mcp_oauth::auth_error_label(&err)),
        Err(_) => return failed("認可サーバーのメタデータが時間内に返りませんでした"),
    };
    if !metadata
        .code_challenge_methods_supported
        .as_ref()
        .is_some_and(|methods| methods.iter().any(|m| m == "S256"))
    {
        fuseforks_core::note!("mcp oauth: server={server} resource={resource} outcome=no_pkce");
        return failed("認可サーバーが PKCE（S256）に対応していないため、認可できません");
    }
    let issuer = issuer_of(&metadata);

    // 2. スコープ（D9）— ① 401 の scope → ② 保護リソースの scopes_supported → 無ければ送らない。
    let challenge_scope = probe_challenge_scope(url).await;
    let resource_scopes = fetch_resource_scopes(url).await;
    let offline = metadata
        .scopes_supported
        .as_ref()
        .is_some_and(|scopes| scopes.iter().any(|s| s == "offline_access"));
    let scopes = select_scopes(challenge_scope.as_deref(), resource_scopes.as_deref(), offline);
    let scope_refs: Vec<&str> = scopes.iter().map(String::as_str).collect();

    // 3. 受け口。登録したときのポートを先に試し、埋まっていれば空いているものを使う（D2）。
    let existing = read_client_record(secrets.as_ref(), &issuer);
    let preferred_port = existing
        .as_ref()
        .and_then(|record| url::Url::parse(&record.redirect_uri).ok())
        .and_then(|parsed| parsed.port());
    let listener = match bind_loopback(preferred_port).await {
        Ok(listener) => listener,
        Err(message) => return failed(&message),
    };
    let Ok(port) = listener.local_addr().map(|addr| addr.port()) else {
        return failed("受け口のポートを読めませんでした");
    };
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");

    // 4. クライアント登録（D3）。認可サーバーごとに 1 件を使い回す。
    manager.set_metadata(metadata.clone());
    let client_id = match existing {
        Some(record) => {
            if let Err(err) = manager.configure_client(OAuthClientConfig::new(
                record.client_id.clone(),
                redirect_uri.clone(),
            )) {
                return failed(&fuseforks_core::mcp_oauth::auth_error_label(&err));
            }
            record.client_id
        }
        None => match register(&mut manager, &secrets, &issuer, &redirect_uri, &scope_refs).await {
            Ok(client_id) => client_id,
            Err(outcome) => return outcome,
        },
    };
    let store = KeyringCredentialStore::new(
        Arc::clone(&secrets),
        resource.to_owned(),
        issuer.clone(),
        client_id.clone(),
    );
    manager.set_credential_store(store);

    // 5. 認可 URL。戻り先の照合がポートまで厳密なサーバーなら（RFC 8252 に従わない）、
    //    認可口が 400 を返すので、その認可の中で 1 回だけ登録し直す（D3）。
    let mut auth_url = match manager.get_authorization_url(&scope_refs).await {
        Ok(auth_url) => auth_url,
        Err(err) => return failed(&fuseforks_core::mcp_oauth::auth_error_label(&err)),
    };
    if preferred_port.is_some_and(|p| p != port) && authorize_rejects_redirect(&auth_url).await {
        if metadata.registration_endpoint.is_none() {
            fuseforks_core::note!("mcp oauth: server={server} resource={resource} outcome=no_registration");
            return failed("戻り先の照合が厳密な接続先ですが、動的登録が無いため登録し直せません");
        }
        fuseforks_core::note!("mcp oauth: server={server} resource={resource} outcome=reregistered");
        if let Err(outcome) =
            register(&mut manager, &secrets, &issuer, &redirect_uri, &scope_refs).await
        {
            return outcome;
        }
        auth_url = match manager.get_authorization_url(&scope_refs).await {
            Ok(auth_url) => auth_url,
            Err(err) => return failed(&fuseforks_core::mcp_oauth::auth_error_label(&err)),
        };
    }
    let Some(state) = query_value(&auth_url, "state") else {
        return failed("認可 URL に state がありません");
    };

    // 6. ブラウザを開き、戻りを待つ。
    if let Err(message) = open(&auth_url) {
        return failed(&format!("ブラウザを開けませんでした: {message}"));
    }
    let callback = tokio::select! {
        callback = wait_callback(&listener, &state) => callback,
        () = cancel.cancelled() => return AuthorizeOutcome::Cancelled,
        () = tokio::time::sleep(timeout) => return AuthorizeOutcome::TimedOut,
    };
    let (mut stream, params) = match callback {
        Ok(found) => found,
        Err(message) => return failed(&message),
    };
    if params.contains_key("error") {
        respond_page(&mut stream, false).await;
        return AuthorizeOutcome::Denied;
    }
    let Some(code) = params.get("code") else {
        respond_page(&mut stream, false).await;
        return failed("戻りに認可コードがありません");
    };

    // 7. コードの交換。保存は KeyringCredentialStore が行う（リフレッシュトークンを OS へ）。
    let exchanged = manager
        .exchange_code_for_token_with_issuer(code, &state, params.get("iss").map(String::as_str))
        .await;
    if let Err(err) = exchanged {
        respond_page(&mut stream, false).await;
        return failed(&fuseforks_core::mcp_oauth::auth_error_label(&err));
    }
    if read_token_record(secrets.as_ref(), resource).is_none() {
        respond_page(&mut stream, false).await;
        return failed(
            "認可サーバーがリフレッシュトークンを返さなかったため、認可を保存できません\
             （再起動のたびに認可し直すことになるので、保存しません）",
        );
    }
    respond_page(&mut stream, true).await;
    AuthorizeOutcome::Authorized
}

/// 動的登録し、記録を OS へ書く（D3）。
async fn register(
    manager: &mut AuthorizationManager,
    secrets: &Arc<dyn SecretStore>,
    issuer: &str,
    redirect_uri: &str,
    scopes: &[&str],
) -> Result<String, AuthorizeOutcome> {
    let config = manager
        .register_client(CLIENT_NAME, redirect_uri, scopes)
        .await
        .map_err(|err| match err {
            rmcp::transport::auth::AuthError::RegistrationFailed(_) => {
                failed("この接続先は動的登録を持たないため認可できません")
            }
            other => failed(&fuseforks_core::mcp_oauth::auth_error_label(&other)),
        })?;
    let record = ClientRecord {
        client_id: config.client_id.clone(),
        redirect_uri: redirect_uri.to_owned(),
    };
    let json = serde_json::to_string(&record).map_err(|err| failed(&err.to_string()))?;
    secrets
        .set(&client_key(issuer), &json)
        .map_err(|err| failed(&err.to_string()))?;
    Ok(config.client_id)
}

/// 認可サーバーの識別子。メタデータに `issuer` が無ければ認可口のオリジン。
fn issuer_of(metadata: &AuthorizationMetadata) -> String {
    metadata.issuer.clone().unwrap_or_else(|| {
        url::Url::parse(&metadata.authorization_endpoint)
            .map(|u| u.origin().ascii_serialization())
            .unwrap_or_else(|_| metadata.authorization_endpoint.clone())
    })
}

async fn bind_loopback(preferred: Option<u16>) -> Result<tokio::net::TcpListener, String> {
    if let Some(port) = preferred
        && let Ok(listener) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await
    {
        return Ok(listener);
    }
    tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|err| format!("受け口を開けませんでした: {err}"))
}

/// URL の query の値を 1 つ引く。
fn query_value(raw: &str, key: &str) -> Option<String> {
    let parsed = url::Url::parse(raw).ok()?;
    parsed
        .query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}

/// 認証なしの initialize を送り、401 の `WWW-Authenticate` の `scope` を拾う（D9 の ①）。取れなければ `None`。
async fn probe_challenge_scope(url: &str) -> Option<String> {
    let client = fuseforks_core::mcp_oauth::http_client(true).ok()?;
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fuseforks","version":"0"}}}"#;
    let response = client
        .post(url)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(body)
        .send()
        .await
        .ok()?;
    let header = response.headers().get("www-authenticate")?.to_str().ok()?.to_owned();
    challenge_scope(&header)
}

/// `WWW-Authenticate` の `scope="…"` を取り出す（純関数）。
pub fn challenge_scope(header: &str) -> Option<String> {
    let start = header.find("scope=\"")? + "scope=\"".len();
    let rest = &header[start..];
    let end = rest.find('"')?;
    let scope = rest[..end].trim();
    (!scope.is_empty()).then(|| scope.to_owned())
}

/// 認可口が戻り先を拒むか（ログインの手前で 400 を返すか。P0 の実測の形）。リダイレクトは辿らない。
async fn authorize_rejects_redirect(auth_url: &str) -> bool {
    let Ok(client) = fuseforks_core::mcp_oauth::http_client(false) else {
        return false;
    };
    match client.get(auth_url).send().await {
        Ok(response) => response.status().as_u16() == 400,
        Err(_) => false,
    }
}

/// 戻りを待つ。パス `/callback` で `state` が一致する要求だけを受け、ほかには 400 を返して待ち続ける。
async fn wait_callback(
    listener: &tokio::net::TcpListener,
    state: &str,
) -> Result<(tokio::net::TcpStream, HashMap<String, String>), String> {
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|err| format!("受け口が閉じました: {err}"))?;
        let Some(target) = read_request_target(&mut stream).await else {
            continue;
        };
        let parsed = url::Url::parse(&format!("http://127.0.0.1{target}")).ok();
        let matched = parsed.as_ref().and_then(|parsed| {
            if parsed.path() != "/callback" {
                return None;
            }
            let params: HashMap<String, String> = parsed
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            (params.get("state").map(String::as_str) == Some(state)).then_some(params)
        });
        match matched {
            Some(params) => return Ok((stream, params)),
            None => {
                let body = "Bad Request";
                let reply = format!(
                    "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        }
    }
}

/// 要求行の対象（`/callback?…`）を読む。読めなければ `None`。
async fn read_request_target(stream: &mut tokio::net::TcpStream) -> Option<String> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 2048];
    while !buffer.windows(4).any(|w| w == b"\r\n\r\n") && buffer.len() < 16 * 1024 {
        let read = tokio::time::timeout(HTTP_TIMEOUT, stream.read(&mut chunk))
            .await
            .ok()?
            .ok()?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let head = String::from_utf8_lossy(&buffer);
    let first = head.lines().next()?;
    let mut parts = first.split_whitespace();
    (parts.next()? == "GET").then_some(())?;
    parts.next().map(str::to_owned)
}

/// ブラウザへ返す最小のページ（外部の資源を読まない。認可サーバーの文は写さない — D2）。
async fn respond_page(stream: &mut tokio::net::TcpStream, ok: bool) {
    let message = if ok {
        "認可しました。このタブを閉じて Fuseforks に戻ってください。<br>Authorized. You can close this tab and return to Fuseforks."
    } else {
        "認可できませんでした。Fuseforks の MCP の設定で状態を確かめてください。<br>Authorization did not complete. Check the MCP settings in Fuseforks."
    };
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Fuseforks</title></head><body style=\"font-family:sans-serif;padding:2em\"><p>{message}</p></body></html>"
    );
    let reply = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(reply.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// 取り消す（D7）。順序: リフレッシュトークンを読む → 世代を進めて手元を消す（`registry`）→
/// （呼び出し側が接続を切る）→ `revocation_endpoint` があれば失効を要求。失効の失敗で手元は戻さない。
pub async fn revoke(
    server: &str,
    url: &str,
    secrets: Arc<dyn SecretStore>,
    registry: &McpOAuthRegistry,
) -> RevokeOutcome {
    let resource = canonical_resource(url).unwrap_or_else(|| "-".to_owned());
    let record = read_token_record(secrets.as_ref(), &resource);
    registry.revoke_local(url).await;
    let mut remote_revoked = false;
    if let Some(record) = record {
        remote_revoked = revoke_remote(&resource, &record.issuer, &record.refresh_token, &secrets).await;
    }
    fuseforks_core::note!(
        "mcp oauth: server={server} resource={resource} outcome=revoked remote={}",
        if remote_revoked { "yes" } else { "no" }
    );
    RevokeOutcome {
        local_cleared: true,
        remote_revoked,
    }
}

async fn revoke_remote(
    resource: &str,
    issuer: &str,
    refresh_token: &str,
    secrets: &Arc<dyn SecretStore>,
) -> bool {
    let Ok(manager) = fuseforks_core::mcp_oauth::new_manager(resource).await else {
        return false;
    };
    let Ok(Ok(metadata)) = tokio::time::timeout(HTTP_TIMEOUT, manager.discover_metadata()).await else {
        return false;
    };
    let Some(endpoint) = metadata
        .additional_fields
        .get("revocation_endpoint")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
    else {
        return false;
    };
    let client_id = read_client_record(secrets.as_ref(), issuer)
        .map(|record| record.client_id)
        .unwrap_or_default();
    let Ok(client) = fuseforks_core::mcp_oauth::http_client(true) else {
        return false;
    };
    let body = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("token", refresh_token)
        .append_pair("token_type_hint", "refresh_token")
        .append_pair("client_id", &client_id)
        .finish();
    client
        .post(&endpoint)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_challenge_scope_is_read_from_the_header() {
        assert_eq!(
            challenge_scope(r#"Bearer resource_metadata="https://x/.well-known", scope="files:read files:write""#),
            Some("files:read files:write".to_owned())
        );
        assert_eq!(challenge_scope(r#"Bearer resource_metadata="https://x""#), None);
        assert_eq!(challenge_scope(r#"Bearer scope="""#), None);
    }

    #[test]
    fn query_values_are_decoded() {
        assert_eq!(
            query_value("http://h/a?state=a%2Bb&x=1", "state"),
            Some("a+b".to_owned())
        );
        assert_eq!(query_value("http://h/a", "state"), None);
    }
}
