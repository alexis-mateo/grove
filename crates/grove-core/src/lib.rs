//! Modèle de données, configuration et persistance de Grove.

pub mod config;
pub mod ids;
pub mod model;
pub mod paths;
pub mod project_config;
pub mod state;

pub use config::{ClaudeConfig, Config};
pub use ids::{ProjectId, TabId, WorkspaceId};
pub use model::{ClaudeView, Project, Tab, TabKind, Workspace, WorkspaceKind};
pub use paths::Paths;
pub use project_config::{ProjectConfig, WorktreeConfig, WorktreeLocation};
pub use state::State;
