use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Chemin du socket de l'application, passé aux PTY.
pub const ENV_SOCKET: &str = "GROVE_SOCKET";
/// Identifiant de l'onglet qui a lancé `claude`.
pub const ENV_TAB_ID: &str = "GROVE_TAB_ID";
/// Statusline de l'utilisateur, appelée en chaîne par `grove statusline`.
pub const ENV_USER_STATUSLINE: &str = "GROVE_USER_STATUSLINE";

/// Un message envoyé au socket : une ligne JSON par connexion.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub tab: String,
    pub kind: MessageKind,
    /// JSON reçu de Claude Code sur stdin, tel quel.
    pub payload: serde_json::Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Hook,
    Statusline,
    /// Demande de permission de la vue native (`grove mcp-permission`) : la
    /// connexion reste ouverte jusqu'à la décision ([`crate::Reply`]).
    Permission,
}

/// Socket propre à chaque instance de Grove. Supprime au passage les sockets
/// laissés par des instances qui n'ont pas quitté proprement.
pub fn socket_path(fallback_dir: &Path) -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| fallback_dir.to_owned());
    remove_stale_sockets(&dir);
    dir.join(format!("grove-{}.sock", std::process::id()))
}

fn remove_stale_sockets(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|n| n.strip_prefix("grove-")?.strip_suffix(".sock")?.parse::<u32>().ok())
        else {
            continue;
        };
        if !Path::new("/proc").join(pid.to_string()).exists() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
