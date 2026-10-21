//! Lecture git (via `gix`), surveillance de `HEAD` et branches (CLI `git`).

mod branches;
mod worktrees;

pub use branches::{Branch, CheckoutStatus, checkout_status, create, delete, list_branches, switch};
pub use worktrees::{
    NewWorktree, Worktree, add_worktree, exclude, list_worktrees, remove_worktree, stash_pop, stash_push,
};

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

/// Dépôt contenant un dossier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repo {
    /// Dossier `.git` de ce checkout (propre à chaque worktree).
    pub git_dir: PathBuf,
    /// Dossier `.git` partagé par tous les worktrees : sert à les regrouper.
    pub common_dir: PathBuf,
}

/// Ce sur quoi pointe `HEAD`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Head {
    /// Branche courante (y compris une branche sans commit).
    Branch(String),
    /// HEAD détachée : hash abrégé.
    Detached(String),
}

impl Head {
    pub fn label(&self) -> &str {
        match self {
            Head::Branch(name) | Head::Detached(name) => name,
        }
    }
}

/// Cherche le dépôt qui contient `path` (en remontant les dossiers parents).
pub fn discover(path: &Path) -> Option<Repo> {
    let repo = gix::discover(path).ok()?;
    Some(Repo {
        git_dir: repo.git_dir().to_owned(),
        common_dir: repo.common_dir().to_owned(),
    })
}

pub fn read_head(git_dir: &Path) -> Result<Head> {
    let repo = gix::open(git_dir).with_context(|| format!("ouverture de {}", git_dir.display()))?;
    if let Some(name) = repo.head_name()? {
        return Ok(Head::Branch(name.shorten().to_string()));
    }
    let id = repo.head_id().context("HEAD sans branche ni commit")?;
    Ok(Head::Detached(id.shorten_or_id().to_string()))
}

/// Surveille `HEAD` et appelle `on_change` quand elle change (checkout, switch…).
/// La surveillance s'arrête quand la valeur est détruite.
pub struct HeadWatcher {
    _watcher: RecommendedWatcher,
}

impl HeadWatcher {
    pub fn new(git_dir: &Path, on_change: impl Fn(Head) + Send + 'static) -> Result<Self> {
        let dir = git_dir.to_owned();
        let last = Mutex::new(read_head(&dir).ok());
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else { return };
            if !event.paths.iter().any(|p| p.file_name().is_some_and(|n| n == "HEAD")) {
                return;
            }
            let head = match read_head(&dir) {
                Ok(head) => head,
                Err(err) => return log::debug!("lecture de HEAD : {err:#}"),
            };
            let mut last = last.lock().unwrap();
            if last.as_ref() != Some(&head) {
                *last = Some(head.clone());
                on_change(head);
            }
        })?;
        // git remplace HEAD par renommage d'un fichier verrou : on surveille le dossier.
        watcher.watch(git_dir, RecursiveMode::NonRecursive)?;
        Ok(Self { _watcher: watcher })
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(status.status.success(), "git {args:?} : {}", String::from_utf8_lossy(&status.stderr));
    }

    #[test]
    fn plain_folder_is_not_a_repo() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(discover(dir.path()), None);
    }

    #[test]
    fn reads_branch_and_detached_head() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();

        let repo = discover(&sub).unwrap();
        assert_eq!(read_head(&repo.git_dir).unwrap(), Head::Branch("main".into()));

        git(dir.path(), &["commit", "-q", "--allow-empty", "-m", "x"]);
        git(dir.path(), &["checkout", "-q", "--detach"]);
        assert!(matches!(read_head(&repo.git_dir).unwrap(), Head::Detached(_)));
    }

    #[test]
    fn watcher_sees_branch_switch() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["commit", "-q", "--allow-empty", "-m", "x"]);
        let repo = discover(dir.path()).unwrap();

        let (tx, rx) = mpsc::channel();
        let _watcher = HeadWatcher::new(&repo.git_dir, move |head| {
            let _ = tx.send(head);
        })
        .unwrap();

        git(dir.path(), &["switch", "-q", "-c", "feat-1"]);
        let head = rx.recv_timeout(Duration::from_secs(5)).expect("pas de notification");
        assert_eq!(head, Head::Branch("feat-1".into()));
    }
}
