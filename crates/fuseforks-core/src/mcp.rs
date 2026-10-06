//! MCP（Model Context Protocol）クライアント。
//!
//! 外部の MCP サーバーを子プロセスとして起動し、そのツールを
//! [`AgentTool`] として登録簿へ流し込む。**オーケストレーターからは
//! 同梱ツール（`remember`）と何も変わらないもの**として見える —
//! これが [`crate::tool`] の trait を LLM のワイヤ形にも MCP のワイヤ形にも
//! 依存させなかった理由である。
//!
//! # なぜ公式 SDK（rmcp）を使うのか
//!
//! LLM のワイヤ形は自前で持っている。プロンプトキャッシュの制御に踏み込む必要があり、
//! 互換層では「機能の交差集合」しか使えないためだった。MCP は事情が逆で、
//! 仕様が広く（handshake / capabilities / 通知 / キャンセル / ページング）、
//! しかも速い周期で改訂される。自前で持っても得るものが無い。
//!
//! # 設定ファイルの形
//!
//! Claude Desktop の `claude_desktop_config.json` と**同じ形**を採る。
//! 利用者が既に持っている設定をそのまま貼れることに実用上の価値がある。
//!
//! ```json
//! {
//!   "mcpServers": {
//!     "filesystem": {
//!       "command": "npx",
//!       "args": ["-y", "@modelcontextprotocol/server-filesystem", "D:\\work"]
//!     }
//!   }
//! }
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rmcp::model::{CallToolRequestParams, CallToolResult, Tool};
use rmcp::service::{Peer, RoleClient, RunningService};
use rmcp::transport::TokioChildProcess;
use rmcp::ServiceExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{CoreError, CoreResult};
use crate::secret::{env_secret_name, SecretStore};
use crate::tool::{AgentTool, ToolContext};

/// ツール名に使える最大長。OpenAI / Anthropic 双方の関数名制限に合わせる。
const MAX_TOOL_NAME: usize = 64;

/// 1 台へ接続してツール一覧を取るまでの上限。
///
/// **アプリの起動が外部コマンドの機嫌に握られてはいけない。** MCP サーバーは
/// 応答しないまま生きていることがあり（依存の取得待ち、対話的プロンプトで停止など）、
/// handshake は待てば返る保証が無い。上限で切って「接続できなかった」として先へ進む。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// 1 台の MCP サーバーへの繋ぎ方（Spec 47 で 2 形へ）。
///
/// Deserialize は [`RawMcpConfig`] 経由（全欄 optional の raw 構造体 +
/// 検証）。untagged enum を使わないのは、エラーが「どの variant にも
/// 一致しない」としか言えず**エントリ名と欄の名指しができない**ため
/// （Spec 34/36 で観測した xAI の 422 と同じ形）。
///
/// Serialize は**形で分ける**（Spec 47 rev2.1 の致命指摘）: stdio は
/// `type` を省略して従来形を出し、http は `type: "http"` を必ず出す。
/// http で省略すると次の読みで stdio と誤判別され「command がありません」に
/// 落ちる — **共通 `mcp.json` は [`crate::ConfigStore::write_mcp_config`] が
/// 構造体から書き戻す本番の経路**なので、これは golden の都合ではない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpServerConfig {
    /// 子プロセスを起動して stdio で話す（従来形。`type` 無しはこちら）。
    Stdio(McpStdioConfig),
    /// リモートの Streamable HTTP サーバーへ繋ぐ（`type: "http"`。Spec 47）。
    Http(McpHttpConfig),
}

/// stdio サーバーの起動方法。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpStdioConfig {
    /// 実行するコマンド（例: `npx`）。
    pub command: String,
    /// コマンド引数。
    pub args: Vec<String>,
    /// 追加の環境変数。
    ///
    /// **秘密をここへ書かせない。** API キーが要るサーバーは、
    /// 資格情報ストア（[`crate::secret`]）を通す経路を別途用意する。
    /// この欄は平文の `mcp.json` に保存される。
    pub env: BTreeMap<String, String>,
    /// 無効化フラグ。設定を消さずに一時停止するための欄。
    pub enabled: bool,
}

/// リモート（Streamable HTTP）サーバーへの繋ぎ方。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpHttpConfig {
    /// 単一エンドポイントの URL。https、または loopback の http だけが通る
    /// （Spec 47 D4 — 平文 http で Authorization が外へ飛ぶ形を既定で塞ぐ）。
    pub url: String,
    /// 毎リクエストに付けるヘッダー。
    ///
    /// **`env` より 1 段重い**（Spec 47 D3）— `env` はローカルの子プロセス
    /// 止まりだが、ここの Authorization は**外部へ送信され**、村（workspace）を
    /// 配るとトークンごと配られる。値はエラーにもログにも出さない（D7）。
    pub headers: BTreeMap<String, String>,
    /// 無効化フラグ。**検証は enabled に関わらず掛かる**（スキップは接続だけ）。
    pub enabled: bool,
}

impl McpServerConfig {
    /// 有効か（形に依らない共通の問い）。
    pub fn enabled(&self) -> bool {
        match self {
            Self::Stdio(config) => config.enabled,
            Self::Http(config) => config.enabled,
        }
    }
}

impl McpHttpConfig {
    /// `headers` の値が参照する秘密の名前（Spec 65 D3）。並びは名前の順・重複なし。
    ///
    /// 書き方は読み込み（[`validate_entry`]）で検査済みなので、ここでは不正な参照を
    /// 数えない。起動前検査と、秘密の変数名の衝突検査が読む。
    pub fn secret_ref_names(&self) -> BTreeSet<String> {
        self.headers
            .values()
            .filter_map(|value| secret_ref_names(value).ok())
            .flatten()
            .map(str::to_owned)
            .collect()
    }
}

/// `headers` の値に書ける秘密の参照の書き出し（Spec 65 D3）。`${secret:NAME}` の形で、
/// NAME は英大文字・数字・`_` だけ。
const SECRET_REF_OPEN: &str = "${secret:";

/// 秘密の参照の書き方の誤りと、引けなかった名前（Spec 65 D3）。**値は持たない。**
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretRefError {
    /// `${secret:` の後に `}` が無い・名前が空・名前に使えない文字がある。
    Malformed,
    /// 引けなかった名前（現れた順・重複なし）。
    Missing(Vec<String>),
}

/// 参照の名前から [`SecretStore`] の鍵を作る（`mcp:NAME`）。env では
/// [`env_secret_name`] で `FUSEFORKS_SECRET_MCP_NAME` に写る。
pub fn mcp_secret_key(name: &str) -> String {
    format!("mcp:{name}")
}

/// 値の中の参照の名前を、現れた順に拾う。`${secret:` を含まない値は空を返す
/// （**参照を書かない値には何もしない** — バイト等価の前提）。
///
/// # Errors
/// 書き方が不正なら [`SecretRefError::Malformed`]。
pub fn secret_ref_names(value: &str) -> Result<Vec<&str>, SecretRefError> {
    let mut names = Vec::new();
    let mut rest = value;
    while let Some(start) = rest.find(SECRET_REF_OPEN) {
        let after = &rest[start + SECRET_REF_OPEN.len()..];
        let end = after.find('}').ok_or(SecretRefError::Malformed)?;
        let name = &after[..end];
        let valid = !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
        if !valid {
            return Err(SecretRefError::Malformed);
        }
        names.push(name);
        rest = &after[end + 1..];
    }
    Ok(names)
}

/// 値の中の参照を、`lookup` で引いた値へ置き換える。参照の無い値はそのまま返す。
///
/// # Errors
/// 書き方が不正なら [`SecretRefError::Malformed`]、引けない名前があれば
/// [`SecretRefError::Missing`]（**1 つでも引けなければ置き換えた値を返さない** —
/// 一部だけ置き換えた値やプレースホルダの文字列を相手へ送らない）。
pub fn expand_secret_refs(
    value: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<String, SecretRefError> {
    let names = secret_ref_names(value)?;
    if names.is_empty() {
        return Ok(value.to_owned());
    }
    let mut out = String::with_capacity(value.len());
    let mut missing: Vec<String> = Vec::new();
    let mut rest = value;
    for name in names {
        let start = rest.find(SECRET_REF_OPEN).expect("名前を拾えた参照は必ずある");
        out.push_str(&rest[..start]);
        match lookup(name) {
            Some(secret) => out.push_str(&secret),
            None => {
                if !missing.iter().any(|m| m == name) {
                    missing.push(name.to_owned());
                }
            }
        }
        rest = &rest[start + SECRET_REF_OPEN.len() + name.len() + 1..];
    }
    out.push_str(rest);
    if missing.is_empty() {
        Ok(out)
    } else {
        Err(SecretRefError::Missing(missing))
    }
}

impl Serialize for McpServerConfig {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        match self {
            // 欄の並びと「全欄を常に出す」は旧 derive と同じ — 出力は従来形に
            // バイト一致する（golden で凍結）。
            Self::Stdio(config) => {
                let mut out = serializer.serialize_struct("McpServerConfig", 4)?;
                out.serialize_field("command", &config.command)?;
                out.serialize_field("args", &config.args)?;
                out.serialize_field("env", &config.env)?;
                out.serialize_field("enabled", &config.enabled)?;
                out.end()
            }
            Self::Http(config) => {
                let mut out = serializer.serialize_struct("McpServerConfig", 4)?;
                out.serialize_field("type", "http")?;
                out.serialize_field("url", &config.url)?;
                out.serialize_field("headers", &config.headers)?;
                out.serialize_field("enabled", &config.enabled)?;
                out.end()
            }
        }
    }
}

/// `mcp.json` 全体。
///
/// キーが `mcpServers` なのは Claude Desktop の設定と互換にするため。
/// 互換を主張するのは **stdio と Streamable HTTP の 2 形だけ** — 旧 SSE
/// transport は意図的な非互換（Spec 47 D1。名指しで拒否する）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawMcpConfig")]
pub struct McpConfig {
    /// サーバー名 → 繋ぎ方。
    #[serde(rename = "mcpServers")]
    pub servers: BTreeMap<String, McpServerConfig>,
}

/// ワイヤの受け皿（全欄 optional）。検証は [`validate_entry`] が持つ。
#[derive(Deserialize)]
struct RawServerConfig {
    #[serde(rename = "type")]
    kind: Option<String>,
    command: Option<String>,
    args: Option<Vec<String>>,
    env: Option<BTreeMap<String, String>>,
    url: Option<String>,
    headers: Option<BTreeMap<String, String>>,
    enabled: Option<bool>,
}

#[derive(Deserialize)]
struct RawMcpConfig {
    #[serde(default, rename = "mcpServers")]
    servers: BTreeMap<String, RawServerConfig>,
}

impl TryFrom<RawMcpConfig> for McpConfig {
    type Error = String;

    fn try_from(raw: RawMcpConfig) -> Result<Self, Self::Error> {
        let mut servers = BTreeMap::new();
        for (name, entry) in raw.servers {
            let config = validate_entry(&name, entry)?;
            servers.insert(name, config);
        }
        Ok(Self { servers })
    }
}

/// エントリ 1 件の検証（Spec 47 D2 — **固定順**、最初の違反 1 つを
/// エントリ名と欄で名指しする）:
/// 1. 形の判別 → 2. 相互排他（両向き） → 3. 必須欄 → 4. 値の形式 →
/// 5. URL スキーム（D4） → 6. `headers` の秘密の参照の書き方（Spec 65 D3。http だけ）。
///
/// **`enabled: false` でも検証は掛かる** — 無効のまま不正な URL や平文 http を
/// 残せる形にしない（スキップされるのは接続だけ）。
fn validate_entry(name: &str, raw: RawServerConfig) -> Result<McpServerConfig, String> {
    // 1. 形の判別。
    let http = match raw.kind.as_deref() {
        None | Some("stdio") => false,
        Some("http") => true,
        Some("sse") => {
            return Err(format!(
                "`{name}`: sse は旧形式です。接続先が Streamable HTTP に対応している\
                 必要があります（多くのサーバーは /sse ではなく /mcp などの\
                 単一エンドポイントです）"
            ));
        }
        Some(other) => {
            return Err(format!("`{name}`: type `{other}` は使えません（stdio か http）"));
        }
    };
    let enabled = raw.enabled.unwrap_or(true);

    if http {
        // 2. 相互排他 — 黙ってどちらかを選ばない。
        for (field, present) in [
            ("command", raw.command.is_some()),
            ("args", raw.args.is_some()),
            ("env", raw.env.is_some()),
        ] {
            if present {
                return Err(format!(
                    "`{name}`: http のエントリに {field} は書けません（stdio の欄です）"
                ));
            }
        }
        // 3. 必須欄。
        let Some(url) = raw.url else {
            return Err(format!("`{name}`: type が http ですが url がありません"));
        };
        // 4. 値の形式。
        let url = url.trim().to_owned();
        if url.is_empty() {
            return Err(format!("`{name}`: url が空です"));
        }
        let parsed = url::Url::parse(&url).map_err(|err| {
            format!("`{name}`: url の形式が不正です（絶対 URL を指定してください）: {err}")
        })?;
        // 5. スキーム（D4）。
        if !scheme_allowed(&parsed) {
            return Err(format!(
                "`{name}`: url は https、またはループバック（127.0.0.1 / [::1] / \
                 localhost）の http だけが使えます（平文の http で Authorization が\
                 外へ飛ぶ形を塞いでいます）"
            ));
        }
        // 6. 秘密の参照の書き方（Spec 65 D3）。**読み込みで拒む** — 接続の時点で起きうる
        //    失敗を「秘密が無い」の 1 つにする。値は載せず、エントリとヘッダーの名前だけ。
        let headers = raw.headers.unwrap_or_default();
        for (key, value) in &headers {
            if secret_ref_names(value).is_err() {
                return Err(format!(
                    "`{name}`: ヘッダー `{key}` の値の ${{secret:…}} の書き方が不正です\
                     （名前は英大文字・数字・_ だけで、}} で閉じる。値は表示しません）"
                ));
            }
        }
        Ok(McpServerConfig::Http(McpHttpConfig {
            url,
            headers,
            enabled,
        }))
    } else {
        // 2. 相互排他（逆向き）。
        for (field, present) in [("url", raw.url.is_some()), ("headers", raw.headers.is_some())] {
            if present {
                return Err(format!(
                    "`{name}`: stdio のエントリに {field} は書けません（リモートサーバー\
                     なら \"type\": \"http\" を指定してください）"
                ));
            }
        }
        // 3. 必須欄。
        let Some(command) = raw.command else {
            return Err(format!(
                "`{name}`: command がありません（リモートサーバーなら \"type\": \"http\" \
                 と url を指定してください）"
            ));
        };
        Ok(McpServerConfig::Stdio(McpStdioConfig {
            command,
            args: raw.args.unwrap_or_default(),
            env: raw.env.unwrap_or_default(),
            enabled,
        }))
    }
}

/// D4: https、または loopback の http。**判定は文字列照合ではなく host**
/// （`Url::host()` — IP は `is_loopback()` で `::1` を含み、ホスト名は
/// `localhost` の完全一致。プライベート帯・`host.docker.internal` は通さない）。
fn scheme_allowed(url: &url::Url) -> bool {
    match url.scheme() {
        "https" => true,
        "http" => match url.host() {
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
            None => false,
        },
        _ => false,
    }
}

/// 1 台の接続状態。UI へそのまま出せる形。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerStatus {
    /// 設定上の名前。
    pub name: String,
    /// 接続できているか。
    pub connected: bool,
    /// 提供されたツール名（修飾後）。
    pub tools: Vec<String>,
    /// 失敗した理由。接続できていれば `None`。
    pub error: Option<String>,
}

/// ツール名を「サーバー名で修飾した、安全な名前」へ変換する（純関数）。
///
/// 修飾するのは**サーバーを跨ぐと名前が衝突するから**。`search` のような
/// ありふれた名前は複数のサーバーが持ちうる。衝突すると後勝ちで片方が消え、
/// しかもモデルからは「そういうツールだ」としか見えない（黙って消える類の事故）。
///
/// 記号は `_` へ落とす。関数名に使える文字は `[a-zA-Z0-9_-]` に限られ、
/// 範囲外の文字はプロバイダ側で拒否されるかツールごと無視される。
pub fn qualified_tool_name(server: &str, tool: &str) -> String {
    let sanitize = |s: &str| -> String {
        s.chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
            .collect()
    };

    let name = format!("{}__{}", sanitize(server), sanitize(tool));
    if name.len() <= MAX_TOOL_NAME {
        return name;
    }

    // 長すぎる場合は**末尾**を残す。ツール名の識別力は後ろ（tool 側）にあり、
    // 頭を残すと `filesystem__` だけが並んで区別が付かなくなる。
    let tail: String = name
        .chars()
        .rev()
        .take(MAX_TOOL_NAME)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    tail
}

/// MCP のツール実行結果を、モデルへ返す 1 本の文字列へ落とす（純関数）。
///
/// テキスト以外のブロック（画像・音声・埋め込みリソース）は、そのまま渡せないので
/// **種別だけを明示して落とす**。黙って捨てると、モデルは「空の結果が返った」と
/// 解釈して同じツールを呼び直す。
pub fn render_tool_result(result: &CallToolResult) -> String {
    let mut parts: Vec<String> = Vec::new();

    for block in &result.content {
        match block.as_text() {
            Some(text) => parts.push(text.text.clone()),
            None => parts.push("（テキスト以外の内容が返されました。この経路では扱えません）".to_owned()),
        }
    }

    // 構造化結果を持つサーバーは、content が空でもこちらに本体を入れてくる。
    if let Some(structured) = &result.structured_content
        && parts.is_empty()
    {
        parts.push(structured.to_string());
    }

    if parts.is_empty() {
        parts.push("（空の結果）".to_owned());
    }

    let body = parts.join("\n");
    // 失敗は隠さない。会話は止めないが、モデルが「失敗した」と分かる形で返す。
    if result.is_error.unwrap_or(false) {
        format!("ツールはエラーを返しました:\n{body}")
    } else {
        body
    }
}

/// MCP サーバーの 1 ツールを [`AgentTool`] として見せるアダプタ。
pub struct McpTool {
    /// 修飾済みの名前（モデルへ提示する名前）。
    qualified: String,
    /// サーバー側の元の名前（呼ぶときはこちらを使う）。
    remote_name: String,
    /// 設定上のサーバー名。エラーメッセージの帰属に使う。
    server: String,
    description: String,
    parameters: Value,
    peer: Peer<RoleClient>,
}

impl McpTool {
    /// `tools/list` の 1 件からアダプタを組む。
    fn from_listed(server: &str, tool: &Tool, peer: Peer<RoleClient>) -> Self {
        let description = tool
            .description
            .as_deref()
            .unwrap_or("（説明なし）")
            .to_owned();
        // input_schema は JsonObject（= serde_json::Map）。canonical 側は Value を持つ。
        let parameters = Value::Object((*tool.input_schema).clone());

        Self {
            qualified: qualified_tool_name(server, &tool.name),
            remote_name: tool.name.to_string(),
            server: server.to_owned(),
            description,
            parameters,
            peer,
        }
    }
}

#[async_trait]
impl AgentTool for McpTool {
    fn name(&self) -> &str {
        &self.qualified
    }

    /// **言語は無視する**（Spec 35 D4-1）。名付けたのは接続先で、
    /// 訳語を当てると何が走ったかについて嘘になる。
    fn description(&self, _language: crate::world::Language) -> String {
        // どのサーバー由来かをモデルにも見せる。同種のツールが複数あるとき、
        // 説明文だけでは選べない（例: 2 つの検索ツール）。
        format!("[{}] {}", self.server, self.description)
    }

    fn parameters(&self, _language: crate::world::Language) -> Value {
        self.parameters.clone()
    }

    /// **理由欄を足さない**（Spec 27 D5）。
    ///
    /// このスキーマは**サーバーが宣言したもの**で、こちらの欄を生やすと
    /// そのまま `tools/call` の引数として転送される。
    /// `additionalProperties: false` を宣言しているサーバーは**呼び出しごと拒否する**。
    ///
    /// **画面では `ReasonState::Unsupported`（「外部ツール」）として出る** —
    /// 「モデルが書かなかった」ではなく「こちらが尋ねていない」。
    fn wants_reason(&self) -> bool {
        false
    }

    /// 接続先が返す本文は圧縮の対象（Spec 59 / `tool_prune_contract`）。
    ///
    /// **実測でツール出力の字数の約半分が MCP 由来**（`fuseforks.log` 2026-08-09〜09-22）。
    /// ただし**返り値の形は名前でも説明文でも決まらない** — 実際に叩いて数えたら、
    /// 4,000 字以上の本文の 30.1% が JSON の配列型で、これは `prune` が
    /// `structured` として素通しする（`failures.md` #134 / Spec 60）。
    fn prunable(&self) -> bool {
        true
    }

    async fn call(&self, _ctx: &ToolContext, args: &Value) -> CoreResult<String> {
        // MCP の arguments はオブジェクト。オブジェクト以外は引数なしとして送る
        // （モデルが `null` や文字列を寄越すことは実際にある）。
        let arguments = args.as_object().cloned();
        let mut params = CallToolRequestParams::new(self.remote_name.clone());
        if let Some(arguments) = arguments {
            params = params.with_arguments(arguments);
        }

        let result = self
            .peer
            .call_tool(params)
            .await
            .map_err(|err| CoreError::Mcp {
                server: self.server.clone(),
                message: err.to_string(),
            })?;

        Ok(render_tool_result(&result))
    }
}

/// 接続中の MCP サーバー群。
///
/// [`RunningService`] を**保持し続ける**必要がある。drop すると接続が畳まれ、
/// 子プロセスが落ちる（`Peer` だけ持っていても通信できなくなる）。
#[derive(Default)]
pub struct McpManager {
    /// 生かしておくための接続本体。触らないが捨ててもいけない。
    connections: Vec<RunningService<RoleClient, ()>>,
    /// UI へ見せる状態。
    statuses: Vec<McpServerStatus>,
    /// 登録簿へ流すツール。
    tools: Vec<Arc<dyn AgentTool>>,
}

impl McpManager {
    /// 設定に従って全サーバーへ接続する。
    ///
    /// **1 台の失敗で全体を止めない。** 起動できないサーバーは
    /// [`McpServerStatus::error`] に理由を残して次へ進む。MCP サーバーは
    /// 外部コマンドであり、未インストール・パス違い・権限で普通に落ちる。
    /// そこでアプリが起動しなくなるのは筋が悪い。
    ///
    /// `secrets` は `headers` の `${secret:NAME}` を引く置き場（Spec 65 D3。GUI は資格情報
    /// ストア、`--secrets env` は環境変数）。参照を書かない `mcp.json` はここを 1 度も読まない。
    pub async fn connect_all(config: &McpConfig, secrets: &dyn SecretStore) -> Self {
        let mut manager = Self::default();

        for (name, server) in &config.servers {
            if !server.enabled() {
                manager.statuses.push(McpServerStatus {
                    name: name.clone(),
                    connected: false,
                    tools: Vec::new(),
                    error: Some("無効化されています".to_owned()),
                });
                continue;
            }

            let attempt = tokio::time::timeout(CONNECT_TIMEOUT, connect_one(name, server, secrets))
                .await
                .unwrap_or_else(|_| {
                    Err(CoreError::Mcp {
                        server: name.clone(),
                        message: format!(
                            "{} 秒以内に応答しませんでした",
                            CONNECT_TIMEOUT.as_secs()
                        ),
                    })
                });

            match attempt {
                Ok((service, tools)) => {
                    manager.statuses.push(McpServerStatus {
                        name: name.clone(),
                        connected: true,
                        tools: tools.iter().map(|t| t.name().to_owned()).collect(),
                        error: None,
                    });
                    manager.tools.extend(tools);
                    manager.connections.push(service);
                }
                Err(err) => {
                    manager.statuses.push(McpServerStatus {
                        name: name.clone(),
                        connected: false,
                        tools: Vec::new(),
                        error: Some(err.to_string()),
                    });
                }
            }
        }

        manager
    }

    /// 各サーバーの接続状態。
    pub fn statuses(&self) -> &[McpServerStatus] {
        &self.statuses
    }

    /// 登録簿へ流すツール。
    pub fn tools(&self) -> &[Arc<dyn AgentTool>] {
        &self.tools
    }

    /// 全接続を畳む。子プロセスも終了する。
    pub async fn shutdown(self) {
        for connection in self.connections {
            // 畳めなくても続ける。プロセスは Drop 側でも回収される。
            let _ = connection.cancel().await;
        }
    }
}

/// 1 台へ接続し、ツール一覧まで取る。
async fn connect_one(
    name: &str,
    config: &McpServerConfig,
    secrets: &dyn SecretStore,
) -> CoreResult<(RunningService<RoleClient, ()>, Vec<Arc<dyn AgentTool>>)> {
    let service = match config {
        McpServerConfig::Stdio(config) => serve_stdio(name, config).await?,
        McpServerConfig::Http(config) => {
            let headers = resolve_headers(name, &config.headers, secrets)?;
            serve_http(name, &config.url, &headers).await?
        }
    };

    let listed = service.list_all_tools().await.map_err(|err| CoreError::Mcp {
        server: name.to_owned(),
        message: format!("ツール一覧を取得できませんでした: {err}"),
    })?;

    let peer = service.peer().clone();
    let tools: Vec<Arc<dyn AgentTool>> = listed
        .iter()
        .map(|tool| Arc::new(McpTool::from_listed(name, tool, peer.clone())) as Arc<dyn AgentTool>)
        .collect();

    Ok((service, tools))
}

/// stdio サーバーを起動して initialize まで済ませる。
async fn serve_stdio(
    name: &str,
    config: &McpStdioConfig,
) -> CoreResult<RunningService<RoleClient, ()>> {
    // コマンドは PATH から解決する。**素の名前をそのまま渡してはいけない。**
    // Windows では `npx` が拡張子なしのスクリプトとしても PATH に居るため、
    // CreateProcess が実行できず `program not found` になる（実測で確認）。
    // Claude Desktop の設定は素の `npx` で書かれており、「そのまま貼れる」という
    // 互換の主張はここを解決して初めて成り立つ。
    let mut command =
        rmcp::transport::which_command(&config.command).map_err(|err| CoreError::Mcp {
            server: name.to_owned(),
            message: format!(
                "コマンド `{}` が見つかりません（PATH を確認してください）: {err}",
                config.command
            ),
        })?;
    command.args(&config.args);
    for (key, value) in &config.env {
        command.env(key, value);
    }
    // Windows で子プロセスのコンソール窓が開くのを防ぐ。GUI アプリから
    // MCP サーバーを起動するたびに黒い窓が現れるのは、単純に壊れて見える。
    // tokio の Command は Windows で creation_flags を直に持つ（std の
    // CommandExt を use する必要はない）。
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let spawn_error = |err: std::io::Error| CoreError::Mcp {
        server: name.to_owned(),
        message: format!("起動できませんでした: {err}"),
    };
    // 子の標準エラーは rmcp の既定で親から継がれる。`fuseforks-cli --events jsonl` の間は
    // 標準エラーの全行が JSON（Spec 64 凍結 13）なので、そのときだけ piped にして 1 行ずつ
    // 包み直す。GUI（と JSON でない CLI）は今までどおり継ぐ — 挙動を変えない。
    let transport = if crate::diag::stderr_json() {
        let (transport, stderr) = TokioChildProcess::builder(command)
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(spawn_error)?;
        if let Some(stderr) = stderr {
            tokio::spawn(forward_child_stderr(format!("mcp:{name}"), stderr));
        }
        transport
    } else {
        TokioChildProcess::new(command).map_err(spawn_error)?
    };

    ().serve(transport).await.map_err(|err| CoreError::Mcp {
        server: name.to_owned(),
        message: format!("接続に失敗しました: {err}"),
    })
}

/// 子の標準エラーを 1 行ずつ読み、JSON の 1 行で写す（`--events jsonl` の間だけ動く）。
async fn forward_child_stderr(source: String, stderr: tokio::process::ChildStderr) {
    forward_lines(stderr, |line| crate::diag::child_stderr_line(&source, line)).await;
}

/// 子の出力 1 行の上限（バイト）。超えたらそこで切って 1 行として写す。
///
/// **改行を出さない子（プログレス表示・バイナリの垂れ流し）で読み手のバッファが際限なく
/// 伸びない**ための上限。無人で回す `serve` では、1 つの子が親のメモリを食い尽くす形を
/// 作らない。切った位置で UTF-8 の文字が割れることがあり、その文字は置き換えて写す。
const CHILD_LINE_MAX: usize = 64 * 1024;

/// 読み手を 1 行ずつ（最長 [`CHILD_LINE_MAX`] バイト）に切って `emit` へ渡す。空行は渡さない。
///
/// UTF-8 でない出力（Windows の子が出すコードページの文字）も落とさず、不正な並びは
/// 置き換えて渡す。読み手が閉じるか読めなくなれば終わる。
async fn forward_lines<R>(reader: R, mut emit: impl FnMut(&str))
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::{AsyncBufReadExt, AsyncReadExt};
    let mut reader = tokio::io::BufReader::new(reader);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        // `take` で 1 回に読む量を上限で切る — 改行が来なくても上限で返る。
        match (&mut reader)
            .take(CHILD_LINE_MAX as u64)
            .read_until(b'\n', &mut buf)
            .await
        {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                let line = String::from_utf8_lossy(&buf);
                let line = line.trim_end_matches(['\r', '\n']);
                if !line.is_empty() {
                    emit(line);
                }
            }
        }
    }
}

/// `headers` の値の `${secret:NAME}` を引いて置き換える（Spec 65 D3）。
///
/// **1 つでも引けなければ接続しない** — プレースホルダの文字列も、一部だけ置き換えた値も
/// 相手へ送らない。理由は**名前と変数名だけ**を書く（値は書かない — Spec 47 D7）。
/// 置き場の読み取りが失敗した鍵も「無い」として数える（値が手元に無いことは同じ）。
fn resolve_headers(
    name: &str,
    headers: &BTreeMap<String, String>,
    secrets: &dyn SecretStore,
) -> CoreResult<BTreeMap<String, String>> {
    let lookup = |ref_name: &str| secrets.get(&mcp_secret_key(ref_name)).ok().flatten();
    let mut resolved = BTreeMap::new();
    let mut missing: Vec<String> = Vec::new();
    for (key, value) in headers {
        match expand_secret_refs(value, &lookup) {
            Ok(expanded) => {
                resolved.insert(key.clone(), expanded);
            }
            Err(SecretRefError::Missing(names)) => {
                for ref_name in names {
                    if !missing.contains(&ref_name) {
                        missing.push(ref_name);
                    }
                }
            }
            Err(SecretRefError::Malformed) => {
                return Err(CoreError::Mcp {
                    server: name.to_owned(),
                    message: format!(
                        "ヘッダー `{key}` の値の ${{secret:…}} の書き方が不正です（値は表示しません）"
                    ),
                });
            }
        }
    }
    if missing.is_empty() {
        return Ok(resolved);
    }
    let listed = missing
        .iter()
        .map(|ref_name| {
            format!(
                "{ref_name}（keyring の鍵 {} / 環境変数 {}）",
                mcp_secret_key(ref_name),
                env_secret_name(&mcp_secret_key(ref_name))
            )
        })
        .collect::<Vec<_>>()
        .join("、");
    Err(CoreError::Mcp {
        server: name.to_owned(),
        message: format!("ヘッダーが参照する秘密がありません: {listed}。接続していません"),
    })
}

/// リモート（Streamable HTTP）サーバーへ initialize まで済ませる（Spec 47 P2）。
///
/// **HTTP は最初の initialize が返って初めて成否が確定する**（stdio は spawn の
/// 失敗で即エラーが出る — 検知タイミングの違い。Spec 47 D5）。timeout は
/// stdio と同じく呼び出し元の [`CONNECT_TIMEOUT`] が掛ける。
///
/// **401 / 403 は「設定の確認が要る」として即確定し、自動では再試行しない** —
/// 接続は明示操作 / 起動時の 1 回だけで再接続ループは元から無く、その性質を
/// ここで凍結する（期限切れの Bearer で叩き続ける形を作らない）。
async fn serve_http(
    name: &str,
    url: &str,
    headers: &BTreeMap<String, String>,
) -> CoreResult<RunningService<RoleClient, ()>> {
    use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

    let mut transport_config = StreamableHttpClientTransportConfig::with_uri(url.to_owned());
    let mut custom: std::collections::HashMap<http::HeaderName, http::HeaderValue> =
        std::collections::HashMap::new();
    for (key, value) in headers {
        // Authorization の Bearer は rmcp の `auth_header` へ。**渡すのは素の
        // トークン** — reqwest の `bearer_auth` が接頭辞を付ける（P0 の実装読みと
        // 実測。`custom_headers` へ回すと `Bearer Bearer …` にはならないが、
        // rmcp が 401 時の再 initialize 等で使う認証の席はこちら）。
        if key.eq_ignore_ascii_case("authorization")
            && let Some(token) = value.strip_prefix("Bearer ")
        {
            transport_config = transport_config.auth_header(token.trim().to_owned());
            continue;
        }
        // 名前・値の検査はここで初めて掛かる。**値はエラーに載せない**（D7 —
        // Authorization 以外のヘッダーにもトークンは普通に入る: X-Api-Key 等）。
        let header_name =
            http::HeaderName::try_from(key.as_str()).map_err(|_| CoreError::Mcp {
                server: name.to_owned(),
                message: format!("ヘッダー `{key}` の名前が HTTP ヘッダーとして不正です"),
            })?;
        let header_value =
            http::HeaderValue::try_from(value.as_str()).map_err(|_| CoreError::Mcp {
                server: name.to_owned(),
                message: format!(
                    "ヘッダー `{key}` の値が HTTP ヘッダーとして不正です（値は表示しません）"
                ),
            })?;
        custom.insert(header_name, header_value);
    }
    if !custom.is_empty() {
        transport_config = transport_config.custom_headers(custom);
    }

    // `from_config` は rmcp 側の reqwest(0.13) Client を内部で作る。**こちらから
    // reqwest の型を名指ししない** — ワークスペースの 0.12（LLM クライアント）とは
    // 別の実体で、名指しすると版の不一致でコンパイルが落ちる（P0 実測）。
    let transport = rmcp::transport::StreamableHttpClientTransport::from_config(transport_config);
    ().serve(transport).await.map_err(|err| CoreError::Mcp {
        server: name.to_owned(),
        message: classify_http_connect_error(&err.to_string()),
    })
}

/// HTTP 接続エラーを、**相手の応答本文を含まない**分類文字列へ落とす（純関数。
/// Spec 47 D5 / D7）。
///
/// rmcp の 401 は `UnexpectedServerResponse("HTTP 401 Unauthorized: {本文}")` の
/// 形で**本文を逐語で運んでくる**（P0 実測）。本文には受信ヘッダーをエコーする
/// サーバーが実在するので、そのまま `McpServerStatus.error` へ写すと
/// Authorization の転送経路になる（`failures.md` #71 の系譜）。`HTTP <code>` を
/// 見つけたら**状態コードだけ**の自前の文へ差し替える。それ以外（DNS / 接続拒否 /
/// TLS）は reqwest 層のエラーで応答本文を含まないため、そのまま出す —
/// 全部を丸めると「動かないが理由が分からない」へ戻る。
fn classify_http_connect_error(text: &str) -> String {
    // OAuth の challenge（WWW-Authenticate）は rmcp が `AuthRequired` として
    // 返す（alphaXiv で実測 — 401 の HTTP 行を持たない別の形）。対話の
    // OAuth フローは持たない（Spec 47 Notes 2）ので、案内は API キーの側。
    if text.contains("AuthRequired") {
        return "認証が必要です。headers の Authorization に API キー（Bearer）を\
                設定してください。自動では再試行しません"
            .to_owned();
    }
    let Some(code) = find_http_status(text) else {
        return format!("接続に失敗しました: {text}");
    };
    match code {
        401 | 403 => format!(
            "認証に失敗しました（HTTP {code}）。headers の Authorization を確認して\
             ください。自動では再試行しません"
        ),
        404 => format!("エンドポイントが見つかりません（HTTP {code}）。url を確認してください"),
        _ => format!("サーバーがエラーを返しました（HTTP {code}）"),
    }
}

/// `HTTP 401` のような並びから 3 桁の状態コードを拾う。無ければ `None`。
fn find_http_status(text: &str) -> Option<u16> {
    let rest = &text[text.find("HTTP ")? + 5..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.len() == 3 { digits.parse().ok() } else { None }
}

#[cfg(test)]
mod tests {
    /// 子の出力は改行で切れ、改行の無い長い出力は上限で切れる（バッファが伸び続けない）。
    #[tokio::test]
    async fn child_output_is_split_by_newline_and_capped_without_one() {
        let mut input = b"one\r\ntwo\n\n".to_vec();
        input.extend(std::iter::repeat_n(b'x', CHILD_LINE_MAX * 2 + 10));
        let mut lines: Vec<String> = Vec::new();
        forward_lines(&input[..], |line| lines.push(line.to_owned())).await;
        assert_eq!(lines[0], "one");
        assert_eq!(lines[1], "two");
        let rest: Vec<usize> = lines[2..].iter().map(String::len).collect();
        assert_eq!(rest, vec![CHILD_LINE_MAX, CHILD_LINE_MAX, 10], "上限で切れる");
    }

    use super::*;

    /// サーバーが実際に返すワイヤ形（JSON）から結果を組む。
    ///
    /// `CallToolResult` は `#[non_exhaustive]` で外部から構造体リテラルを書けないが、
    /// それ以上に**ワイヤ形から起こすほうが忠実**である。ここで検証したいのは
    /// 「実サーバーの応答をどう畳むか」であって、Rust の構造体の詰め方ではない。
    fn result_from_wire(raw: &str) -> CallToolResult {
        serde_json::from_str(raw).expect("MCP の応答として読めること")
    }

    #[test]
    fn config_accepts_the_claude_desktop_shape() {
        // 利用者が既に持っている設定をそのまま貼れることに意味がある。
        let raw = r#"{
            "mcpServers": {
                "filesystem": {
                    "command": "npx",
                    "args": ["-y", "@modelcontextprotocol/server-filesystem", "D:\\work"]
                }
            }
        }"#;
        let config: McpConfig = serde_json::from_str(raw).expect("読めること");

        let McpServerConfig::Stdio(server) = config.servers.get("filesystem").expect("1 台目")
        else {
            panic!("type 無しは stdio と読む");
        };
        assert_eq!(server.command, "npx");
        assert_eq!(server.args.len(), 3);
        assert!(server.enabled, "enabled 未指定は有効として扱う");
        assert!(server.env.is_empty());
    }

    #[test]
    fn missing_config_is_an_empty_set_not_an_error() {
        let config: McpConfig = serde_json::from_str("{}").expect("空でも読めること");
        assert!(config.servers.is_empty());
    }

    // ---- Spec 47: 2 形の受理と 5 段の検証 --------------------------------------

    /// 1 エントリの JSON を `McpConfig` として読んだ結果。
    fn parse_entry(name: &str, body: &str) -> Result<McpConfig, String> {
        let raw = format!(r#"{{ "mcpServers": {{ "{name}": {body} }} }}"#);
        serde_json::from_str::<McpConfig>(&raw).map_err(|err| err.to_string())
    }

    /// 拒否されること + 文言がエントリ名と語を名指ししていること。
    fn rejects(name: &str, body: &str, expected_words: &[&str]) {
        let err = parse_entry(name, body).expect_err("拒否されること");
        assert!(err.contains(&format!("`{name}`")), "エントリ名の名指しが無い: {err}");
        for word in expected_words {
            assert!(err.contains(word), "`{word}` が文言に無い: {err}");
        }
    }

    /// 秘密の参照の名前を現れた順に拾う。参照の無い値は空。書き方の誤りは Malformed。
    #[test]
    fn secret_refs_are_parsed_in_order_and_malformed_ones_are_rejected() {
        assert_eq!(secret_ref_names("Bearer ${secret:OUTCASTS}").unwrap(), vec!["OUTCASTS"]);
        assert_eq!(
            secret_ref_names("${secret:A_1}:${secret:B2}").unwrap(),
            vec!["A_1", "B2"]
        );
        assert!(secret_ref_names("Bearer plain-token").unwrap().is_empty());
        // `$` や `${` を含んでも `${secret:` で始まる部分が無ければ参照ではない。
        assert!(secret_ref_names("price $5 and ${HOME}").unwrap().is_empty());
        for bad in [
            "${secret:}",
            "${secret:lower}",
            "${secret:HAS-DASH}",
            "${secret:NO_CLOSE",
            "ok ${secret:A} then ${secret:",
        ] {
            assert_eq!(secret_ref_names(bad), Err(SecretRefError::Malformed), "{bad}");
        }
    }

    /// 置き換えは参照だけ。参照の無い値はそのまま。引けない名前は全部返し、置き換えた値を返さない。
    #[test]
    fn expansion_replaces_only_refs_and_refuses_partial_values() {
        let lookup = |name: &str| (name == "A").then(|| "alpha".to_owned());
        assert_eq!(
            expand_secret_refs("Bearer ${secret:A}", &lookup).unwrap(),
            "Bearer alpha"
        );
        assert_eq!(
            expand_secret_refs("x${secret:A}y${secret:A}z", &lookup).unwrap(),
            "xalphayalphaz"
        );
        assert_eq!(expand_secret_refs("no refs $x", &lookup).unwrap(), "no refs $x");
        assert_eq!(
            expand_secret_refs("${secret:A} ${secret:B} ${secret:C} ${secret:B}", &lookup),
            Err(SecretRefError::Missing(vec!["B".to_owned(), "C".to_owned()]))
        );
    }

    /// 書き方の誤りは読み込みで拒む（エントリとヘッダーを名指し・値は出さない）。
    #[test]
    fn a_malformed_secret_ref_in_headers_is_rejected_at_parse() {
        rejects(
            "outcasts",
            r#"{ "type": "http", "url": "https://example.com/mcp",
                 "headers": { "Authorization": "Bearer ${secret:bad-name} tail" } }"#,
            &["Authorization", "${secret:…}"],
        );
        let err = parse_entry(
            "outcasts",
            r#"{ "type": "http", "url": "https://example.com/mcp",
                 "headers": { "Authorization": "Bearer ${secret:bad-name} tail" } }"#,
        )
        .unwrap_err();
        assert!(!err.contains("bad-name"), "値が漏れている: {err}");

        // 正しい参照は通り、名前を拾える。
        let config = parse_entry(
            "outcasts",
            r#"{ "type": "http", "url": "https://example.com/mcp",
                 "headers": { "Authorization": "Bearer ${secret:OUTCASTS}", "X-Other": "${secret:OTHER}" } }"#,
        )
        .unwrap();
        let McpServerConfig::Http(http) = &config.servers["outcasts"] else {
            panic!("http のはず");
        };
        assert_eq!(
            http.secret_ref_names().into_iter().collect::<Vec<_>>(),
            vec!["OTHER".to_owned(), "OUTCASTS".to_owned()]
        );
    }

    /// 引けない参照があれば接続しない。理由は名前と変数名だけ（値もプレースホルダも出ない）。
    #[test]
    fn unresolved_header_secrets_refuse_to_connect_and_name_only() {
        let store = crate::secret::InMemorySecretStore::new();
        store.set("mcp:A", "alpha-secret").unwrap();
        let mut headers = BTreeMap::new();
        headers.insert("Authorization".to_owned(), "Bearer ${secret:A}".to_owned());
        headers.insert("X-Key".to_owned(), "${secret:MISSING}".to_owned());
        let err = resolve_headers("srv", &headers, &store).unwrap_err().to_string();
        assert!(err.contains("MISSING"), "{err}");
        assert!(err.contains("FUSEFORKS_SECRET_MCP_MISSING"), "{err}");
        assert!(!err.contains("alpha-secret"), "値が漏れている: {err}");

        headers.remove("X-Key");
        let resolved = resolve_headers("srv", &headers, &store).unwrap();
        assert_eq!(resolved["Authorization"], "Bearer alpha-secret");
    }

    #[test]
    fn explicit_stdio_type_is_accepted() {
        // Claude Desktop の設定からの写しで普通に入ってくる行（今までは黙って無視）。
        let config =
            parse_entry("m", r#"{ "type": "stdio", "command": "npx" }"#).expect("受理されること");
        assert!(matches!(
            config.servers.get("m"),
            Some(McpServerConfig::Stdio(_))
        ));
    }

    #[test]
    fn http_entry_is_accepted_with_url_and_headers() {
        // 起点の実機入力（type: http + url + headers）は新設計では正常系（rev2.1）。
        let config = parse_entry(
            "elyth",
            r#"{ "type": "http", "url": "https://example.com/api/mcp", "headers": { "Authorization": "Bearer x" } }"#,
        )
        .expect("受理されること");
        let Some(McpServerConfig::Http(server)) = config.servers.get("elyth") else {
            panic!("http と読めること");
        };
        assert_eq!(server.url, "https://example.com/api/mcp");
        assert_eq!(server.headers.len(), 1);
        assert!(server.enabled, "enabled 未指定は有効");
    }

    #[test]
    fn sse_is_rejected_by_name_with_guidance() {
        // 互換を主張するのは stdio と Streamable HTTP の 2 形だけ（D1）。
        rejects(
            "old",
            r#"{ "type": "sse", "url": "https://example.com/sse" }"#,
            &["sse は旧形式", "Streamable HTTP"],
        );
    }

    #[test]
    fn unknown_type_is_rejected_by_name() {
        rejects("x", r#"{ "type": "websocket", "url": "https://a" }"#, &["websocket"]);
    }

    #[test]
    fn mutual_exclusion_works_in_both_directions() {
        // 片向きだけだと「黙ってどちらかを選ばない」が半分しか成立しない（rev2.1）。
        rejects(
            "h",
            r#"{ "type": "http", "url": "https://a.example", "command": "npx" }"#,
            &["command は書けません"],
        );
        rejects(
            "s",
            r#"{ "command": "npx", "url": "https://a.example" }"#,
            &["url は書けません", "type"],
        );
    }

    #[test]
    fn missing_required_fields_name_the_other_form() {
        // 起点の実機エラー `missing field 'command'` はこの文言に変わる —
        // 次の手（type: "http" を書く）が本文に載る。
        rejects("e", r#"{ "args": [] }"#, &["command がありません", "http"]);
        rejects("r", r#"{ "type": "http" }"#, &["url がありません"]);
    }

    #[test]
    fn url_must_be_an_absolute_url() {
        rejects("e", r#"{ "type": "http", "url": "" }"#, &["url が空"]);
        rejects("r", r#"{ "type": "http", "url": "/api/mcp" }"#, &["絶対 URL"]);
    }

    #[test]
    fn plain_http_is_loopback_only() {
        // D4 — 判定は host。文字列照合ではないので [::1] も通り、
        // プライベート帯は塞がる。
        for allowed in [
            "https://example.com/mcp",
            "http://127.0.0.1:8080/mcp",
            "http://[::1]:8080/mcp",
            "http://localhost:3000/mcp",
        ] {
            parse_entry("ok", &format!(r#"{{ "type": "http", "url": "{allowed}" }}"#))
                .unwrap_or_else(|err| panic!("{allowed} は通ること: {err}"));
        }
        for denied in [
            "http://example.com/mcp",
            "http://192.168.1.5/mcp",
            "http://host.docker.internal/mcp",
            "ws://127.0.0.1/mcp",
        ] {
            rejects("ng", &format!(r#"{{ "type": "http", "url": "{denied}" }}"#), &["https"]);
        }
    }

    #[test]
    fn disabled_entries_are_still_validated() {
        // 無効のまま不正な URL を残せる形にしない（rev2.1 — スキップは接続だけ）。
        rejects(
            "off",
            r#"{ "type": "http", "url": "http://example.com/mcp", "enabled": false }"#,
            &["https"],
        );
    }

    // ---- Spec 47 D6: Serialize は形で分ける（golden） ---------------------------

    #[test]
    fn stdio_serializes_to_the_legacy_shape_byte_for_byte() {
        // 期待値は**旧 derive の実出力から捕獲した**（手書きの転記ではない —
        // Spec 35 の golden 先行手順）。type が生えたら旧クライアントとの
        // 共有で #112 の形になるので、バイト等価で凍結する。
        let config = McpConfig {
            servers: BTreeMap::from([(
                "filesystem".to_owned(),
                McpServerConfig::Stdio(McpStdioConfig {
                    command: "npx".to_owned(),
                    args: vec![
                        "-y".to_owned(),
                        "@modelcontextprotocol/server-filesystem".to_owned(),
                        "/work".to_owned(),
                    ],
                    env: BTreeMap::from([("KEY".to_owned(), "value".to_owned())]),
                    enabled: true,
                }),
            )]),
        };
        let expected = r#"{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": [
        "-y",
        "@modelcontextprotocol/server-filesystem",
        "/work"
      ],
      "env": {
        "KEY": "value"
      },
      "enabled": true
    }
  }
}"#;
        assert_eq!(serde_json::to_string_pretty(&config).unwrap(), expected);
    }

    #[test]
    fn http_serializes_with_its_type_and_round_trips() {
        // `type` を省略すると次の読みで stdio と誤判別される（rev2.1 の致命）。
        // 共通 mcp.json は write_mcp_config が構造体から書き戻す本番の経路。
        let config = McpConfig {
            servers: BTreeMap::from([(
                "elyth".to_owned(),
                McpServerConfig::Http(McpHttpConfig {
                    url: "https://example.com/api/mcp".to_owned(),
                    headers: BTreeMap::from([(
                        "Authorization".to_owned(),
                        "Bearer x".to_owned(),
                    )]),
                    enabled: true,
                }),
            )]),
        };
        let json = serde_json::to_string_pretty(&config).unwrap();
        assert!(json.contains(r#""type": "http""#), "実際: {json}");
        let back: McpConfig = serde_json::from_str(&json).expect("往復で読めること");
        assert_eq!(back, config, "Serialize → Deserialize で形が保存されること");
    }

    #[test]
    fn stdio_round_trips_without_growing_a_type_field() {
        let config = McpConfig {
            servers: BTreeMap::from([(
                "m".to_owned(),
                McpServerConfig::Stdio(McpStdioConfig {
                    command: "npx".to_owned(),
                    args: Vec::new(),
                    env: BTreeMap::new(),
                    enabled: false,
                }),
            )]),
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(!json.contains("\"type\""), "stdio に type が生えている: {json}");
        let back: McpConfig = serde_json::from_str(&json).expect("往復で読めること");
        assert_eq!(back, config);
    }

    // ---- Spec 47 D5/D7: HTTP エラーの分類（本文を転記しない） -------------------

    #[test]
    fn http_status_errors_drop_the_response_body() {
        // rmcp の 401 の実物の形（P0 実測）。本文（ここでは受信ヘッダーの
        // エコーを模した文字列）が分類後の文へ 1 文字も漏れないこと。
        let raw = r#"UnexpectedServerResponse("HTTP 401 Unauthorized: {\"error\":\"UNAUTHENTICATED\",\"echo\":\"Bearer secret-token-xyz\"}")"#;
        let classified = classify_http_connect_error(raw);
        assert!(classified.contains("HTTP 401"), "実際: {classified}");
        assert!(classified.contains("再試行しません"), "実際: {classified}");
        assert!(!classified.contains("secret-token-xyz"), "本文が漏れている: {classified}");
        assert!(!classified.contains("UNAUTHENTICATED"), "本文が漏れている: {classified}");

        assert!(
            classify_http_connect_error("… HTTP 404 Not Found: <html>…").contains("url を確認"),
        );
        assert!(
            classify_http_connect_error("… HTTP 500 Internal: {…}").contains("HTTP 500"),
        );
    }

    #[test]
    fn oauth_challenges_are_classified_as_auth_needed() {
        // alphaXiv の実測（P4）— OAuth の challenge は HTTP 行を持たない
        // `AuthRequired` で返る。生文のまま出すと技術的すぎて次の手が読めない。
        let raw = r#"AuthRequired(AuthRequiredError { www_authenticate_header: "Bearer realm=\"mcp\", resource_metadata=\"https://api.example.org/.well-known/oauth-protected-resource\"" })"#;
        let classified = classify_http_connect_error(raw);
        assert!(classified.contains("API キー"), "実際: {classified}");
        assert!(!classified.contains("well-known"), "生文のまま: {classified}");
    }

    #[test]
    fn non_status_errors_pass_through_for_diagnosis() {
        // DNS / 接続拒否は reqwest 層のエラーで応答本文を含まない。丸めると
        // 「動かないが理由が分からない」へ戻るので、そのまま出す。
        let raw = "error sending request: dns error: no such host";
        assert!(classify_http_connect_error(raw).contains("no such host"));
    }

    #[test]
    fn tool_names_are_namespaced_by_server() {
        // 同名ツールがサーバーを跨いで衝突しないこと。
        assert_eq!(qualified_tool_name("github", "search"), "github__search");
        assert_ne!(
            qualified_tool_name("github", "search"),
            qualified_tool_name("slack", "search")
        );
    }

    #[test]
    fn tool_names_are_reduced_to_safe_characters() {
        // 関数名に使えるのは [a-zA-Z0-9_-]。範囲外はプロバイダに拒否される。
        let name = qualified_tool_name("my server.v2", "read:file");
        assert_eq!(name, "my_server_v2__read_file");
        assert!(
            name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
            "実際: {name}"
        );
    }

    #[test]
    fn overlong_tool_names_keep_the_distinguishing_tail() {
        let long_server = "a".repeat(50);
        let name = qualified_tool_name(&long_server, "fetch_the_document");
        assert!(name.len() <= MAX_TOOL_NAME, "実際の長さ: {}", name.len());
        // 頭を残すとサーバー名だけが並んで区別が付かない。識別力のある末尾を残す。
        assert!(name.ends_with("fetch_the_document"), "実際: {name}");
    }

    #[test]
    fn text_blocks_are_joined_as_is() {
        let result = result_from_wire(
            r#"{"content":[{"type":"text","text":"結果です"}],"isError":false}"#,
        );
        assert_eq!(render_tool_result(&result), "結果です");
    }

    #[test]
    fn errors_are_labelled_rather_than_hidden() {
        // 会話は止めないが、失敗したことはモデルに分かる形で返す。
        let result = result_from_wire(
            r#"{"content":[{"type":"text","text":"ファイルがありません"}],"isError":true}"#,
        );
        let rendered = render_tool_result(&result);
        assert!(rendered.contains("エラー"), "実際: {rendered}");
        assert!(rendered.contains("ファイルがありません"));
    }

    #[test]
    fn non_text_blocks_are_reported_rather_than_dropped() {
        // 黙って捨てると、モデルは「空が返った」と読んで同じツールを呼び直す。
        let result = result_from_wire(
            r#"{"content":[{"type":"image","data":"ZGF0YQ==","mimeType":"image/png"}]}"#,
        );
        let rendered = render_tool_result(&result);
        assert!(!rendered.is_empty());
        assert!(rendered.contains("テキスト以外"), "実際: {rendered}");
    }

    #[test]
    fn structured_content_is_used_when_there_is_no_text() {
        let result = result_from_wire(r#"{"content":[],"structuredContent":{"count":3}}"#);
        assert!(render_tool_result(&result).contains("count"));
    }

    #[test]
    fn an_entirely_empty_result_still_says_something() {
        // content 欠落は仕様上ありうる（サーバーによっては返さない）。
        let result = result_from_wire(r#"{"isError":false}"#);
        assert!(!render_tool_result(&result).is_empty());
    }
}
