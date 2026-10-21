use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ids::{ProjectId, TabId, WorkspaceId};

/// Un projet : un dossier racine (repo git ou non) et ses espaces de travail.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub root: PathBuf,
    pub workspaces: Vec<Workspace>,
    /// Replié dans la sidebar.
    #[serde(default)]
    pub collapsed: bool,
    /// Worktrees retirés de Grove par l'utilisateur : l'import automatique les ignore.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignored_worktrees: Vec<PathBuf>,
}

impl Project {
    /// Crée un projet pour `root`, avec son espace de travail direct.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            id: ProjectId::new(),
            name: dir_name(&root),
            workspaces: vec![Workspace::direct(root.clone())],
            root,
            collapsed: false,
            ignored_worktrees: Vec::new(),
        }
    }

    pub fn workspace(&self, id: WorkspaceId) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| w.id == id)
    }

    pub fn workspace_mut(&mut self, id: WorkspaceId) -> Option<&mut Workspace> {
        self.workspaces.iter_mut().find(|w| w.id == id)
    }

    /// Ajoute un espace de travail worktree, ou renvoie celui qui pointe déjà sur `path`.
    pub fn add_worktree(&mut self, path: impl Into<PathBuf>, branch: &str, base: &str) -> WorkspaceId {
        let path = path.into();
        if let Some(existing) = self.workspaces.iter().find(|w| w.path == path) {
            return existing.id;
        }
        let workspace = Workspace {
            kind: WorkspaceKind::Worktree { branch: branch.to_owned(), base: base.to_owned() },
            ..Workspace::direct(path)
        };
        let id = workspace.id;
        self.workspaces.push(workspace);
        id
    }

    /// Retire un espace de travail. L'espace direct ne peut pas être retiré.
    pub fn remove_workspace(&mut self, id: WorkspaceId) -> Option<Workspace> {
        let index = self.workspaces.iter().position(|w| w.id == id)?;
        if self.workspaces[index].kind == WorkspaceKind::Direct {
            return None;
        }
        Some(self.workspaces.remove(index))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub path: PathBuf,
    pub kind: WorkspaceKind,
    pub tabs: Vec<Tab>,
    pub active_tab: Option<TabId>,
}

impl Workspace {
    pub fn direct(path: impl Into<PathBuf>) -> Self {
        Self {
            id: WorkspaceId::new(),
            path: path.into(),
            kind: WorkspaceKind::Direct,
            tabs: Vec::new(),
            active_tab: None,
        }
    }

    /// Ajoute un onglet terminal, le rend actif et renvoie son id.
    pub fn add_terminal_tab(&mut self) -> TabId {
        self.add_tab(TabKind::Terminal)
    }

    /// Ajoute un onglet Claude ; `session_id` pour reprendre une session existante.
    pub fn add_claude_tab(&mut self, session_id: Option<String>) -> TabId {
        self.add_tab(TabKind::Claude { session_id })
    }

    fn add_tab(&mut self, kind: TabKind) -> TabId {
        let tab = Tab { id: TabId::new(), title: None, kind };
        let id = tab.id;
        self.tabs.push(tab);
        self.active_tab = Some(id);
        id
    }

    /// Ferme un onglet. Si c'était l'onglet actif, active son voisin.
    pub fn close_tab(&mut self, id: TabId) -> Option<Tab> {
        let index = self.tabs.iter().position(|t| t.id == id)?;
        let tab = self.tabs.remove(index);
        if self.active_tab == Some(id) {
            self.active_tab = self
                .tabs
                .get(index)
                .or_else(|| self.tabs.last())
                .map(|t| t.id);
        }
        Some(tab)
    }

    /// Renomme un onglet. Un titre vide rend la main au titre défini par le programme.
    pub fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    pub fn rename_tab(&mut self, id: TabId, title: &str) -> bool {
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) else {
            return false;
        };
        let title = title.trim();
        tab.title = (!title.is_empty()).then(|| title.to_owned());
        true
    }

    /// Déplace un onglet à la position `to` (bornée à la fin de la liste).
    pub fn move_tab(&mut self, id: TabId, to: usize) -> bool {
        let Some(from) = self.tabs.iter().position(|t| t.id == id) else {
            return false;
        };
        let tab = self.tabs.remove(from);
        self.tabs.insert(to.min(self.tabs.len()), tab);
        true
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkspaceKind {
    /// Checkout principal : on travaille directement dans le dossier.
    Direct,
    /// git worktree dédié.
    Worktree { branch: String, base: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tab {
    pub id: TabId,
    /// Titre choisi par l'utilisateur ; sinon l'UI affiche celui du programme.
    pub title: Option<String>,
    pub kind: TabKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TabKind {
    Terminal,
    /// CLI `claude` dans un PTY. `session_id` est connu après `SessionStart`
    /// et sert à reprendre la session au redémarrage (`--resume`).
    Claude { session_id: Option<String> },
}

fn dir_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_project_has_a_direct_workspace() {
        let project = Project::new("/home/me/code/grove");
        assert_eq!(project.name, "grove");
        assert_eq!(project.workspaces.len(), 1);
        assert_eq!(project.workspaces[0].kind, WorkspaceKind::Direct);
        assert_eq!(project.workspaces[0].path, project.root);
    }

    #[test]
    fn worktrees_are_added_once_and_direct_stays() {
        let mut project = Project::new("/code/app");
        let a = project.add_worktree("/code/app.worktrees/x", "x", "main");
        assert_eq!(project.add_worktree("/code/app.worktrees/x", "x", "main"), a);
        assert_eq!(project.workspaces.len(), 2);
        let direct = project.workspaces[0].id;
        assert!(project.remove_workspace(direct).is_none());
        assert!(project.remove_workspace(a).is_some());
        assert_eq!(project.workspaces.len(), 1);
    }

    #[test]
    fn closing_active_tab_activates_neighbour() {
        let mut ws = Workspace::direct("/tmp");
        let a = ws.add_terminal_tab();
        let b = ws.add_terminal_tab();
        let c = ws.add_terminal_tab();

        ws.active_tab = Some(b);
        ws.close_tab(b);
        assert_eq!(ws.active_tab, Some(c));

        ws.close_tab(c);
        assert_eq!(ws.active_tab, Some(a));

        ws.close_tab(a);
        assert_eq!(ws.active_tab, None);
    }

    #[test]
    fn rename_trims_and_empty_resets() {
        let mut ws = Workspace::direct("/tmp");
        let a = ws.add_terminal_tab();
        assert!(ws.rename_tab(a, "  build  "));
        assert_eq!(ws.tabs[0].title.as_deref(), Some("build"));
        assert!(ws.rename_tab(a, " "));
        assert_eq!(ws.tabs[0].title, None);
    }

    #[test]
    fn move_tab_reorders() {
        let mut ws = Workspace::direct("/tmp");
        let a = ws.add_terminal_tab();
        let b = ws.add_terminal_tab();
        let c = ws.add_terminal_tab();

        assert!(ws.move_tab(c, 0));
        let order: Vec<_> = ws.tabs.iter().map(|t| t.id).collect();
        assert_eq!(order, [c, a, b]);

        assert!(ws.move_tab(c, 99));
        let order: Vec<_> = ws.tabs.iter().map(|t| t.id).collect();
        assert_eq!(order, [a, b, c]);
    }
}
