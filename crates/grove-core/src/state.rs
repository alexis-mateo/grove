use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::ids::{ProjectId, WorkspaceId};
use crate::model::{Project, Workspace};

/// État restauré au démarrage : projets, espaces de travail, onglets, sélection.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub projects: Vec<Project>,
    pub active_workspace: Option<(ProjectId, WorkspaceId)>,
    /// Largeur de la sidebar en pixels, si l'utilisateur l'a redimensionnée.
    pub sidebar_width: Option<f32>,
}

impl State {
    /// Charge l'état. Un fichier absent donne un état vide.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).with_context(|| format!("lecture de {}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("lecture de {}", path.display())),
        }
    }

    /// Écrit l'état de façon atomique (fichier temporaire puis renommage).
    pub fn save(&self, path: &Path) -> Result<()> {
        let dir = path.parent().context("chemin d'état sans dossier parent")?;
        std::fs::create_dir_all(dir).with_context(|| format!("création de {}", dir.display()))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("écriture de {}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| format!("écriture de {}", path.display()))?;
        Ok(())
    }

    /// Ajoute un projet pour `root`, ou renvoie celui qui existe déjà.
    pub fn add_project(&mut self, root: &Path) -> ProjectId {
        if let Some(project) = self.projects.iter().find(|p| p.root == root) {
            return project.id;
        }
        let project = Project::new(root);
        let id = project.id;
        if self.active_workspace.is_none() {
            self.active_workspace = Some((id, project.workspaces[0].id));
        }
        self.projects.push(project);
        id
    }

    /// Retire un projet de Grove (rien n'est supprimé sur le disque).
    pub fn remove_project(&mut self, id: ProjectId) -> Option<Project> {
        let index = self.projects.iter().position(|p| p.id == id)?;
        let project = self.projects.remove(index);
        if self.active_workspace.is_some_and(|(p, _)| p == id) {
            self.active_workspace = self
                .projects
                .get(index)
                .or_else(|| self.projects.last())
                .map(|p| (p.id, p.workspaces[0].id));
        }
        Some(project)
    }

    pub fn project(&self, id: ProjectId) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }

    pub fn workspace(&self, (project, workspace): (ProjectId, WorkspaceId)) -> Option<&Workspace> {
        self.project(project)?.workspace(workspace)
    }

    pub fn workspace_mut(&mut self, (project, workspace): (ProjectId, WorkspaceId)) -> Option<&mut Workspace> {
        self.projects
            .iter_mut()
            .find(|p| p.id == project)?
            .workspace_mut(workspace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/state.json");

        let mut state = State::default();
        let project = state.add_project(Path::new("/home/me/code/grove"));
        let active = state.active_workspace.unwrap();
        assert_eq!(active.0, project);
        state.workspace_mut(active).unwrap().add_terminal_tab();
        state.workspace_mut(active).unwrap().add_claude_tab(Some("abc".into()));

        state.save(&path).unwrap();
        assert_eq!(State::load(&path).unwrap(), state);
    }

    #[test]
    fn adding_same_root_twice_is_idempotent() {
        let mut state = State::default();
        let a = state.add_project(Path::new("/x"));
        let b = state.add_project(Path::new("/x"));
        assert_eq!(a, b);
        assert_eq!(state.projects.len(), 1);
    }

    #[test]
    fn removing_active_project_activates_another() {
        let mut state = State::default();
        let a = state.add_project(Path::new("/a"));
        let b = state.add_project(Path::new("/b"));
        assert_eq!(state.active_workspace.unwrap().0, a);
        state.remove_project(a);
        assert_eq!(state.active_workspace.unwrap().0, b);
        state.remove_project(b);
        assert_eq!(state.active_workspace, None);
    }

    #[test]
    fn missing_file_gives_empty_state() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(State::load(&dir.path().join("state.json")).unwrap(), State::default());
    }
}
