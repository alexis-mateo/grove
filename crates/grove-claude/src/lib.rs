//! Intégration de Claude Code en mode terminal (specs §8).
//!
//! `claude` tourne dans un PTY avec `--settings <fichier Grove>`. Ce fichier
//! déclare des hooks et une statusline qui appellent `grove hook` /
//! `grove statusline` ; ces commandes transmettent le JSON reçu au socket Unix
//! de l'application ([`EventServer`]), qui en déduit l'état de chaque session.
//!
//! En vue native (specs §10), `claude -p` tourne en stream-json ([`NativeProcess`]) ;
//! son flux est réduit en fil de conversation ([`Conversation`]).

mod client;
mod native;
mod permission;
mod protocol;
mod server;
mod session;
mod settings;
mod stream;
pub mod transcripts;
pub mod usage_api;

pub use client::{run_hook, run_statusline};
pub use native::{NativeEvent, NativeOptions, NativeProcess, PermissionPrompt};
pub use protocol::{ENV_SOCKET, ENV_TAB_ID, ENV_USER_STATUSLINE, Message, MessageKind, socket_path};
pub use permission::{decision as permission_decision, mcp_config, permission_tool, run_mcp_server};
pub use server::{EventServer, Reply};
pub use session::{
    ClaudeStatus, NeedsInputReason, RateLimits, RateWindow, Session, SessionMetrics, Transition,
};
pub use settings::{user_statusline_command, write_settings};
pub use stream::{Conversation, Entry, ToolCall, ToolResult};
