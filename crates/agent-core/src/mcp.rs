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

use std::collections::BTreeMap;
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
use crate::tool::{AgentTool, ToolContext};

/// ツール名に使える最大長。OpenAI / Anthropic 双方の関数名制限に合わせる。
const MAX_TOOL_NAME: usize = 64;

/// 1 台へ接続してツール一覧を取るまでの上限。
///
/// **アプリの起動が外部コマンドの機嫌に握られてはいけない。** MCP サーバーは
/// 応答しないまま生きていることがあり（依存の取得待ち、対話的プロンプトで停止など）、
/// handshake は待てば返る保証が無い。上限で切って「接続できなかった」として先へ進む。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// 1 台の MCP サーバーの起動方法。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// 実行するコマンド（例: `npx`）。
    pub command: String,
    /// コマンド引数。
    #[serde(default)]
    pub args: Vec<String>,
    /// 追加の環境変数。
    ///
    /// **秘密をここへ書かせない。** API キーが要るサーバーは、
    /// 資格情報ストア（[`crate::secret`]）を通す経路を別途用意する。
    /// この欄は平文の `mcp.json` に保存される。
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// 無効化フラグ。設定を消さずに一時停止するための欄。
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// `mcp.json` 全体。
///
/// キーが `mcpServers` なのは Claude Desktop の設定と互換にするため。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpConfig {
    /// サーバー名 → 起動方法。
    #[serde(default, rename = "mcpServers")]
    pub servers: BTreeMap<String, McpServerConfig>,
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
    if let Some(structured) = &result.structured_content {
        if parts.is_empty() {
            parts.push(structured.to_string());
        }
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

    fn description(&self) -> String {
        // どのサーバー由来かをモデルにも見せる。同種のツールが複数あるとき、
        // 説明文だけでは選べない（例: 2 つの検索ツール）。
        format!("[{}] {}", self.server, self.description)
    }

    fn parameters(&self) -> Value {
        self.parameters.clone()
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
    pub async fn connect_all(config: &McpConfig) -> Self {
        let mut manager = Self::default();

        for (name, server) in &config.servers {
            if !server.enabled {
                manager.statuses.push(McpServerStatus {
                    name: name.clone(),
                    connected: false,
                    tools: Vec::new(),
                    error: Some("無効化されています".to_owned()),
                });
                continue;
            }

            let attempt = tokio::time::timeout(CONNECT_TIMEOUT, connect_one(name, server))
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
) -> CoreResult<(RunningService<RoleClient, ()>, Vec<Arc<dyn AgentTool>>)> {
    let mut command = tokio::process::Command::new(&config.command);
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

    let transport = TokioChildProcess::new(command).map_err(|err| CoreError::Mcp {
        server: name.to_owned(),
        message: format!("起動できませんでした: {err}"),
    })?;

    let service = ().serve(transport).await.map_err(|err| CoreError::Mcp {
        server: name.to_owned(),
        message: format!("接続に失敗しました: {err}"),
    })?;

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

#[cfg(test)]
mod tests {
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

        let server = config.servers.get("filesystem").expect("1 台目");
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
