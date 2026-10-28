//! Modèle de données, configuration et persistance de Grove.

pub mod config;
pub mod config_file;
pub mod ids;
pub mod layout;
pub mod model;
pub mod paths;
pub mod profile;
pub mod project_config;
pub mod state;

pub use config::{AddressConfig, ClaudeConfig, Config, FontConfig, GitHubConfig, ReviewConfig, TerminalConfig};
pub use config_file::ConfigFile;
pub use ids::{ProjectId, TabId, WorkspaceId};
pub use layout::{Axis, ClosedPane, Direction, Layout, Pane};
pub use model::{ClaudeView, Project, Tab, TabKind, Workspace, WorkspaceKind};
pub use paths::Paths;
pub use profile::{Launch, Profile, ProfileKind, ProfileSource, Reconnect, ResolvedProfile};
pub use project_config::{ProjectConfig, WorktreeConfig, WorktreeLocation};
pub use state::State;
