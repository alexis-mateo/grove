use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::ProjectDirs;

/// Emplacements des fichiers de Grove.
///
/// Sous Linux : `~/.config/grove/` pour la configuration et
/// `~/.local/share/grove/` pour l'état. `GROVE_CONFIG_DIR` et
/// `GROVE_DATA_DIR` permettent de les remplacer (tests, plusieurs profils).
#[derive(Clone, Debug)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
}

impl Paths {
    pub fn from_env() -> Result<Self> {
        let dirs = ProjectDirs::from("", "", "grove").context("répertoire personnel introuvable")?;
        Ok(Self {
            config_dir: env_dir("GROVE_CONFIG_DIR").unwrap_or_else(|| dirs.config_dir().to_owned()),
            data_dir: env_dir("GROVE_DATA_DIR").unwrap_or_else(|| dirs.data_dir().to_owned()),
        })
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    pub fn state_file(&self) -> PathBuf {
        self.data_dir.join("state.json")
    }
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}
