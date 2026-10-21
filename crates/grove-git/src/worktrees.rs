//! Worktrees git (specs §9) : liste, création, suppression, promotion.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::branches::git;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    /// `None` si HEAD est détachée.
    pub branch: Option<String>,
    /// Checkout principal du dépôt.
    pub main: bool,
    /// Le dossier n'existe plus (`git worktree prune` le retirerait).
    pub prunable: bool,
}

/// Worktrees du dépôt qui contient `repo`, checkout principal en premier.
pub fn list_worktrees(repo: &Path) -> Result<Vec<Worktree>> {
    let out = git(repo, &["worktree", "list", "--porcelain"])?;
    let mut worktrees = Vec::new();
    for block in out.split("\n\n") {
        let mut path = None;
        let mut branch = None;
        let mut prunable = false;
        let mut bare = false;
        for line in block.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                path = Some(PathBuf::from(p));
            } else if let Some(b) = line.strip_prefix("branch ") {
                branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).to_owned());
            } else if line.starts_with("prunable") {
                prunable = true;
            } else if line == "bare" {
                bare = true;
            }
        }
        if let Some(path) = path.filter(|_| !bare) {
            let main = worktrees.is_empty();
            worktrees.push(Worktree { path, branch, main, prunable });
        }
    }
    Ok(worktrees)
}

/// Branche à extraire dans le nouveau worktree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NewWorktree {
    /// Nouvelle branche `name` créée depuis `base`.
    NewBranch { name: String, base: String },
    /// Branche locale existante.
    Existing(String),
    /// Branche distante (`origin/x`) : crée la branche locale `x` qui la suit.
    Remote(String),
}

impl NewWorktree {
    /// Nom de la branche locale extraite.
    pub fn branch(&self) -> &str {
        match self {
            NewWorktree::NewBranch { name, .. } | NewWorktree::Existing(name) => name,
            NewWorktree::Remote(remote) => remote.split_once('/').map_or(remote.as_str(), |(_, name)| name),
        }
    }
}

pub fn add_worktree(repo: &Path, path: &Path, what: &NewWorktree) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("création de {}", parent.display()))?;
    }
    let path = path.to_string_lossy();
    match what {
        NewWorktree::NewBranch { name, base } => {
            git(repo, &["check-ref-format", "--branch", name])
                .map_err(|_| anyhow::anyhow!("nom de branche invalide : {name}"))?;
            git(repo, &["worktree", "add", "-b", name, &path, base])?
        }
        NewWorktree::Existing(name) => git(repo, &["worktree", "add", &path, name])?,
        NewWorktree::Remote(remote) => git(repo, &["worktree", "add", "--track", "-b", what.branch(), &path, remote])?,
    };
    Ok(())
}

/// Supprime le worktree. Sans `force`, git refuse s'il reste des modifications.
pub fn remove_worktree(repo: &Path, path: &Path, force: bool) -> Result<()> {
    let path = path.to_string_lossy();
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&path);
    git(repo, &args)?;
    Ok(())
}

/// Ajoute `pattern` à `info/exclude` du dépôt s'il n'y est pas déjà.
pub fn exclude(common_dir: &Path, pattern: &str) -> Result<()> {
    let file = common_dir.join("info").join("exclude");
    let current = std::fs::read_to_string(&file).unwrap_or_default();
    if current.lines().any(|l| l.trim() == pattern) {
        return Ok(());
    }
    std::fs::create_dir_all(file.parent().expect("info/"))?;
    let separator = if current.is_empty() || current.ends_with('\n') { "" } else { "\n" };
    std::fs::write(&file, format!("{current}{separator}{pattern}\n"))?;
    Ok(())
}

/// Met de côté les modifications (fichiers non suivis compris). `false` s'il n'y avait rien.
pub fn stash_push(workdir: &Path, message: &str) -> Result<bool> {
    let before = git(workdir, &["stash", "list"])?.lines().count();
    git(workdir, &["stash", "push", "--include-untracked", "-m", message])?;
    Ok(git(workdir, &["stash", "list"])?.lines().count() > before)
}

/// Réapplique le dernier stash. La pile est partagée entre les worktrees d'un dépôt.
pub fn stash_pop(workdir: &Path) -> Result<()> {
    git(workdir, &["stash", "pop"])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    fn run(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?} : {}", String::from_utf8_lossy(&out.stderr));
    }

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("app");
        std::fs::create_dir(&root).unwrap();
        run(&root, &["init", "-q"]);
        std::fs::write(root.join("a.txt"), "a").unwrap();
        run(&root, &["add", "."]);
        run(&root, &["commit", "-q", "-m", "init"]);
        dir
    }

    #[test]
    fn add_list_remove() {
        let dir = repo();
        let root = dir.path().join("app");
        let path = dir.path().join("app.worktrees/feat-1");

        let what = NewWorktree::NewBranch { name: "feat/1".into(), base: "main".into() };
        add_worktree(&root, &path, &what).unwrap();
        assert!(path.join("a.txt").exists());

        let list = list_worktrees(&root).unwrap();
        assert_eq!(list.len(), 2);
        assert!(list[0].main);
        assert_eq!(list[1].branch.as_deref(), Some("feat/1"));
        assert_eq!(list[1].path.canonicalize().unwrap(), path.canonicalize().unwrap());

        // Une modification bloque la suppression sans `force`.
        std::fs::write(path.join("a.txt"), "changé").unwrap();
        assert!(remove_worktree(&root, &path, false).is_err());
        remove_worktree(&root, &path, true).unwrap();
        assert_eq!(list_worktrees(&root).unwrap().len(), 1);
    }

    #[test]
    fn promote_moves_changes_into_new_worktree() {
        let dir = repo();
        let root = dir.path().join("app");
        std::fs::write(root.join("a.txt"), "travail en cours").unwrap();
        std::fs::write(root.join("nouveau.txt"), "non suivi").unwrap();

        assert!(stash_push(&root, "grove").unwrap());
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "a");
        let path = dir.path().join("app.worktrees/wip");
        add_worktree(&root, &path, &NewWorktree::NewBranch { name: "wip".into(), base: "HEAD".into() }).unwrap();
        stash_pop(&path).unwrap();

        assert_eq!(std::fs::read_to_string(path.join("a.txt")).unwrap(), "travail en cours");
        assert!(path.join("nouveau.txt").exists());
        assert!(!root.join("nouveau.txt").exists());
        // Rien à mettre de côté : pas de stash créé.
        assert!(!stash_push(&root, "grove").unwrap());
    }

    #[test]
    fn exclude_is_idempotent() {
        let dir = repo();
        let common = dir.path().join("app/.git");
        exclude(&common, "/.worktrees/").unwrap();
        exclude(&common, "/.worktrees/").unwrap();
        let content = std::fs::read_to_string(common.join("info/exclude")).unwrap();
        assert_eq!(content.matches("/.worktrees/").count(), 1);
    }

    #[test]
    fn remote_branch_name() {
        assert_eq!(NewWorktree::Remote("origin/feat/x".into()).branch(), "feat/x");
    }
}
