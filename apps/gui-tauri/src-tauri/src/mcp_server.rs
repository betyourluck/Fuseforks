//! 外の LLM から村へ依頼を投げる扉（Spec 25 P2）。
//!
//! **サーバーは GUI プロセスの中に立つ。** 村は 1 つのプロセスの中にしか居らず
//! （`single_instance` / redb の単一書き手 / 稼働中の個体はメモリ）、別プロセスの
//! MCP サーバーを立てても村を持たない空の殻になる。だから扉は動いている GUI へ
//! 向かって開く（`mcp_server_contract`）。
//!
//! この層が持つのは**扉の開け閉めと合鍵の検査**だけで、オーケストレーションは
//! 1 つも増えない — 依頼はそのまま [`Orchestrator::ask_external`] へ渡る。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_core::Orchestrator;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager,
    tower::{StreamableHttpServerConfig, StreamableHttpService},
};
use rmcp::{ErrorData, Peer, RoleServer, schemars, tool, tool_handler, tool_router};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

/// 設定ファイルの名前（`{app_data_dir}/mcp_server.json`）。
///
/// **workspace の外**に置く（`mcp_server_contract` 凍結 2）。`world.json` へ入れると
/// 村を配ったときに合鍵まで一緒に配られ、`localStorage` へ入れると検査する側の
/// Rust から読めない。**この配置の帰結として「村を配っても扉は開かない」が
/// 構造で成立する**（配られるのは workspace だけ）。
pub const CONFIG_FILE: &str = "mcp_server.json";

/// MCP のエンドポイントのパス。
pub const MCP_PATH: &str = "/mcp";

/// 既定のポート。
///
/// 登録ポートを避けた高い番号を選ぶ。**既定で衝突すると「開いたのに繋がらない」の
/// 診断が難しくなる**（扉の不具合と他プロセスの占有が画面で同じに見える）。
pub const DEFAULT_PORT: u16 = 39641;

/// 扉の設定。`{app_data_dir}/mcp_server.json` の中身そのもの。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    /// 扉を開けるか。**既定は OFF**（`mcp_server_contract` 凍結 — 開けるのは人の操作）。
    #[serde(default)]
    pub enabled: bool,
    /// 待ち受けるポート。bind は常に 127.0.0.1。
    #[serde(default = "default_port")]
    pub port: u16,
    /// 合鍵。**ON にした時点で生成する**（起動時ではない — 凍結 3）。
    ///
    /// `None` = まだ一度も開けていない。毎起動で作り直さないのは、
    /// 作り直すとクライアント側の設定が毎回無効になるため。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_PORT,
            token: None,
        }
    }
}

impl McpServerConfig {
    /// 合鍵が無ければ作る。**ON にする操作からだけ呼ぶ。**
    ///
    /// 32 桁の 16 進（`Uuid::new_v4` の 122 bit）。人が読み上げて写す値ではなく、
    /// クライアントの設定ファイルへ貼る値なので、短さより桁を採る。
    pub fn ensure_token(&mut self) -> &str {
        self.token
            .get_or_insert_with(|| uuid::Uuid::new_v4().simple().to_string())
    }

    /// 合鍵を作り直す。**古い鍵で開いている接続は次の要求から弾かれる。**
    pub fn regenerate_token(&mut self) -> &str {
        self.token = Some(uuid::Uuid::new_v4().simple().to_string());
        self.token.as_deref().unwrap_or_default()
    }
}

/// 設定ファイルの読み書き。
///
/// **読めなかったら書き込みを拒む**（`schedules.json` と同じ規律）。既定値を
/// 書き戻すと、人が手で直せば戻ったはずの合鍵とポートを消すことになる —
/// 「安全側へ倒す」は読みの語彙で、**書きでは記録を捨てることを意味する**
/// （`failures.md` #70）。
#[derive(Debug)]
pub struct McpServerStore {
    path: PathBuf,
    /// 読み込みに失敗した理由。`Some` の間は書き込みを拒む。
    blocked: Option<String>,
    config: McpServerConfig,
}

impl McpServerStore {
    /// 設定を読み込む。**ファイルが無いのは失敗ではない**（既定で OFF）。
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(CONFIG_FILE);
        match std::fs::read_to_string(&path) {
            Ok(raw) => match serde_json::from_str::<McpServerConfig>(&raw) {
                Ok(config) => Self {
                    path,
                    blocked: None,
                    config,
                },
                Err(err) => {
                    // **値は出さない**（合鍵が入っているファイルなので、
                    // 読めなかった中身をログへ流さない）。
                    agent_core::note!(
                        "mcp server: {CONFIG_FILE} を読めませんでした。扉は開かず、設定の保存も拒みます（{err}）"
                    );
                    Self {
                        path,
                        blocked: Some(err.to_string()),
                        config: McpServerConfig::default(),
                    }
                }
            },
            // 未作成。初回はこれが正常。
            Err(_) => Self {
                path,
                blocked: None,
                config: McpServerConfig::default(),
            },
        }
    }

    /// 現在の設定。
    pub fn config(&self) -> &McpServerConfig {
        &self.config
    }

    /// 読み込みに失敗した理由（`Some` の間は保存できない）。
    pub fn blocked(&self) -> Option<&str> {
        self.blocked.as_deref()
    }

    /// 設定を差し替えて保存する。
    ///
    /// # Errors
    /// 読み込みに失敗している間、または書き込みに失敗した場合。
    pub fn save(&mut self, config: McpServerConfig) -> Result<(), String> {
        if let Some(reason) = &self.blocked {
            return Err(format!(
                "{CONFIG_FILE} が読めないため保存できません。ファイルを直すか削除してください（{reason}）"
            ));
        }
        let raw = serde_json::to_string_pretty(&config).map_err(|err| err.to_string())?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        std::fs::write(&self.path, raw).map_err(|err| err.to_string())?;
        restrict_permissions(&self.path);
        self.config = config;
        Ok(())
    }
}

/// 設定ファイルを本人だけが読める権限にする（Unix のみ）。
///
/// **Windows では何もしない。** `app_data_dir` は既に利用者ごとに分かれており、
/// ACL を自前で組むと「効いているつもりで効いていない」を作りやすい。
/// できないことをできたふりの処理で覆わない。
fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(err) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            agent_core::note!("mcp server: 設定ファイルの権限を絞れませんでした（{err}）");
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// 合鍵を定数時間で突き合わせる。
///
/// **効くのは別の利用者のプロセスに対してだけ** — 同じ利用者のプロセスは
/// 設定ファイルそのものを読めるので、そちらには何の防御にもならない。
/// 長さの違いで早期に返るのは、鍵の長さが固定なので情報を漏らさない。
fn token_matches(expected: &str, provided: &str) -> bool {
    let (a, b) = (expected.as_bytes(), provided.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// `Authorization: Bearer <token>` を取り出す。
fn bearer_token(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

/// 外の LLM へ提示するツール。**扉は 1 枚だけ**（`mcp_server_contract` 凍結 1）。
#[derive(Clone)]
struct ConcordiaTools {
    orchestrator: Arc<Orchestrator>,
    tool_router: ToolRouter<Self>,
}

/// `ask_concordia` の引数。**`message` だけ**（凍結 1）。
///
/// 宛先を受け取らないのは、村の状態が外からブラックボックスだから —
/// 呼ぶ側は誰が居るか知らないので宛先を選べない。窓口は村の側の設定で決まる。
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct AskParams {
    /// 村への依頼。自然言語で、必要な文脈を含めて 1 通に書く。
    message: String,
}

#[tool_router]
impl ConcordiaTools {
    fn new(orchestrator: Arc<Orchestrator>) -> Self {
        Self {
            orchestrator,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        name = "ask_concordia",
        description = "Concordia の村（複数の LLM サーヴァントによるオーケストレーション）へ依頼を 1 通送り、\
                       束ねられた答えを受け取る。窓口のサーヴァントが受け取り、必要に応じて他のサーヴァントへ\
                       分配・検証してから 1 つの答えにまとめる。\n\
                       単発の推論には向かない（自分で答えたほうが速く正確）。\
                       複数の視点・分担調査・相互検証が効く問いに使うこと。\n\
                       返答まで数分かかることがある。同時に処理できる依頼は 1 件だけ。"
    )]
    async fn ask_concordia(
        &self,
        Parameters(params): Parameters<AskParams>,
        peer: Peer<RoleServer>,
    ) -> Result<String, ErrorData> {
        // 名乗りは `clientInfo.name` の自己申告。**正規化はコア側**が
        // `normalize_client_name` で行う（プロンプトへ入る前の 1 箇所に置く）。
        let client = peer
            .peer_info()
            .map(|info| info.client_info.name.clone())
            .unwrap_or_default();

        self.orchestrator
            .ask_external(&client, &params.message)
            .await
            // **理由はツールが返す**（凍結 7）。窓口が未設定・停止中・処理中は
            // どれも「扉の故障」ではなく、呼ぶ側が次の手を決められる情報。
            .map_err(|err| ErrorData::internal_error(err.to_string(), None))
    }
}

// **`router = self.tool_router` を明示する。** マクロの既定は
// `Self::tool_router()` で、ツール呼び出しのたびにルーターを組み直す
// （提示は 1 本きりなので実害は小さいが、組んで持っているものを使わない形になる）。
#[tool_handler(router = self.tool_router)]
impl rmcp::ServerHandler for ConcordiaTools {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        let mut info = rmcp::model::ServerInfo::default();
        info.capabilities = rmcp::model::ServerCapabilities::builder().enable_tools().build();
        info.server_info.name = "Concordia".into();
        info.server_info.version = env!("CARGO_PKG_VERSION").into();
        info.instructions = Some(
            "Concordia はマルチエージェント・オーケストレーターです。\
             提示されるツールは 1 本だけで、村へ依頼を投げて答えを受け取ります。\
             村の顔ぶれや設定は外からは見えません。"
                .into(),
        );
        info
    }
}

/// 稼働中の扉。
#[derive(Debug)]
pub struct RunningServer {
    cancel: CancellationToken,
    /// 実際に待ち受けているポート（設定値と同じだが、実測を持っておく）。
    pub port: u16,
}

impl RunningServer {
    /// 扉を閉じる。**セッションもここで畳まれる**（rmcp の cancellation_token）。
    pub fn stop(self) {
        self.cancel.cancel();
        agent_core::note!("mcp server: 127.0.0.1:{} の扉を閉じました", self.port);
    }
}

/// 扉を開ける。
///
/// bind は **127.0.0.1 固定**（凍結 4）。トークン必須でも `0.0.0.0` で開けば
/// LAN へ露出する。Host / Origin の検査は rmcp の
/// [`StreamableHttpServerConfig`] が持つ（既定の `allowed_hosts` が loopback 3 種 =
/// DNS rebinding 対策）ので、こちらは **Origin を明示的に絞るだけ**。
///
/// # Errors
/// ポートを bind できない場合（他プロセスが使っている等）。
pub async fn start(
    orchestrator: Arc<Orchestrator>,
    port: u16,
    token: String,
) -> std::io::Result<RunningServer> {
    let cancel = CancellationToken::new();

    let mut config = StreamableHttpServerConfig::default();
    // **空のままだと Origin 検査が無効**（rmcp の既定）。ブラウザ経由の
    // 攻撃だけを落としたいので、loopback の origin だけを許す。
    // Origin ヘッダの無い要求（CLI クライアントはこちら）はそのまま通る。
    config.allowed_origins = vec![
        format!("http://127.0.0.1:{port}"),
        format!("http://localhost:{port}"),
    ];
    config.cancellation_token = cancel.clone();

    let service = StreamableHttpService::new(
        move || Ok(ConcordiaTools::new(Arc::clone(&orchestrator))),
        Arc::new(LocalSessionManager::default()),
        config,
    );

    let app = axum::Router::new()
        .nest_service(MCP_PATH, service)
        .layer(axum::middleware::from_fn_with_state(
            Arc::new(token),
            require_bearer_token,
        ));

    let listener =
        tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    let actual_port = listener.local_addr()?.port();
    let shutdown = cancel.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = axum::serve(listener, app)
            .with_graceful_shutdown(async move { shutdown.cancelled().await })
            .await
        {
            agent_core::note!("mcp server: 待ち受けが終了しました（{err}）");
        }
    });

    agent_core::note!("mcp server: 127.0.0.1:{actual_port}{MCP_PATH} で待ち受けます");
    Ok(RunningServer {
        cancel,
        port: actual_port,
    })
}

/// 設定と稼働状態をまとめて持つ管理役。
///
/// **ここが扉の唯一の所有者。** 設定ファイルは投影で、真実はこの中にある
/// （`schedules` と同じ形 — 書き手が 2 つあると片方の変更がもう片方に潰される）。
pub struct McpServerManager {
    store: McpServerStore,
    running: Option<RunningServer>,
    orchestrator: Arc<Orchestrator>,
}

/// 画面へ返す現在の状態（P3 の IPC がそのまま使う形）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpHostStatus {
    /// 設定上の ON / OFF。
    pub enabled: bool,
    /// 実際に待ち受けているか。**`enabled` と別に持つ** — ポートが埋まっていると
    /// 「ON なのに開いていない」が起こりうるので、1 つの真偽値に畳まない。
    pub listening: bool,
    /// 設定上のポート。
    pub port: u16,
    /// 合鍵。未生成なら `None`。
    pub token: Option<String>,
    /// 設定ファイルが読めない理由（`Some` の間は保存できない）。
    pub blocked: Option<String>,
    /// 直近の起動の失敗理由（ポート衝突など）。
    pub last_error: Option<String>,
}

impl McpServerManager {
    /// 設定を読み込むだけ。**扉はまだ開けない。**
    pub fn load(dir: &Path, orchestrator: Arc<Orchestrator>) -> Self {
        Self {
            store: McpServerStore::load(dir),
            running: None,
            orchestrator,
        }
    }

    /// 現在の状態。
    pub fn status(&self, last_error: Option<String>) -> McpHostStatus {
        McpHostStatus {
            enabled: self.store.config().enabled,
            listening: self.running.is_some(),
            port: self.store.config().port,
            token: self.store.config().token.clone(),
            blocked: self.store.blocked().map(str::to_owned),
            last_error,
        }
    }

    /// 設定が ON なら扉を開ける（起動時に 1 回呼ぶ）。
    ///
    /// **合鍵が無い ON は開けない** — 生成は ON にする操作の側の責務で、
    /// ここで作ると「起動のたびに鍵が変わる」に戻りうる（凍結 3）。
    /// 失敗の理由を返すが、**起動は止めない**（扉が開かないことはアプリが
    /// 動かない理由にならない）。
    pub async fn start_if_enabled(&mut self) -> Option<String> {
        let config = self.store.config();
        if !config.enabled {
            return None;
        }
        let Some(token) = config.token.clone() else {
            let reason = "合鍵が未生成のため扉を開きませんでした".to_owned();
            agent_core::note!("mcp server: {reason}");
            return Some(reason);
        };
        let port = config.port;
        match start(Arc::clone(&self.orchestrator), port, token).await {
            Ok(handle) => {
                self.running = Some(handle);
                None
            }
            Err(err) => {
                let reason = format!("ポート {port} で待ち受けられませんでした（{err}）");
                agent_core::note!("mcp server: {reason}");
                Some(reason)
            }
        }
    }

    /// 扉を閉じる。開いていなければ何もしない。
    pub fn stop(&mut self) {
        if let Some(server) = self.running.take() {
            server.stop();
        }
    }

    /// 設定を差し替えて、稼働状態を設定へ合わせ直す。
    ///
    /// **ON にする操作で合鍵を生成する**（凍結 3）。**閉じてから開ける** —
    /// 同じポートを掴んだまま開き直すと bind に失敗する。
    ///
    /// # Errors
    /// 設定ファイルが読めず保存できない場合。
    pub async fn apply(&mut self, enabled: bool, port: u16) -> Result<Option<String>, String> {
        let mut config = self.store.config().clone();
        config.enabled = enabled;
        config.port = port;
        if enabled {
            config.ensure_token();
        }
        self.store.save(config)?;

        self.stop();
        Ok(self.start_if_enabled().await)
    }

    /// 合鍵を作り直し、開いていれば新しい鍵で開き直す。
    ///
    /// # Errors
    /// 設定ファイルが読めず保存できない場合。
    pub async fn regenerate_token(&mut self) -> Result<Option<String>, String> {
        let mut config = self.store.config().clone();
        config.regenerate_token();
        self.store.save(config)?;

        self.stop();
        Ok(self.start_if_enabled().await)
    }
}

/// 合鍵を検査する層。**扉の内側へ入る前に落とす。**
pub async fn require_bearer_token(
    axum::extract::State(expected): axum::extract::State<Arc<String>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let ok = bearer_token(request.headers()).is_some_and(|token| token_matches(&expected, token));
    if !ok {
        // **理由を細かく分けない**（鍵が無いのか違うのかを教えない）。
        return axum::response::IntoResponse::into_response(axum::http::StatusCode::UNAUTHORIZED);
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_comparison_rejects_wrong_and_truncated_keys() {
        assert!(token_matches("abc123", "abc123"));
        assert!(!token_matches("abc123", "abc124"));
        // **前方一致で通らない**（長さの違いは先に落ちる）。
        assert!(!token_matches("abc123", "abc"));
        assert!(!token_matches("abc123", "abc1234"));
        assert!(!token_matches("abc123", ""));
    }

    #[test]
    fn bearer_token_is_extracted_only_from_the_expected_shape() {
        let mut headers = axum::http::HeaderMap::new();
        assert_eq!(bearer_token(&headers), None, "ヘッダ自体が無い");

        headers.insert(axum::http::header::AUTHORIZATION, "abc123".parse().unwrap());
        assert_eq!(bearer_token(&headers), None, "`Bearer ` が無い裸の値は採らない");

        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer abc123".parse().unwrap(),
        );
        assert_eq!(bearer_token(&headers), Some("abc123"));
    }

    #[test]
    fn token_is_generated_once_and_kept() {
        let mut config = McpServerConfig::default();
        assert_eq!(config.token, None, "既定では合鍵を作らない（凍結 3）");

        let first = config.ensure_token().to_owned();
        assert_eq!(first.len(), 32, "32 桁の 16 進");
        assert_eq!(
            config.ensure_token(),
            first,
            "**2 回目は作り直さない** — 作り直すとクライアント側の設定が無効になる"
        );

        let second = config.regenerate_token().to_owned();
        assert_ne!(second, first, "明示的な作り直しでは変わる");
    }

    #[test]
    fn default_config_keeps_the_door_closed() {
        let config = McpServerConfig::default();
        assert!(!config.enabled, "既定は OFF");
        assert_eq!(config.port, DEFAULT_PORT);
    }

    /// 壊れたファイルは**既定へ落として読むが、書き戻さない**（#70）。
    #[test]
    fn unreadable_config_blocks_writes_instead_of_overwriting() {
        let dir = std::env::temp_dir().join(format!(
            "concordia-mcpcfg-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(CONFIG_FILE);
        std::fs::write(&path, "{ これは JSON ではない").unwrap();

        let mut store = McpServerStore::load(&dir);
        assert!(store.blocked().is_some(), "読めなかったことを覚えている");
        assert!(!store.config().enabled, "読めない設定で扉は開かない");

        let err = store
            .save(McpServerConfig::default())
            .expect_err("保存は拒まれること");
        assert!(err.contains(CONFIG_FILE), "実際: {err}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{ これは JSON ではない",
            "**1 バイトも上書きしない** — 人が直せば戻る"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 保存 → 読み直しで往復すること（合鍵が消えない）。
    #[test]
    fn config_roundtrips_through_the_file() {
        let dir = std::env::temp_dir().join(format!(
            "concordia-mcpcfg-rt-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let mut store = McpServerStore::load(&dir);
        assert!(store.blocked().is_none(), "未作成は失敗ではない");

        let mut config = store.config().clone();
        config.enabled = true;
        config.port = 40000;
        let token = config.ensure_token().to_owned();
        store.save(config).unwrap();

        let reloaded = McpServerStore::load(&dir);
        assert!(reloaded.config().enabled);
        assert_eq!(reloaded.config().port, 40000);
        assert_eq!(reloaded.config().token.as_deref(), Some(token.as_str()));

        std::fs::remove_dir_all(&dir).ok();
    }
}
