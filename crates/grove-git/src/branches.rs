//! Branches en mode direct : liste, état du checkout, switch, création,
//! suppression. Les opérations passent par la CLI `git` (specs §4).

use std::path::Path;
use std::process::Command;

use anyhow::{Result, bail};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    /// `main`, ou `origin/main` pour une branche distante.
    pub name: String,
    pub remote: bool,
    pub current: bool,
    /// Branche suivie (`origin/main`).
    pub upstream: Option<String>,
    /// Date du dernier commit, en secondes depuis l'epoch.
    pub committed_at: i64,
}

/// État du checkout : avance/retard sur la branche suivie, fichiers modifiés.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CheckoutStatus {
    pub ahead: u32,
    pub behind: u32,
    /// Fichiers modifiés, indexés ou non suivis.
    pub changed: u32,
    pub has_upstream: bool,
}

pub(crate) fn git(workdir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(["-c", "color.ui=false"])
        .args(args)
        .current_dir(workdir)
        .env("GIT_TERMINAL_PROMPT", "0")
        // Messages en anglais : certaines erreurs sont reconnues à leur texte.
        .env("LC_ALL", "C")
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{}", stderr.trim().trim_start_matches("error: ").trim_start_matches("fatal: "));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Branches locales puis distantes, de la plus récemment modifiée à la plus ancienne.
pub fn list_branches(workdir: &Path) -> Result<Vec<Branch>> {
    let out = git(
        workdir,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname)%00%(HEAD)%00%(upstream:short)%00%(committerdate:unix)",
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    let mut branches: Vec<Branch> = out
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\0');
            let refname = fields.next()?;
            let current = fields.next()? == "*";
            let upstream = fields.next().filter(|u| !u.is_empty()).map(str::to_owned);
            let committed_at = fields.next()?.parse().unwrap_or(0);
            let (name, remote) = if let Some(name) = refname.strip_prefix("refs/heads/") {
                (name, false)
            } else {
                let name = refname.strip_prefix("refs/remotes/")?;
                // `origin/HEAD` n'est qu'un alias.
                if name.ends_with("/HEAD") {
                    return None;
                }
                (name, true)
            };
            Some(Branch { name: name.to_owned(), remote, current, upstream, committed_at })
        })
        .collect();
    branches.sort_by_key(|b| b.remote);
    Ok(branches)
}

pub fn checkout_status(workdir: &Path) -> Result<CheckoutStatus> {
    let out = git(workdir, &["status", "--porcelain=v2", "--branch", "--untracked-files=normal"])?;
    let mut status = CheckoutStatus::default();
    for line in out.lines() {
        if let Some(ab) = line.strip_prefix("# branch.ab ") {
            status.has_upstream = true;
            for part in ab.split_whitespace() {
                if let Some(n) = part.strip_prefix('+') {
                    status.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix('-') {
                    status.behind = n.parse().unwrap_or(0);
                }
            }
        } else if !line.starts_with('#') && !line.is_empty() {
            status.changed += 1;
        }
    }
    Ok(status)
}

/// Passe sur `branch`. Une branche distante (`origin/x`) crée la branche locale `x` qui la suit.
pub fn switch(workdir: &Path, branch: &Branch) -> Result<()> {
    if branch.remote {
        let local = branch.name.split_once('/').map_or(branch.name.as_str(), |(_, name)| name);
        git(workdir, &["switch", "--track", "-c", local, &branch.name])?;
    } else {
        git(workdir, &["switch", &branch.name])?;
    }
    Ok(())
}

/// Crée `name` à partir de `base` (HEAD si `None`) et passe dessus.
pub fn create(workdir: &Path, name: &str, base: Option<&str>) -> Result<()> {
    let name = name.trim();
    git(workdir, &["check-ref-format", "--branch", name]).map_err(|_| anyhow::anyhow!("nom de branche invalide : {name}"))?;
    let mut args = vec!["switch", "-c", name];
    args.extend(base);
    git(workdir, &args)?;
    Ok(())
}

/// Supprime une branche locale. Sans `force`, git refuse si elle n'est pas fusionnée.
pub fn delete(workdir: &Path, name: &str, force: bool) -> Result<()> {
    git(workdir, &["branch", if force { "-D" } else { "-d" }, name])?;
    Ok(())
}

#[cfg(test)]
mod tests {
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
        run(dir.path(), &["init", "-q"]);
        run(dir.path(), &["commit", "-q", "--allow-empty", "-m", "init"]);
        dir
    }

    #[test]
    fn create_switch_list_delete() {
        let dir = repo();
        let path = dir.path();

        create(path, "feat-1", None).unwrap();
        let names: Vec<_> = list_branches(path).unwrap().into_iter().map(|b| (b.name, b.current)).collect();
        assert!(names.contains(&("feat-1".into(), true)));
        assert!(names.contains(&("main".into(), false)));

        let main = Branch { name: "main".into(), remote: false, current: false, upstream: None, committed_at: 0 };
        switch(path, &main).unwrap();
        delete(path, "feat-1", false).unwrap();
        assert_eq!(list_branches(path).unwrap().len(), 1);

        assert!(create(path, "nom invalide..", None).is_err());
    }

    #[test]
    fn unmerged_branch_needs_force() {
        let dir = repo();
        let path = dir.path();
        create(path, "wip", None).unwrap();
        run(path, &["commit", "-q", "--allow-empty", "-m", "wip"]);
        run(path, &["switch", "-q", "main"]);
        let err = delete(path, "wip", false).unwrap_err().to_string();
        assert!(err.contains("not fully merged"), "{err}");
        delete(path, "wip", true).unwrap();
    }

    #[test]
    fn status_counts_changes_and_tracking() {
        let dir = repo();
        let path = dir.path();
        std::fs::write(path.join("a.txt"), "x").unwrap();
        std::fs::write(path.join("b.txt"), "y").unwrap();
        let status = checkout_status(path).unwrap();
        assert_eq!(status, CheckoutStatus { changed: 2, ..Default::default() });

        // Branche suivie : un clone du dépôt, avec un commit d'avance.
        let clone = tempfile::tempdir().unwrap();
        run(clone.path(), &["clone", "-q", path.to_str().unwrap(), "."]);
        run(clone.path(), &["commit", "-q", "--allow-empty", "-m", "local"]);
        let status = checkout_status(clone.path()).unwrap();
        assert_eq!((status.ahead, status.behind, status.has_upstream), (1, 0, true));

        let branches = list_branches(clone.path()).unwrap();
        assert!(branches.iter().any(|b| b.remote && b.name == "origin/main"));
        assert!(!branches.iter().any(|b| b.name.ends_with("/HEAD")));
    }
}
