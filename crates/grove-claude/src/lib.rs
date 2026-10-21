//! Intégration de Claude Code en mode terminal (specs §8).
//!
//! `claude` tourne dans un PTY avec `--settings <fichier Grove>`. Ce fichier
//! déclare des hooks et une statusline qui appellent `grove hook` /
//! `grove statusline` ; ces commandes transmettent le JSON reçu au socket Unix
//! de l'application ([`EventServer`]), qui en déduit l'état de chaque session.

mod client;
mod protocol;
mod server;
mod session;
mod settings;
pub mod transcripts;
pub mod usage_api;

pub use client::{run_hook, run_statusline};
pub use protocol::{ENV_SOCKET, ENV_TAB_ID, ENV_USER_STATUSLINE, Message, MessageKind, socket_path};
pub use server::EventServer;
pub use session::{
    ClaudeStatus, NeedsInputReason, RateLimits, RateWindow, Session, SessionMetrics, Transition,
};
pub use settings::{user_statusline_command, write_settings};
