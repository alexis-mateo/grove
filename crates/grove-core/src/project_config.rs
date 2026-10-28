//! Configuration d'un projet : `<projet>/.grove.toml`, optionnelle et versionnable.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const FILE_NAME: &str = ".grove.toml";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectConfig {
    pub worktree: WorktreeConfig,
    pub terminal: ProjectTerminalConfig,
    pub github: ProjectGitHubConfig,
}

/// Skills ou prompts propres au projet, à la place de ceux de `config.toml` (specs §12.1).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectGitHubConfig {
    pub review: ProjectSkill,
    pub address: ProjectSkill,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectSkill {
    #[serde(alias = "skill")]
    pub prompt: Option<String>,
}

/// Profils de terminal propres au projet, partagés avec l'équipe (specs §11.1).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectTerminalConfig {
    pub profiles: Vec<crate::Profile>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorktreeConfig {
    pub location: WorktreeLocation,
    /// Commandes lancées dans le nouveau worktree après sa création.
    /// `{{repo}}` est remplacé par le nom du dossier du projet.
    pub setup: Vec<String>,
    /// Fichiers copiés depuis le checkout principal (`.env`…), avant le setup.
    pub copy: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeLocation {
    /// `../<repo>.worktrees/<branche>`
    #[default]
    Sibling,
    /// `<repo>/.worktrees/<branche>`, ajouté à `.git/info/exclude`.
    Inside,
}

impl ProjectConfig {
    /// Charge `.grove.toml` à la racine du projet. Absent : configuration par défaut.
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join(FILE_NAME);
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("lecture de {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("lecture de {}", path.display())),
        }
    }
}

impl WorktreeConfig {
    /// Dossier du worktree de `branch` pour le projet `root`.
    pub fn path_for(&self, root: &Path, branch: &str) -> PathBuf {
        // `feat/x` donnerait un sous-dossier : on aplatit.
        let dir = branch.replace('/', "-");
        match self.location {
            WorktreeLocation::Sibling => {
                let repo = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                root.parent().unwrap_or(root).join(format!("{repo}.worktrees")).join(dir)
            }
            WorktreeLocation::Inside => root.join(".worktrees").join(dir),
        }
    }

    /// Commandes de setup, `{{repo}}` remplacé.
    pub fn setup_commands(&self, root: &Path) -> Vec<String> {
        let repo = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.setup.iter().map(|c| c.replace("{{repo}}", &repo)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_spec_example() {
        let config: ProjectConfig = toml::from_str(
            r#"
            [worktree]
            location = "inside"
            setup = ["cp ../{{repo}}/.env .env", "npm install"]
            copy = [".env", ".env.local"]
            "#,
        )
        .unwrap();
        assert_eq!(config.worktree.location, WorktreeLocation::Inside);
        assert_eq!(
            config.worktree.setup_commands(Path::new("/code/app")),
            ["cp ../app/.env .env", "npm install"]
        );
        assert_eq!(config.worktree.copy, [".env", ".env.local"]);
    }

    #[test]
    fn worktree_paths() {
        let root = Path::new("/code/app");
        let sibling = WorktreeConfig::default();
        assert_eq!(sibling.path_for(root, "feat/login"), Path::new("/code/app.worktrees/feat-login"));
        let inside = WorktreeConfig { location: WorktreeLocation::Inside, ..Default::default() };
        assert_eq!(inside.path_for(root, "fix-1"), Path::new("/code/app/.worktrees/fix-1"));
    }

    #[test]
    fn missing_file_is_default() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(ProjectConfig::load(dir.path()).unwrap(), ProjectConfig::default());
    }
}
