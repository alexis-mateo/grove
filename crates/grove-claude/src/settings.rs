//! Fichier passé à `claude --settings` : hooks et statusline de Grove.
//!
//! Il s'ajoute aux réglages de l'utilisateur sans les modifier : les hooks se
//! cumulent, mais la statusline de Grove remplace la sienne, d'où l'appel en
//! chaîne (cf. [`user_statusline_command`]).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};

/// Événements écoutés. Les outils prennent un `matcher`.
const EVENTS: &[(&str, bool)] = &[
    ("SessionStart", false),
    ("UserPromptSubmit", false),
    ("PreToolUse", true),
    ("PostToolUse", true),
    ("PostToolUseFailure", true),
    ("PermissionRequest", true),
    ("Notification", false),
    ("PostCompact", false),
    ("Stop", false),
    ("StopFailure", false),
    ("SessionEnd", false),
];

/// Écrit le fichier de réglages. `grove` : chemin de l'exécutable, qui sert
/// de client de hook (`grove hook`, `grove statusline`).
pub fn write_settings(path: &Path, grove: &Path) -> Result<()> {
    let exe = shell_quote(&grove.to_string_lossy());
    let hook = json!({ "type": "command", "command": format!("{exe} hook"), "timeout": 5 });

    let mut hooks = Map::new();
    for &(event, tool) in EVENTS {
        let mut group = json!({ "hooks": [hook.clone()] });
        if tool {
            group["matcher"] = json!("*");
        }
        hooks.insert(event.to_owned(), json!([group]));
    }

    let mut statusline = json!({ "type": "command", "command": format!("{exe} statusline") });
    // Reprend les options d'affichage de la statusline de l'utilisateur.
    if let Some(user) = user_statusline() {
        for key in ["padding", "refreshInterval"] {
            if let Some(value) = user.get(key) {
                statusline[key] = value.clone();
            }
        }
    }

    let settings = json!({ "hooks": hooks, "statusLine": statusline });
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("création de {}", dir.display()))?;
    }
    std::fs::write(path, serde_json::to_vec_pretty(&settings)?)
        .with_context(|| format!("écriture de {}", path.display()))
}

/// Commande de la statusline définie dans `~/.claude/settings.json`, s'il y en a une.
pub fn user_statusline_command() -> Option<String> {
    let statusline = user_statusline()?;
    if statusline.get("type").and_then(Value::as_str) != Some("command") {
        return None;
    }
    statusline.get("command").and_then(Value::as_str).map(str::to_owned)
}

fn user_statusline() -> Option<Value> {
    let path = claude_dir()?.join("settings.json");
    let settings: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    settings.get("statusLine").cloned()
}

/// `~/.claude` (ou `$CLAUDE_CONFIG_DIR`).
pub(crate) fn claude_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude"))
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_declare_hooks_and_statusline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("claude-settings.json");
        write_settings(&path, Path::new("/opt/my grove/grove")).unwrap();

        let v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(v["statusLine"]["command"], "'/opt/my grove/grove' statusline");
        assert_eq!(v["hooks"]["PreToolUse"][0]["matcher"], "*");
        assert!(v["hooks"]["Stop"][0].get("matcher").is_none());
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], "'/opt/my grove/grove' hook");
    }
}
