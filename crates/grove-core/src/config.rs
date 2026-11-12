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
    pub github: GitHubConfig,
}

/// Actions Claude sur les pull requests (specs §12.1), surchargeables dans `.grove.toml`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GitHubConfig {
    pub review: ReviewConfig,
    pub address: AddressConfig,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewConfig {
    /// Pour reviewer une PR : un skill (`/review`), lancé avec l'URL de
    /// la PR en argument, ou un prompt libre, suivi de l'URL.
    #[serde(alias = "skill")]
    pub prompt: String,
    /// Ajouté quand l'auteur a poussé depuis ma dernière review.
    pub refresh_prompt: String,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            prompt: "Fais la review de cette pull request : lis sa description et son diff, puis signale les bugs, les risques et ce qui manque (tests, documentation), avec fichier:ligne. Ne poste rien sur GitHub sans mon accord.".into(),
            refresh_prompt: "L'auteur a répondu ou poussé de nouveaux commits, mets la review à jour.".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AddressConfig {
    /// Pour traiter les commentaires reçus sur mes PR : skill ou prompt libre,
    /// suivi de l'URL et des fils qui attendent ma réponse.
    #[serde(alias = "skill")]
    pub prompt: String,
}

impl Default for AddressConfig {
    fn default() -> Self {
        Self {
            prompt: "Analyse les commentaires de review de cette PR et applique les corrections pertinentes. \
                     Propose ensuite dans le chat une réponse concise en anglais pour chaque fil. \
                     Ne poste rien sur GitHub avant mon accord : quand je dis go, poste les réponses."
                .into(),
        }
    }
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
    /// Vue des nouveaux onglets Claude : `terminal` (TUI officiel) ou `native`.
    pub view: crate::ClaudeView,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self { command: "claude".into(), notifications: true, usage_api: false, view: crate::ClaudeView::Terminal }
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
    /// Profil des nouveaux onglets (`Ctrl+Shift+T`, `+`). Par défaut : le shell.
    pub default_profile: Option<String>,
    pub profiles: Vec<crate::Profile>,
    /// Commande qui ouvre un fichier cliqué dans un terminal (`Ctrl+clic`), lancée
    /// par `sh -c` : `{path}`, `{line}`, `{column}` y sont remplacés. Exemple :
    /// `code -g {path}:{line}:{column}`. Par défaut : l'application du système.
    pub open_files_with: Option<String>,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            shell: None,
            scrollback: 10_000,
            default_profile: None,
            profiles: Vec::new(),
            open_files_with: None,
        }
    }
}

impl TerminalConfig {
    /// Commande `sh -c` qui ouvre `path`, ou `None` sans [`TerminalConfig::open_files_with`].
    /// Sans ligne (ou colonne), `:{line}` (`:{column}`) est retiré ; ailleurs, il vaut 1.
    pub fn open_file_command(&self, path: &Path, line: Option<u32>, column: Option<u32>) -> Option<String> {
        let template = self.open_files_with.as_deref().map(str::trim).filter(|t| !t.is_empty())?;
        let mut command = template.to_owned();
        if column.is_none() || line.is_none() {
            command = command.replace(":{column}", "");
        }
        if line.is_none() {
            command = command.replace(":{line}", "");
        }
        let quoted = format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"));
        Some(
            command
                .replace("{path}", &quoted)
                .replace("{line}", &line.unwrap_or(1).to_string())
                .replace("{column}", &column.unwrap_or(1).to_string()),
        )
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
    fn open_file_command() {
        let mut terminal = TerminalConfig::default();
        let path = Path::new("/tmp/l'été.rs");
        assert_eq!(terminal.open_file_command(path, Some(3), None), None);

        terminal.open_files_with = Some("code -g {path}:{line}:{column}".into());
        assert_eq!(terminal.open_file_command(path, Some(3), Some(7)).unwrap(), r"code -g '/tmp/l'\''été.rs':3:7");
        assert_eq!(terminal.open_file_command(path, Some(3), None).unwrap(), r"code -g '/tmp/l'\''été.rs':3");
        assert_eq!(terminal.open_file_command(path, None, None).unwrap(), r"code -g '/tmp/l'\''été.rs'");

        terminal.open_files_with = Some("vim +{line} {path}".into());
        assert_eq!(terminal.open_file_command(Path::new("/a"), None, None).unwrap(), "vim +1 '/a'");
        terminal.open_files_with = Some("  ".into());
        assert_eq!(terminal.open_file_command(path, None, None), None);
    }

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

    #[test]
    fn github_prompts_have_defaults() {
        // `skill`, l'ancien nom, est encore lu.
        let config: Config = toml::from_str("[github.address]\nskill = \"/fix\"\n").unwrap();
        assert_eq!(config.github.address.prompt, "/fix");
        assert!(config.github.review.prompt.starts_with("Fais la review"));
    }
}
