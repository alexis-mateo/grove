//! Liste des raccourcis clavier (specs §11.3), générée depuis les `KeyBinding`
//! enregistrés : elle ne peut pas diverger du comportement.

use gpui::KeyBinding;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Group {
    Tabs,
    Panes,
    Workspaces,
    Branches,
    Claude,
    Sidebar,
    Terminal,
    Grove,
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Group::Tabs => "Onglets",
            Group::Panes => "Volets (splits)",
            Group::Workspaces => "Projets et espaces de travail",
            Group::Branches => "Branches",
            Group::Claude => "Claude",
            Group::Sidebar => "Sidebar",
            Group::Terminal => "Terminal",
            Group::Grove => "Grove",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Shortcut {
    pub group: Group,
    pub description: &'static str,
    /// « Ctrl+Tab, Ctrl+PageDown » : toutes les touches de l'action.
    pub keys: String,
}

/// Groupe et description d'une action, par son nom (`grove::NewTab`).
fn describe(action: &str) -> Option<(Group, &'static str)> {
    Some(match action {
        "grove::NewTab" => (Group::Tabs, "Nouveau terminal (profil par défaut)"),
        "grove::CloseTab" => (Group::Tabs, "Fermer le volet actif, ou l'onglet s'il n'est pas partagé"),
        "grove::NextTab" => (Group::Tabs, "Onglet suivant"),
        "grove::PreviousTab" => (Group::Tabs, "Onglet précédent"),
        "grove::MoveTabLeft" => (Group::Tabs, "Déplacer l'onglet à gauche"),
        "grove::MoveTabRight" => (Group::Tabs, "Déplacer l'onglet à droite"),
        "grove::RenameTab" => (Group::Tabs, "Renommer l'onglet"),
        "grove::SplitRight" => (Group::Panes, "Partager : nouveau terminal à droite"),
        "grove::SplitDown" => (Group::Panes, "Partager : nouveau terminal en dessous"),
        "grove::FocusPaneLeft" => (Group::Panes, "Volet de gauche"),
        "grove::FocusPaneRight" => (Group::Panes, "Volet de droite"),
        "grove::FocusPaneUp" => (Group::Panes, "Volet du dessus"),
        "grove::FocusPaneDown" => (Group::Panes, "Volet du dessous"),
        "grove::AddProject" => (Group::Workspaces, "Ajouter un projet"),
        "grove::NewWorkspace" => (Group::Workspaces, "Nouvel espace de travail (worktree)"),
        "grove::OpenBranches" => (Group::Branches, "Sélecteur de branches"),
        "grove::NewClaudeTab" => (Group::Claude, "Nouvelle session Claude"),
        "grove::NextWaiting" => (Group::Claude, "Prochaine session qui attend une réponse"),
        "grove::ToggleSessionPanel" => (Group::Claude, "Panneau de session"),
        "grove::ShowHistory" => (Group::Claude, "Sessions passées de l'espace"),
        "grove::ToggleClaudeView" => (Group::Claude, "Basculer entre le terminal et la vue native"),
        "grove::ShowPullRequests" => (Group::Workspaces, "Pull requests GitHub"),
        "grove::ToggleSidebar" => (Group::Sidebar, "Afficher / masquer la sidebar"),
        "grove::OpenSettings" => (Group::Grove, "Paramètres"),
        "terminal::Copy" => (Group::Terminal, "Copier la sélection"),
        "terminal::Paste" => (Group::Terminal, "Coller"),
        "terminal::ScrollPageUp" => (Group::Terminal, "Remonter d'une page dans le scrollback"),
        "terminal::ScrollPageDown" => (Group::Terminal, "Descendre d'une page dans le scrollback"),
        _ => return None,
    })
}

/// « ctrl-shift-pageup » → « Ctrl+Shift+PageUp ».
pub(crate) fn display_keys(keys: &str) -> String {
    keys.split(' ')
        .map(|stroke| {
            // Le dernier élément est la touche ; `ctrl--` désigne la touche `-`.
            let (modifiers, key) = match stroke.strip_suffix("--") {
                Some(modifiers) => (modifiers, "-"),
                None => stroke.rsplit_once('-').unwrap_or(("", stroke)),
            };
            let mut parts: Vec<String> = modifiers
                .split('-')
                .filter(|m| !m.is_empty())
                .map(|m| match m {
                    "ctrl" => "Ctrl".to_owned(),
                    "shift" => "Shift".to_owned(),
                    "alt" => "Alt".to_owned(),
                    "cmd" | "super" | "win" => "Super".to_owned(),
                    other => other.to_owned(),
                })
                .collect();
            parts.push(match key {
                "pageup" => "PageUp".to_owned(),
                "pagedown" => "PageDown".to_owned(),
                "escape" => "Échap".to_owned(),
                "enter" => "Entrée".to_owned(),
                key if key.chars().count() == 1 => key.to_uppercase(),
                key => {
                    let mut chars = key.chars();
                    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
                }
            });
            parts.join("+")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Raccourcis de `bindings`, groupés, dans l'ordre d'enregistrement ; les touches
/// d'une même action sont réunies.
pub(crate) fn list(bindings: &[KeyBinding]) -> Vec<Shortcut> {
    let mut shortcuts: Vec<(String, Shortcut)> = Vec::new();
    for binding in bindings {
        let name = binding.action().name();
        let keys = display_keys(&binding.keystrokes().iter().map(|k| k.unparse()).collect::<Vec<_>>().join(" "));
        match shortcuts.iter_mut().find(|(action, _)| action == name) {
            Some((_, shortcut)) => {
                shortcut.keys.push_str(", ");
                shortcut.keys.push_str(&keys);
            }
            None => {
                let (group, description) = describe(name).unwrap_or((Group::Grove, "Action sans description"));
                shortcuts.push((name.to_owned(), Shortcut { group, description, keys }));
            }
        }
    }
    let mut shortcuts: Vec<_> = shortcuts.into_iter().map(|(_, s)| s).collect();
    shortcuts.sort_by_key(|s| s.group);
    shortcuts
}

/// Recherche par action ou par touche, sans casse ni accents de touche.
pub(crate) fn matches(shortcut: &Shortcut, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    let keys = shortcut.keys.to_lowercase();
    let compact = |text: &str| text.replace(['+', ' ', '-'], "");
    shortcut.description.to_lowercase().contains(&query)
        || shortcut.group.label().to_lowercase().contains(&query)
        || keys.contains(&query)
        || compact(&keys).contains(&compact(&query))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_binding_is_described() {
        for binding in crate::grove_view::key_bindings() {
            let name = binding.action().name();
            assert!(describe(name).is_some(), "raccourci sans description : {name}");
        }
    }

    #[test]
    fn keys_are_readable_and_grouped() {
        assert_eq!(display_keys("ctrl-shift-pageup"), "Ctrl+Shift+PageUp");
        assert_eq!(display_keys("ctrl-,"), "Ctrl+,");
        assert_eq!(display_keys("ctrl--"), "Ctrl+-");
        assert_eq!(display_keys("shift-tab"), "Shift+Tab");
        let shortcuts = list(&crate::grove_view::key_bindings());
        let next = shortcuts.iter().find(|s| s.description == "Onglet suivant").unwrap();
        assert_eq!(next.keys, "Ctrl+PageDown, Ctrl+Tab");
        assert!(shortcuts.windows(2).all(|w| w[0].group <= w[1].group));
    }

    #[test]
    fn search_by_action_or_key() {
        let shortcut = Shortcut { group: Group::Tabs, description: "Fermer l'onglet", keys: "Ctrl+Shift+W".into() };
        assert!(matches(&shortcut, "fermer"));
        assert!(matches(&shortcut, "ctrl+shift+w"));
        assert!(matches(&shortcut, "ctrl-shift-w"));
        assert!(matches(&shortcut, "onglets"));
        assert!(!matches(&shortcut, "claude"));
    }
}
