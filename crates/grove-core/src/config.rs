use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Configuration globale (`~/.config/grove/config.toml`).
/// Toutes les clés sont optionnelles.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub font: FontConfig,
    pub terminal: TerminalConfig,
    pub claude: ClaudeConfig,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeConfig {
    /// Commande lancée dans les onglets Claude, via le shell interactif
    /// (pour garder alias et fonctions de l'utilisateur).
    pub command: String,
    /// Notifications système quand une session attend ou termine un tour.
    pub notifications: bool,
    /// Lit l'usage du compte (limites 5 h / semaine) via l'endpoint interne
    /// d'Anthropic avec le jeton de Claude Code, toutes les 3 min. Hors des
    /// points d'intégration officiels : désactivé par défaut (specs §8.5).
    pub usage_api: bool,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self { command: "claude".into(), notifications: true, usage_api: false }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontConfig {
    pub family: String,
    pub size: f32,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "monospace".into(),
            size: 14.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    /// Shell lancé dans les onglets. Par défaut : `$SHELL`, sinon `/bin/sh`.
    pub shell: Option<String>,
    pub scrollback: usize,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            shell: None,
            scrollback: 10_000,
        }
    }
}

impl Config {
    /// Charge la configuration. Un fichier absent donne la configuration par défaut.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("lecture de {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("lecture de {}", path.display())),
        }
    }

    pub fn shell(&self) -> String {
        self.terminal
            .shell
            .clone()
            .or_else(|| std::env::var("SHELL").ok().filter(|s| !s.is_empty()))
            .unwrap_or_else(|| "/bin/sh".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::load(&dir.path().join("config.toml")).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn partial_file_keeps_other_defaults() {
        let config: Config = toml::from_str("[font]\nsize = 16.0\n").unwrap();
        assert_eq!(config.font.size, 16.0);
        assert_eq!(config.font.family, "monospace");
        assert_eq!(config.terminal.scrollback, 10_000);
    }
}
