//! # agent-core
//!
//! Concordia のマルチエージェント・オーケストレーション中核。
//!
//! ## 依存の向き
//!
//! このクレートは **GUI 層（Tauri / Vue）に一切依存しない**。逆向きの依存だけが存在する:
//!
//! ```text
//! apps/gui-tauri  ──依存──▶  crates/agent-core
//! ```
//!
//! `Cargo.toml` に `tauri` が現れないことがこの分離の機械的な保証になっている。
//! GUI への通知は [`event::CoreEvent`] を `broadcast` チャネルへ流すだけで、
//! 受け手が Tauri かテストコードかをコア層は知らない。
//!
//! ## 並行モデル
//!
//! | 担当 | ランタイム | 理由 |
//! |---|---|---|
//! | エージェント実行・LLM 呼び出し・配送 | Tokio | I/O バウンド。待機中にスレッドを占有しない |
//! | ログのトークン集計 | Rayon | CPU バウンド。コア数ぶんの並列で焼き切る |
//!
//! エージェントを Rayon に載せない理由は、Rayon のプールがコア数に固定されるため。
//! ネットワーク待ちでスレッドを塞ぐと、同時稼働エージェント数がコア数で頭打ちになる。
//! 橋渡しは [`compute::spawn_rayon`] が担い、双方をブロックしない。
//!
//! ## 最小の使い方
//!
//! ```no_run
//! use std::sync::Arc;
//! use agent_core::{
//!     ConfigStore, FixedBackendFactory, InMemorySecretStore, Orchestrator, OrchestratorConfig,
//!     model::{AgentSpec, ModelTemplate},
//! };
//!
//! # async fn example() -> Result<(), agent_core::CoreError> {
//! let store = ConfigStore::new("./workspace");
//! let orchestrator = Orchestrator::bootstrap(
//!     store,
//!     // 実運用では HttpBackendFactory と KeyringSecretStore を渡す。
//!     Arc::new(FixedBackendFactory::echo("[echo]")),
//!     Arc::new(InMemorySecretStore::new()),
//!     OrchestratorConfig::default(),
//! )
//! .await?;
//!
//! orchestrator
//!     .upsert_template(ModelTemplate::new("tpl_default", "既定", "gpt-4o"))
//!     .await?;
//! orchestrator
//!     .create_agent(AgentSpec::new("agent_01", "PlannerAgent", "tpl_default"))
//!     .await?;
//!
//! orchestrator.start_agent(&"agent_01".into()).await?;
//! orchestrator
//!     .send_user_message(&"agent_01".into(), "計画を立ててください")
//!     .await?;
//! # Ok(())
//! # }
//! ```

pub mod attachment;
pub mod blackboard;
pub mod budget;
pub mod command;
pub mod compute;
pub mod config_store;
pub mod diag;
pub mod doc_index;
pub mod error;
pub mod event;
pub mod llm;
pub mod mcp;
pub mod model;
pub mod orchestrator;
pub mod plan;
pub mod room_log;
pub mod schedule;
pub mod secret;
pub mod session_store;
pub mod tool;
pub mod tools;
pub mod world;

pub use attachment::{Attachment, AttachmentStore, GcReport, validate_attachment};
pub use blackboard::BlackboardNote;
pub use budget::BudgetPool;
pub use command::{
    ApprovalOutcome, CommandPolicy, CommandPolicyView, Decision, PendingCommand,
};
pub use config_store::{ConfigStore, LoadedSchedules};
pub use diag::open_log;
pub use error::{CoreError, CoreResult, ErrorPayload};
pub use event::CoreEvent;
pub use llm::{
    BackendFactory, EchoBackend, FixedBackendFactory, HttpBackendFactory, HttpLlmBackend,
    LlmBackend, LlmConfig, LlmError, Provider,
};
pub use mcp::{McpConfig, McpManager, McpServerConfig, McpServerStatus};
pub use model::{
    AgentId, AgentMessage, AgentSnapshot, AgentSpec, AgentStatus, ConfigFileKind, CredentialSource,
    AgentRole, AgentRoleDefaults, AgentRoleId, RoleColor, Endpoint, ModelTemplate, ModelTemplateId,
    TopologyEdge,
};
pub use orchestrator::{AgentMcpStatus, AttachmentUpload, Orchestrator, OrchestratorConfig};
pub use schedule::{
    GRACE_MINUTES, InvalidRecurrence, Recurrence, ScheduledTask, Tick, Weekday, due_interval,
    due_wall_clock,
};
pub use secret::{InMemorySecretStore, KeyringSecretStore, SecretStore};
pub use session_store::{
    ForkPoint, Record, RestoredHistories, SessionMeta, SessionStore, SessionSummary,
};
pub use tool::{AgentTool, ToolContext, ToolRegistry};
pub use tools::run::resolve_program;
pub use tools::{
    DiffTool, FdTool, FileTool, GrepTool, RagTool, RememberTool, RunTool, SdTool, YqTool,
};
pub use world::World;
