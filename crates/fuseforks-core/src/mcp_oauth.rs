//! リモート MCP の OAuth 2.1 の認可（Spec 67。契約は `data_contract.yaml` の `mcp_oauth_contract`）。
//!
//! この層が持つのは**接続の時点の認可**だけ — 保存したリフレッシュトークンからアクセストークンを得て、
//! rmcp の `AuthClient` を接続へ渡す。ブラウザを開いて認可コードを受け取る流れ（受け口・`state` の
//! 照合・コードの交換）はホスト層（Spec 67 P2）で、**ここからブラウザが開くことは無い**（D6）。
//!
//! # 置き場は二層（D4）
//!
//! OS の資格情報ストアには長命のもの（リフレッシュトークン・`issuer`・スコープ・時刻）だけを置き、
//! アクセストークンはメモリだけに持つ。rmcp の [`CredentialStore`] は完全なトークン応答を前提に
//! しているので、[`KeyringCredentialStore`] が二層を 1 つに見せる:
//!
//! - `load` — メモリに完全な記録があればそれを返す。無ければ OS の記録から「アクセストークン空・
//!   期限 0」の応答を組む。rmcp の `get_access_token` はこれを期限切れと読み、1 回リフレッシュする
//!   （起動直後の経路。`tests/mcp_oauth.rs` がこの読み方を留める）
//! - `save` — メモリを置き換え、OS はリフレッシュトークンかスコープが変わったときだけ書く
//! - `clear` — 両方を消し、世代を進める（D7。飛行中のリフレッシュの `save` を書き戻させない）
//!
//! # 接続先ごとに 1 つ（D5）
//!
//! [`McpOAuthRegistry`] が接続先（正準形の URL）ごとに `AuthClient` を 1 つだけ持ち、個体の接続は
//! その複製を使う。`AuthClient` の中身は `Arc<Mutex<AuthorizationManager>>` なので、リフレッシュは
//! 直列になる。**プロセスをまたぐ取り合い**（別の村を開いた 2 つのプロセス、GUI と CLI）は、`load` が
//! 毎回 OS の記録と照合して吸収する — 相手が回した新しいリフレッシュトークンを、自分がリフレッシュする前に
//! 拾う。照合は 1 回の HTTP 要求ごとに資格情報ストアを 1 回読む。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use oauth2::basic::BasicTokenType;
use oauth2::{AccessToken, RefreshToken, TokenResponse};
use rmcp::transport::auth::{
    AuthClient, AuthError, AuthorizationManager, CredentialStore, OAuthHttpClient,
    OAuthHttpClientError, OAuthHttpClientFuture, OAuthHttpRedirectPolicy, OAuthHttpRequest,
    OAuthTokenResponse, StoredCredentials, VendorExtraTokenFields,
};
use serde::{Deserialize, Serialize};

use crate::secret::SecretStore;

/// 資格情報ストアの鍵の接頭辞（トークンの記録。D4）。
const TOKEN_KEY_PREFIX: &str = "mcp-oauth:";
/// 資格情報ストアの鍵の接頭辞（クライアント登録の記録。D3）。
const CLIENT_KEY_PREFIX: &str = "mcp-oauth-client:";

/// Windows の資格情報マネージャーに入る値の上限（UTF-16 の単位。Spec 67 P0 実測 — `keyring` 3 は
/// 値を UTF-16 で保存し、上限 2,560 バイトがその単位で掛かる。ASCII でも日本語でも 1,281 字で失敗）。
/// 他の OS では上限が違うが、**村は OS をまたいで配られる**ので一番狭い値で揃える。
pub const CREDENTIAL_VALUE_LIMIT_UTF16: usize = 1280;

/// 認可サーバーのメタデータと保護リソースのメタデータを取りに行く上限。
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);

/// OAuth と MCP の HTTP 要求に付ける User-Agent（Spec 67 P4）。**User-Agent の無い要求を 403 で
/// 断る認可サーバーが実在する**（`www.tradingview.com` — curl の既定の UA なら 200、UA 無しは 403）。
/// rmcp の既定のクライアントは UA を付けないので、メタデータの取得が 403 で落ち、rmcp は黙って
/// `{MCP の origin}/authorize` という推測の値へ落ちる（PKCE も名乗らない）。版も端末も付けない —
/// 動的登録の `client_name` と同じ値で、識別子を増やさない。
pub const USER_AGENT: &str = "Fuseforks";

/// 上限が 1 MiB の応答本文（rmcp の既定と同じ）。
const MAX_OAUTH_BODY_BYTES: usize = 1024 * 1024;

/// User-Agent を付けた reqwest 0.13 のクライアント。こちらから直接撃つ要求（保護リソースの
/// メタデータ・401 の scope・認可口の事前確認・失効）と、MCP の接続（`AuthClient`）に使う。
///
/// # Errors
/// クライアントを組めないとき（TLS の初期化の失敗など）。
pub fn http_client(redirects: bool) -> Result<reqwest_rmcp::Client, String> {
    let mut builder = reqwest_rmcp::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(30));
    if !redirects {
        builder = builder.redirect(reqwest_rmcp::redirect::Policy::none());
    }
    builder.build().map_err(|err| err.to_string())
}

/// rmcp の認可の HTTP（探索・動的登録・コードの交換・リフレッシュ）を User-Agent 付きで通す。
/// rmcp の `ReqwestOAuthHttpClient` は外から組めない（`new` が非公開）ので、同じ形を写した。
struct UserAgentOAuthHttpClient {
    follow_redirects: reqwest_rmcp::Client,
    stop_redirects: reqwest_rmcp::Client,
}

impl OAuthHttpClient for UserAgentOAuthHttpClient {
    fn execute(&self, request: OAuthHttpRequest) -> OAuthHttpClientFuture<'_> {
        Box::pin(async move {
            let client = match request.redirect_policy {
                OAuthHttpRedirectPolicy::Stop => &self.stop_redirects,
                _ => &self.follow_redirects,
            };
            let request = reqwest_rmcp::Request::try_from(request.request)
                .map_err(|err| OAuthHttpClientError::new(err.to_string()))?;
            let response = client
                .execute(request)
                .await
                .map_err(|err| OAuthHttpClientError::new(err.to_string()))?;
            let mut builder = oauth2::http::Response::builder()
                .status(response.status())
                .version(response.version());
            for (name, value) in response.headers() {
                builder = builder.header(name, value);
            }
            let body = response
                .bytes()
                .await
                .map_err(|err| OAuthHttpClientError::new(err.to_string()))?;
            if body.len() > MAX_OAUTH_BODY_BYTES {
                return Err(OAuthHttpClientError::new(format!(
                    "OAuth HTTP response body exceeds {MAX_OAUTH_BODY_BYTES} bytes"
                )));
            }
            builder
                .body(body.to_vec())
                .map_err(|err| OAuthHttpClientError::new(err.to_string()))
        })
    }
}

/// User-Agent 付きの HTTP で `AuthorizationManager` を作る。**rmcp の `AuthorizationManager::new` を
/// 直接呼ばない**（UA の無いクライアントになる）。
///
/// # Errors
/// クライアントを組めないとき、または rmcp が URL を受け付けないとき。
pub async fn new_manager(resource: &str) -> Result<AuthorizationManager, AuthError> {
    let follow = http_client(true).map_err(AuthError::InternalError)?;
    let stop = http_client(false).map_err(AuthError::InternalError)?;
    AuthorizationManager::new_with_oauth_http_client(
        resource,
        Arc::new(UserAgentOAuthHttpClient {
            follow_redirects: follow,
            stop_redirects: stop,
        }),
    )
    .await
}

/// 接続先の状態（D6）。`McpServerStatus.auth` へ写る。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpAuthState {
    /// `auth: "oauth"` でないエントリ。
    #[default]
    None,
    /// OS の記録があり、要求するスコープを含む（アクセストークンの有無・期限は問わない）。
    Authorized,
    /// OS の記録が無い / スコープが足りない / リフレッシュが拒まれた。**接続を試みない。**
    Required,
}

/// MCP 仕様の正準形へ揃える（D4）: scheme と host を小文字（`url` が揃える）・既定のポートを落とす
/// （同）・fragment を落とす・末尾の `/` を落とす・query は残す。http / https 以外と、host の無い
/// URL は `None`。
///
/// **鍵と、認可サーバーへ送る `resource` の両方がこの値。** 設定の `url`（MCP のエンドポイント）を
/// 揃えたもので、保護リソースのメタデータの `resource` ではない — 鍵を引くのにメタデータを取りに行かずに
/// 済み、MCP 仕様の「MCP サーバーを特定する最も具体的な URI」にも当たる（Spec 67 P1 実装記録）。
pub fn canonical_resource(raw: &str) -> Option<String> {
    let url = url::Url::parse(raw.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?;
    let mut out = format!("{}://{}", url.scheme(), host);
    if let Some(port) = url.port() {
        out.push_str(&format!(":{port}"));
    }
    out.push_str(url.path().trim_end_matches('/'));
    if let Some(query) = url.query() {
        out.push('?');
        out.push_str(query);
    }
    Some(out)
}

/// トークンの記録の鍵（D4）。
pub fn token_key(resource: &str) -> String {
    format!("{TOKEN_KEY_PREFIX}{resource}")
}

/// クライアント登録の記録の鍵（D3）。
pub fn client_key(issuer: &str) -> String {
    format!("{CLIENT_KEY_PREFIX}{issuer}")
}

/// 要求するスコープを MCP 仕様の順で選ぶ（D9。純関数）:
/// ① 401 の `WWW-Authenticate` の `scope` → ② 保護リソースの `scopes_supported` を全部 →
/// どちらも無ければ空（`scope` パラメータを送らない）。**認可サーバーの `scopes_supported` は
/// 引数に取らない**（仕様の順位に無い — rmcp の `select_scopes` が ③ として使うので、それを呼ばない）。
///
/// 選んだ集合が空でなく、認可サーバーが `offline_access` を名乗っていれば足す（OIDC 系はこれが無いと
/// リフレッシュトークンを出さない）。**空のときは足さない** — 足すと `scope=offline_access` だけの
/// 要求になり、送らないときより狭くなる。
pub fn select_scopes(
    www_authenticate_scope: Option<&str>,
    resource_scopes: Option<&[String]>,
    as_offers_offline_access: bool,
) -> Vec<String> {
    let mut scopes: Vec<String> = match www_authenticate_scope.map(str::trim) {
        Some(scope) if !scope.is_empty() => scope.split_whitespace().map(str::to_owned).collect(),
        _ => resource_scopes.map(<[String]>::to_vec).unwrap_or_default(),
    };
    if as_offers_offline_access && !scopes.is_empty() && !scopes.iter().any(|s| s == "offline_access")
    {
        scopes.push("offline_access".to_owned());
    }
    scopes
}

/// 要求するスコープが、記録のスコープに全部含まれるか（D4。空の要求は常に含まれる）。
pub fn scopes_cover(required: &[String], granted: &[String]) -> bool {
    required.iter().all(|scope| granted.iter().any(|g| g == scope))
}

/// 資格情報ストアの値の上限に収まるか（[`CREDENTIAL_VALUE_LIMIT_UTF16`]）。
pub fn fits_credential_limit(value: &str) -> bool {
    value.encode_utf16().count() <= CREDENTIAL_VALUE_LIMIT_UTF16
}

/// OS に置くトークンの記録（D4）。**アクセストークンは持たない。**
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenRecord {
    /// 認可サーバー（クライアント登録の鍵を引く）。
    pub issuer: String,
    /// リフレッシュトークン。
    pub refresh_token: String,
    /// 付与されたスコープ（空 = `scope` を送らずに得た）。
    pub scopes: Vec<String>,
    /// 受け取った時刻（Unix 秒）。
    pub received_at: u64,
}

impl std::fmt::Debug for TokenRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenRecord")
            .field("issuer", &self.issuer)
            .field("refresh_token", &"[REDACTED]")
            .field("scopes", &self.scopes)
            .field("received_at", &self.received_at)
            .finish()
    }
}

/// OS に置くクライアント登録の記録（D3）。認可サーバーごとに 1 件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientRecord {
    /// 動的登録で得た `client_id`（公開クライアント — 秘密は持たない）。
    pub client_id: String,
    /// 登録した戻り先（ポートを含む 1 本。認可のときに先に試すポート）。
    pub redirect_uri: String,
}

/// 記録を読む。壊れた記録は無いものとして扱う（認可し直せば上書きされる）。
pub fn read_token_record(secrets: &dyn SecretStore, resource: &str) -> Option<TokenRecord> {
    let raw = secrets.get(&token_key(resource)).ok()??;
    serde_json::from_str(&raw).ok()
}

/// クライアント登録の記録を読む。
pub fn read_client_record(secrets: &dyn SecretStore, issuer: &str) -> Option<ClientRecord> {
    let raw = secrets.get(&client_key(issuer)).ok()??;
    serde_json::from_str(&raw).ok()
}

/// 記録を書く。上限を超えるなら書かずに `token_too_large`（D4 — 黙って落とさない）。
///
/// # Errors
/// 上限超え、または資格情報ストアへの書き込みの失敗。文面に値は載せない。
pub fn write_token_record(
    secrets: &dyn SecretStore,
    resource: &str,
    record: &TokenRecord,
) -> Result<(), String> {
    let json = serde_json::to_string(record).map_err(|err| err.to_string())?;
    if !fits_credential_limit(&json) {
        return Err(format!(
            "token_too_large: 認可の記録が資格情報ストアの上限（{CREDENTIAL_VALUE_LIMIT_UTF16} 字）を\
             超えました（{} 字）。この接続先のリフレッシュトークンは保存できません",
            json.encode_utf16().count()
        ));
    }
    secrets.set(&token_key(resource), &json).map_err(|err| err.to_string())
}

/// 現在時刻（Unix 秒）。
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// OS の記録から、rmcp に渡す「期限切れの」トークン応答を組む（D4 の起動の経路）。
fn expired_response(refresh_token: &str) -> OAuthTokenResponse {
    let mut response = OAuthTokenResponse::new(
        AccessToken::new(String::new()),
        BasicTokenType::Bearer,
        VendorExtraTokenFields::default(),
    );
    response.set_expires_in(Some(&Duration::ZERO));
    response.set_refresh_token(Some(RefreshToken::new(refresh_token.to_owned())));
    response
}

struct StoreInner {
    secrets: Arc<dyn SecretStore>,
    resource: String,
    issuer: String,
    client_id: String,
    /// 完全なトークン応答（アクセストークンを含む）。プロセスが終われば消える。
    memory: StdMutex<Option<StoredCredentials>>,
    /// 取り消しのたびに進む番号（D7）。
    generation: AtomicU64,
    /// 最後に `load` した時点の番号。`save` はこれが今の番号と違えば書かない。
    /// rmcp の manager は `Mutex` の中で load → refresh → save を続けて行うので、1 つの値で足りる。
    loaded_generation: AtomicU64,
}

/// rmcp の [`CredentialStore`] を、資格情報ストアとメモリの二層で実装する（D4）。
/// **接続先ごとに 1 つ**で、全接続が複製を共有する（中身は `Arc`）。
#[derive(Clone)]
pub struct KeyringCredentialStore {
    inner: Arc<StoreInner>,
}

impl KeyringCredentialStore {
    /// 作る。`issuer` と `client_id` は記録を書くときに使う。
    pub fn new(
        secrets: Arc<dyn SecretStore>,
        resource: impl Into<String>,
        issuer: impl Into<String>,
        client_id: impl Into<String>,
    ) -> Self {
        Self {
            inner: Arc::new(StoreInner {
                secrets,
                resource: resource.into(),
                issuer: issuer.into(),
                client_id: client_id.into(),
                memory: StdMutex::new(None),
                generation: AtomicU64::new(0),
                loaded_generation: AtomicU64::new(0),
            }),
        }
    }

    /// 今の世代（テストと計器のため）。
    pub fn generation(&self) -> u64 {
        self.inner.generation.load(Ordering::SeqCst)
    }

    /// 取り消し（D7 の 1 と 2）。世代を進めてからメモリと OS を消す — この順でないと、間に入った
    /// リフレッシュが消した後に書き戻しうる。
    pub fn revoke_local(&self) {
        self.inner.generation.fetch_add(1, Ordering::SeqCst);
        *self.memory() = None;
        let _ = self.inner.secrets.delete(&token_key(&self.inner.resource));
    }

    /// メモリにあるリフレッシュトークンと、OS の記録のリフレッシュトークンが違うか（別のプロセスが
    /// 回した）。接続の失敗のあとに 1 回やり直すかの判断に使う（D5）。
    pub fn os_has_other_refresh_token(&self) -> bool {
        let Some(os) = read_token_record(self.inner.secrets.as_ref(), &self.inner.resource) else {
            return false;
        };
        let memory = self.memory();
        memory
            .as_ref()
            .and_then(|stored| stored.token_response.as_ref())
            .and_then(|response| response.refresh_token())
            .is_some_and(|rt| rt.secret() != &os.refresh_token)
    }

    fn memory(&self) -> std::sync::MutexGuard<'_, Option<StoredCredentials>> {
        // 毒されても中身は使う（パニックした側が書きかけでも、次の load が OS と照合し直す）。
        self.inner.memory.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[async_trait]
impl CredentialStore for KeyringCredentialStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let inner = &self.inner;
        inner
            .loaded_generation
            .store(inner.generation.load(Ordering::SeqCst), Ordering::SeqCst);
        let Some(os) = read_token_record(inner.secrets.as_ref(), &inner.resource) else {
            // OS の記録が無い = 取り消された（別のプロセスでも）。メモリも捨てる。
            *self.memory() = None;
            return Ok(None);
        };
        let mut memory = self.memory();
        if let Some(stored) = memory.as_mut() {
            let same = stored
                .token_response
                .as_ref()
                .and_then(|response| response.refresh_token())
                .is_some_and(|rt| rt.secret() == &os.refresh_token);
            if !same {
                // 別のプロセスが回した。アクセストークンが生きていればそのまま使い、次の
                // リフレッシュは新しいリフレッシュトークンで行う（双方が回し合わないように）。
                if let Some(response) = stored.token_response.as_mut() {
                    response.set_refresh_token(Some(RefreshToken::new(os.refresh_token.clone())));
                }
                stored.granted_scopes = os.scopes.clone();
            }
            return Ok(Some(stored.clone()));
        }
        let stored = StoredCredentials::new(
            inner.client_id.clone(),
            Some(expired_response(&os.refresh_token)),
            os.scopes.clone(),
            Some(0),
        );
        *memory = Some(stored.clone());
        Ok(Some(stored))
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let inner = &self.inner;
        if inner.loaded_generation.load(Ordering::SeqCst) != inner.generation.load(Ordering::SeqCst)
        {
            // 取り消しの後に終わったリフレッシュ（D7）。書き戻さない。
            return Ok(());
        }
        let refresh_token = credentials
            .token_response
            .as_ref()
            .and_then(|response| response.refresh_token())
            .map(|rt| rt.secret().clone());
        if let Some(refresh_token) = refresh_token {
            let previous = read_token_record(inner.secrets.as_ref(), &inner.resource);
            let changed = previous.as_ref().is_none_or(|p| {
                p.refresh_token != refresh_token || p.scopes != credentials.granted_scopes
            });
            if changed {
                let record = TokenRecord {
                    issuer: inner.issuer.clone(),
                    refresh_token,
                    scopes: credentials.granted_scopes.clone(),
                    received_at: credentials.token_received_at.unwrap_or_else(now_secs),
                };
                write_token_record(inner.secrets.as_ref(), &inner.resource, &record)
                    .map_err(AuthError::InternalError)?;
                // 計器（D10）。サーバー名はこの層に無いので resource で名指す。値は書かない。
                crate::note!("mcp oauth: server=- resource={} outcome=refreshed", inner.resource);
            }
        }
        *self.memory() = Some(credentials);
        Ok(())
    }

    async fn clear(&self) -> Result<(), AuthError> {
        self.revoke_local();
        Ok(())
    }
}

/// [`McpOAuthRegistry::client_for`] の失敗。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OAuthConnectError {
    /// 認可が要る（記録なし・スコープ不足・この実行で拒まれた）。接続は試みない。
    Required(&'static str),
    /// 認可サーバーが PKCE（S256）を名乗らない（仕様の MUST — 進まない）。
    NoPkce,
    /// メタデータの取得などの失敗（相手の応答本文は載せない）。
    Failed(String),
}

impl OAuthConnectError {
    /// 計器の `outcome=`（D10）。
    pub fn outcome(&self) -> &'static str {
        match self {
            Self::Required(_) => "required",
            Self::NoPkce => "no_pkce",
            Self::Failed(_) => "error",
        }
    }
}

struct RegistryEntry {
    client: AuthClient<reqwest_rmcp::Client>,
    store: KeyringCredentialStore,
}

/// 接続先ごとの認可の管理者（D5）。プロセスに 1 つ。
pub struct McpOAuthRegistry {
    secrets: Arc<dyn SecretStore>,
    entries: tokio::sync::Mutex<HashMap<String, RegistryEntry>>,
    /// この実行で認可が拒まれた接続先（D6）。人が認可し直すまで接続を試みない —
    /// 個体が起動するたびに失敗するリフレッシュを繰り返さない。
    required: StdMutex<std::collections::HashSet<String>>,
}

impl McpOAuthRegistry {
    /// 作る。`secrets` は GUI なら資格情報ストア、CLI なら `--secrets` で選んだ置き場。
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            secrets,
            entries: tokio::sync::Mutex::new(HashMap::new()),
            required: StdMutex::new(std::collections::HashSet::new()),
        }
    }

    /// 接続先の状態（D6）。接続の有無と独立に、記録とこの実行の拒否から決まる。ネットワークを使わない。
    pub fn state_of(&self, url: &str) -> McpAuthState {
        let Some(resource) = canonical_resource(url) else {
            return McpAuthState::Required;
        };
        if self.required_set().contains(&resource) {
            return McpAuthState::Required;
        }
        if read_token_record(self.secrets.as_ref(), &resource).is_some() {
            McpAuthState::Authorized
        } else {
            McpAuthState::Required
        }
    }

    /// この実行で認可が拒まれたと記す（接続の失敗の分類から呼ぶ）。
    pub fn mark_required(&self, url: &str) {
        if let Some(resource) = canonical_resource(url) {
            self.required_set().insert(resource);
        }
    }

    /// 取り消し（D7 の 1・2）。共有している全複製が同じ世代を見るので、飛行中のリフレッシュは
    /// 書き戻されない。接続の切断（3）と失効の要求（4）は呼び出し側。
    pub async fn revoke_local(&self, url: &str) {
        let Some(resource) = canonical_resource(url) else {
            return;
        };
        if let Some(entry) = self.entries.lock().await.remove(&resource) {
            entry.store.revoke_local();
        } else {
            let _ = self.secrets.delete(&token_key(&resource));
        }
        self.required_set().insert(resource);
    }

    /// 認可が済んだ（P2 の流れから呼ぶ）。この実行の拒否の印を外し、古い管理者を捨てる —
    /// 次の接続は新しい記録から組み直す。
    pub async fn authorized(&self, url: &str) {
        if let Some(resource) = canonical_resource(url) {
            self.required_set().remove(&resource);
            self.entries.lock().await.remove(&resource);
        }
    }

    /// 接続先のメモリの記録を、別のプロセスが回したリフレッシュトークンで照合し直す必要があるか。
    pub async fn os_has_other_refresh_token(&self, url: &str) -> bool {
        let Some(resource) = canonical_resource(url) else {
            return false;
        };
        let entries = self.entries.lock().await;
        entries
            .get(&resource)
            .is_some_and(|entry| entry.store.os_has_other_refresh_token())
    }

    /// 接続に使う `AuthClient` を返す。接続先ごとに 1 つを作り、以後は複製を返す（D5）。
    ///
    /// 記録が無い・この実行で拒まれた接続先は、**ネットワークに出ずに** [`OAuthConnectError::Required`]。
    ///
    /// # Errors
    /// [`OAuthConnectError`] のとおり。
    pub async fn client_for(
        &self,
        url: &str,
    ) -> Result<AuthClient<reqwest_rmcp::Client>, OAuthConnectError> {
        let resource = canonical_resource(url)
            .ok_or_else(|| OAuthConnectError::Failed("url を正準形にできません".to_owned()))?;
        if self.required_set().contains(&resource) {
            return Err(OAuthConnectError::Required("rejected"));
        }
        let mut entries = self.entries.lock().await;
        if let Some(entry) = entries.get(&resource) {
            return Ok(entry.client.clone());
        }
        let record = read_token_record(self.secrets.as_ref(), &resource)
            .ok_or(OAuthConnectError::Required("no_record"))?;
        let client_record = read_client_record(self.secrets.as_ref(), &record.issuer)
            .ok_or(OAuthConnectError::Required("no_client"))?;

        let mut manager = new_manager(resource.as_str())
            .await
            .map_err(|err| OAuthConnectError::Failed(auth_error_label(&err)))?;
        let metadata = tokio::time::timeout(DISCOVERY_TIMEOUT, manager.discover_metadata())
            .await
            .map_err(|_| OAuthConnectError::Failed("認可サーバーのメタデータが時間内に返りませんでした".to_owned()))?
            .map_err(|err| OAuthConnectError::Failed(auth_error_label(&err)))?;
        if !metadata
            .code_challenge_methods_supported
            .as_ref()
            .is_some_and(|methods| methods.iter().any(|m| m == "S256"))
        {
            return Err(OAuthConnectError::NoPkce);
        }
        // スコープの照合（D4）— 保護リソースが名乗るスコープが記録に無ければ認可し直し。
        // 名乗らない（TradingView）ならここでは照合しない（① の WWW-Authenticate は接続の時点では無い）。
        if let Some(resource_scopes) = fetch_resource_scopes(url).await {
            let required = select_scopes(None, Some(&resource_scopes), false);
            if !scopes_cover(&required, &record.scopes) {
                return Err(OAuthConnectError::Required("scope"));
            }
        }
        manager.set_metadata(metadata);
        manager
            .configure_client_id(&client_record.client_id)
            .map_err(|err| OAuthConnectError::Failed(auth_error_label(&err)))?;
        let store = KeyringCredentialStore::new(
            Arc::clone(&self.secrets),
            resource.clone(),
            record.issuer.clone(),
            client_record.client_id.clone(),
        );
        manager.set_credential_store(store.clone());
        let http = http_client(true).map_err(OAuthConnectError::Failed)?;
        let client = AuthClient::new(http, manager);
        entries.insert(
            resource,
            RegistryEntry {
                client: client.clone(),
                store,
            },
        );
        Ok(client)
    }

    fn required_set(&self) -> std::sync::MutexGuard<'_, std::collections::HashSet<String>> {
        self.required.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// 保護リソースのメタデータの `scopes_supported` を取る（仕様の順: エンドポイントのパスを挟んだ
/// well-known → ルートの well-known）。取れない・名乗らないなら `None`。**失敗は照合しない側へ倒す** —
/// メタデータが一時的に取れないだけで認可済みの接続先を `required` にしない。
pub async fn fetch_resource_scopes(url: &str) -> Option<Vec<String>> {
    let parsed = url::Url::parse(url).ok()?;
    let origin = format!(
        "{}://{}{}",
        parsed.scheme(),
        parsed.host_str()?,
        parsed.port().map(|p| format!(":{p}")).unwrap_or_default()
    );
    let path = parsed.path().trim_end_matches('/');
    let mut candidates = Vec::new();
    if !path.is_empty() {
        candidates.push(format!("{origin}/.well-known/oauth-protected-resource{path}"));
    }
    candidates.push(format!("{origin}/.well-known/oauth-protected-resource"));
    let client = http_client(true).ok()?;
    for candidate in candidates {
        let Ok(Ok(response)) =
            tokio::time::timeout(DISCOVERY_TIMEOUT, client.get(&candidate).send()).await
        else {
            continue;
        };
        if !response.status().is_success() {
            continue;
        }
        let Ok(Ok(body)) = tokio::time::timeout(DISCOVERY_TIMEOUT, response.text()).await else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) else {
            continue;
        };
        let scopes: Vec<String> = value
            .get("scopes_supported")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|s| s.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        return (!scopes.is_empty()).then_some(scopes);
    }
    None
}

/// rmcp の認可のエラーを、相手の応答本文を含まない分類の文へ落とす（Spec 47 D7 と同じ線）。
pub fn auth_error_label(err: &AuthError) -> String {
    match err {
        AuthError::AuthorizationRequired => "認可が必要です".to_owned(),
        AuthError::TokenRefreshFailed(_) => "リフレッシュが拒まれました".to_owned(),
        AuthError::PkceUnsupported => "認可サーバーが PKCE（S256）に対応していません".to_owned(),
        AuthError::MetadataError(_) | AuthError::NoAuthorizationSupport => {
            "認可サーバーのメタデータを取得できませんでした".to_owned()
        }
        AuthError::HttpError(_) => "認可サーバーへ接続できませんでした".to_owned(),
        AuthError::InternalError(message) if message.starts_with("token_too_large") => message.clone(),
        other => format!("認可に失敗しました（{}）", auth_error_kind(other)),
    }
}

/// エラーの種類の名前だけ（本文は載せない）。
fn auth_error_kind(err: &AuthError) -> &'static str {
    match err {
        AuthError::AuthorizationRequired => "authorization_required",
        AuthError::AuthorizationFailed(_) => "authorization_failed",
        AuthError::TokenExchangeFailed(_) => "token_exchange_failed",
        AuthError::TokenRefreshFailed(_) => "token_refresh_failed",
        AuthError::HttpError(_) => "http",
        AuthError::OAuthError(_) => "oauth",
        AuthError::MetadataError(_) => "metadata",
        AuthError::PkceUnsupported => "pkce_unsupported",
        AuthError::UrlError(_) => "url",
        AuthError::NoAuthorizationSupport => "no_authorization_support",
        AuthError::InternalError(_) => "internal",
        AuthError::InvalidTokenType(_) => "invalid_token_type",
        AuthError::TokenExpired => "token_expired",
        AuthError::InvalidScope(_) => "invalid_scope",
        AuthError::RegistrationFailed(_) => "registration_failed",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret::InMemorySecretStore;

    #[test]
    fn canonical_form_folds_case_slash_port_and_fragment() {
        let a = canonical_resource("HTTPS://MCP.Example.com/mcp/").unwrap();
        let b = canonical_resource("https://mcp.example.com:443/mcp#frag").unwrap();
        assert_eq!(a, "https://mcp.example.com/mcp");
        assert_eq!(a, b, "同じ接続先は同じ鍵になる");
        assert_eq!(
            canonical_resource("https://mcp.example.com/").unwrap(),
            "https://mcp.example.com"
        );
        assert_eq!(
            canonical_resource("http://127.0.0.1:8080/mcp?x=1").unwrap(),
            "http://127.0.0.1:8080/mcp?x=1",
            "query は残す"
        );
        assert!(canonical_resource("ftp://example.com").is_none());
        assert!(canonical_resource("not a url").is_none());
    }

    #[test]
    fn scopes_follow_the_mcp_order_and_never_use_the_authorization_server() {
        let prm = vec!["files:read".to_owned()];
        assert_eq!(select_scopes(Some("a b"), Some(&prm), false), vec!["a", "b"]);
        assert_eq!(select_scopes(None, Some(&prm), false), vec!["files:read"]);
        assert!(select_scopes(None, None, false).is_empty(), "どちらも無ければ送らない");
        assert!(select_scopes(Some("  "), None, true).is_empty(), "空の scope は無いのと同じ");
        assert_eq!(
            select_scopes(None, Some(&prm), true),
            vec!["files:read", "offline_access"]
        );
        assert!(
            select_scopes(None, None, true).is_empty(),
            "空のときに offline_access だけを足さない"
        );
    }

    #[test]
    fn scope_coverage() {
        let granted = vec!["a".to_owned(), "b".to_owned()];
        assert!(scopes_cover(&[], &granted));
        assert!(scopes_cover(&["a".to_owned()], &granted));
        assert!(!scopes_cover(&["c".to_owned()], &granted));
    }

    #[test]
    fn credential_limit_counts_utf16_units() {
        assert!(fits_credential_limit(&"a".repeat(1280)));
        assert!(!fits_credential_limit(&"a".repeat(1281)));
        assert!(fits_credential_limit(&"あ".repeat(1280)), "UTF-8 のバイト数ではない");
        assert!(!fits_credential_limit(&"あ".repeat(1281)));
    }

    #[test]
    fn a_record_over_the_limit_is_refused_by_name_and_not_written() {
        let secrets = InMemorySecretStore::new();
        let record = TokenRecord {
            issuer: "https://as.example".to_owned(),
            refresh_token: "r".repeat(1300),
            scopes: Vec::new(),
            received_at: 1,
        };
        let err = write_token_record(&secrets, "https://mcp.example/mcp", &record).unwrap_err();
        assert!(err.starts_with("token_too_large"), "{err}");
        assert!(!err.contains("rrrr"), "値は文面に載せない: {err}");
        assert!(secrets.get(&token_key("https://mcp.example/mcp")).unwrap().is_none());
    }

    #[test]
    fn token_record_debug_redacts_the_refresh_token() {
        let record = TokenRecord {
            issuer: "i".to_owned(),
            refresh_token: "very-secret".to_owned(),
            scopes: Vec::new(),
            received_at: 0,
        };
        assert!(!format!("{record:?}").contains("very-secret"));
    }

    fn seeded_store() -> (Arc<InMemorySecretStore>, KeyringCredentialStore) {
        let secrets = Arc::new(InMemorySecretStore::new());
        let record = TokenRecord {
            issuer: "https://as.example".to_owned(),
            refresh_token: "rt-1".to_owned(),
            scopes: vec!["mcp:tools".to_owned()],
            received_at: 1,
        };
        write_token_record(secrets.as_ref(), "https://mcp.example/mcp", &record).unwrap();
        let store = KeyringCredentialStore::new(
            secrets.clone(),
            "https://mcp.example/mcp",
            "https://as.example",
            "client-1",
        );
        (secrets, store)
    }

    /// 起動の経路（D4）— OS にリフレッシュトークンだけがあるとき、`load` は期限 0 の応答を返す。
    #[tokio::test]
    async fn load_from_the_os_record_yields_an_expired_response_with_the_refresh_token() {
        let (_, store) = seeded_store();
        let stored = store.load().await.unwrap().expect("記録がある");
        let response = stored.token_response.expect("応答がある");
        assert_eq!(response.refresh_token().unwrap().secret(), "rt-1");
        assert_eq!(response.access_token().secret(), "");
        assert_eq!(response.expires_in(), Some(Duration::ZERO));
        assert_eq!(stored.token_received_at, Some(0));
        assert_eq!(stored.granted_scopes, vec!["mcp:tools"]);
    }

    /// 取り消し（D7）— load の後に取り消すと、飛行中だったリフレッシュの save は書き戻されない。
    #[tokio::test]
    async fn a_save_after_revocation_is_dropped() {
        let (secrets, store) = seeded_store();
        let stored = store.load().await.unwrap().unwrap();
        store.revoke_local();
        let mut refreshed = stored.clone();
        if let Some(response) = refreshed.token_response.as_mut() {
            response.set_refresh_token(Some(RefreshToken::new("rt-2".to_owned())));
        }
        store.save(refreshed).await.unwrap();
        assert!(
            secrets.get(&token_key("https://mcp.example/mcp")).unwrap().is_none(),
            "取り消した記録が書き戻された"
        );
        assert!(store.load().await.unwrap().is_none());
    }

    /// プロセスをまたぐ取り合い（D5）— 別のプロセスが OS の記録を回したら、次の load がそれを拾う。
    #[tokio::test]
    async fn load_adopts_a_refresh_token_rotated_by_another_process() {
        let (secrets, store) = seeded_store();
        let _ = store.load().await.unwrap();
        let rotated = TokenRecord {
            issuer: "https://as.example".to_owned(),
            refresh_token: "rt-from-other-process".to_owned(),
            scopes: vec!["mcp:tools".to_owned()],
            received_at: 2,
        };
        write_token_record(secrets.as_ref(), "https://mcp.example/mcp", &rotated).unwrap();
        assert!(store.os_has_other_refresh_token());
        let stored = store.load().await.unwrap().unwrap();
        assert_eq!(
            stored.token_response.unwrap().refresh_token().unwrap().secret(),
            "rt-from-other-process"
        );
        assert!(!store.os_has_other_refresh_token());
    }

    #[test]
    fn the_registry_reports_required_without_a_record_and_authorized_with_one() {
        let secrets = Arc::new(InMemorySecretStore::new());
        let registry = McpOAuthRegistry::new(secrets.clone());
        assert_eq!(registry.state_of("https://mcp.example/mcp"), McpAuthState::Required);
        let record = TokenRecord {
            issuer: "https://as.example".to_owned(),
            refresh_token: "rt".to_owned(),
            scopes: Vec::new(),
            received_at: 0,
        };
        write_token_record(secrets.as_ref(), "https://mcp.example/mcp", &record).unwrap();
        assert_eq!(registry.state_of("https://MCP.example/mcp/"), McpAuthState::Authorized);
        registry.mark_required("https://mcp.example/mcp");
        assert_eq!(registry.state_of("https://mcp.example/mcp"), McpAuthState::Required);
    }
}
