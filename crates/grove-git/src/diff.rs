//! Modifications du checkout (specs §9.5) : fichiers modifiés par rapport à
//! `HEAD` (indexés, non indexés, non suivis) et diff de chacun.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::branches::git;

/// Au-delà, le diff d'un fichier est tronqué : la vue reste fluide.
const MAX_LINES: usize = 20_000;
/// Un fichier non suivi plus gros n'est pas affiché.
const MAX_UNTRACKED_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileChange {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    /// Conflit de fusion non résolu.
    Conflicted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangedFile {
    /// Chemin depuis la racine du dépôt.
    pub path: String,
    /// Ancien chemin d'un fichier renommé.
    pub old_path: Option<String>,
    pub change: FileChange,
    /// Une partie des modifications est indexée.
    pub staged: bool,
    /// Lignes ajoutées et supprimées ; `None` : fichier binaire ou non suivi.
    pub added: Option<u32>,
    pub removed: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffLineKind {
    /// En-tête de bloc (`@@ -1,4 +1,5 @@ fn main`).
    Hunk,
    Context,
    Added,
    Removed,
    /// Remarque de git (`\ No newline at end of file`) ou de Grove.
    Note,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    /// Numéro de ligne dans l'ancienne version, puis dans la nouvelle.
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileDiff {
    pub lines: Vec<DiffLine>,
    /// Fichier binaire : pas de lignes à montrer.
    pub binary: bool,
}

/// Fichiers modifiés par rapport à `HEAD`, triés par chemin.
pub fn changed_files(workdir: &Path) -> Result<Vec<ChangedFile>> {
    let status = git(workdir, &["status", "--porcelain=v1", "-z", "--untracked-files=all"])?;
    let mut files = parse_status(&status);
    // Sans commit, `HEAD` n'existe pas : pas de compteurs.
    if let Ok(numstat) = git(workdir, &["diff", "HEAD", "--numstat", "-z", "--find-renames"]) {
        for (path, added, removed) in parse_numstat(&numstat) {
            if let Some(file) = files.iter_mut().find(|f| f.path == path) {
                (file.added, file.removed) = (added, removed);
            }
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// Diff d'un fichier par rapport à `HEAD`, modifications indexées comprises.
pub fn file_diff(workdir: &Path, file: &ChangedFile) -> Result<FileDiff> {
    if file.change == FileChange::Untracked {
        let root = PathBuf::from(git(workdir, &["rev-parse", "--show-toplevel"])?.trim());
        return untracked_diff(&root.join(&file.path));
    }
    // `:(top,literal)` : chemin depuis la racine, sans motifs.
    let mut paths = vec![format!(":(top,literal){}", file.path)];
    paths.extend(file.old_path.iter().map(|old| format!(":(top,literal){old}")));
    let mut args = vec!["diff", "HEAD", "--no-ext-diff", "--find-renames", "--"];
    args.extend(paths.iter().map(String::as_str));
    let out = match git(workdir, &args) {
        Ok(out) => out,
        // Pas encore de commit : ce qui est indexé.
        Err(_) => {
            args[1] = "--cached";
            git(workdir, &args)?
        }
    };
    Ok(parse_diff(&out))
}

/// Un fichier non suivi : tout est ajouté.
fn untracked_diff(path: &Path) -> Result<FileDiff> {
    let note = |text: &str| FileDiff {
        lines: vec![DiffLine { kind: DiffLineKind::Note, old: None, new: None, text: text.to_owned() }],
        binary: false,
    };
    let meta = std::fs::metadata(path)?;
    if meta.is_dir() {
        return Ok(note("Dossier (sous-module ou dépôt imbriqué)."));
    }
    if meta.len() > MAX_UNTRACKED_BYTES {
        return Ok(note("Fichier trop gros pour être affiché."));
    }
    let bytes = std::fs::read(path)?;
    if bytes.iter().take(8000).any(|b| *b == 0) {
        return Ok(FileDiff { lines: Vec::new(), binary: true });
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<DiffLine> = text
        .lines()
        .take(MAX_LINES)
        .enumerate()
        .map(|(i, line)| DiffLine { kind: DiffLineKind::Added, old: None, new: Some(i as u32 + 1), text: expand_tabs(line) })
        .collect();
    if text.lines().nth(MAX_LINES).is_some() {
        lines.push(truncated_note());
    }
    Ok(FileDiff { lines, binary: false })
}

fn truncated_note() -> DiffLine {
    DiffLine { kind: DiffLineKind::Note, old: None, new: None, text: format!("… diff tronqué après {MAX_LINES} lignes") }
}

fn expand_tabs(line: &str) -> String {
    line.replace('\t', "    ")
}

/// `git status --porcelain=v1 -z` : « XY chemin\0 », suivi de l'ancien chemin
/// pour un renommage ou une copie.
fn parse_status(out: &str) -> Vec<ChangedFile> {
    let mut entries = out.split('\0').filter(|e| !e.is_empty());
    let mut files = Vec::new();
    while let Some(entry) = entries.next() {
        if entry.len() < 4 {
            continue;
        }
        let (x, y) = (entry.as_bytes()[0], entry.as_bytes()[1]);
        let path = entry[3..].to_owned();
        let old_path = matches!(x, b'R' | b'C').then(|| entries.next().map(str::to_owned)).flatten();
        let change = match (x, y) {
            (b'!', b'!') => continue,
            (b'?', b'?') => FileChange::Untracked,
            (b'U', _) | (_, b'U') | (b'A', b'A') | (b'D', b'D') => FileChange::Conflicted,
            (b'R', _) | (_, b'R') => FileChange::Renamed,
            (b'A' | b'C', _) => FileChange::Added,
            (b'D', _) | (_, b'D') => FileChange::Deleted,
            _ => FileChange::Modified,
        };
        let staged = !matches!(x, b' ' | b'?');
        files.push(ChangedFile { path, old_path, change, staged, added: None, removed: None });
    }
    files
}

/// `git diff --numstat -z` : « ajoutées\tsupprimées\tchemin\0 », ou pour un
/// renommage « ajoutées\tsupprimées\t\0ancien\0nouveau\0 ». `-` : binaire.
fn parse_numstat(out: &str) -> Vec<(String, Option<u32>, Option<u32>)> {
    let mut entries = out.split('\0');
    let mut stats = Vec::new();
    while let Some(entry) = entries.next() {
        let mut fields = entry.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) = (fields.next(), fields.next(), fields.next()) else { continue };
        let path = if path.is_empty() {
            let _old = entries.next();
            match entries.next() {
                Some(new) => new.to_owned(),
                None => continue,
            }
        } else {
            path.to_owned()
        };
        stats.push((path, added.parse().ok(), removed.parse().ok()));
    }
    stats
}

/// Diff unifié de git → lignes numérotées. Les en-têtes de fichier sont omis.
pub(crate) fn parse_diff(out: &str) -> FileDiff {
    let mut diff = FileDiff::default();
    let (mut old, mut new) = (0u32, 0u32);
    let mut in_hunk = false;
    for line in out.lines() {
        if diff.lines.len() >= MAX_LINES {
            diff.lines.push(truncated_note());
            break;
        }
        if line.starts_with("diff --git ") {
            in_hunk = false;
            continue;
        }
        if line.starts_with("Binary files ") {
            diff.binary = true;
            continue;
        }
        if let Some(header) = line.strip_prefix("@@ ") {
            let mut ranges = header.split(' ');
            let start = |range: Option<&str>, sign: char| {
                range.and_then(|r| r.strip_prefix(sign)).and_then(|r| r.split(',').next()).and_then(|n| n.parse().ok())
            };
            old = start(ranges.next(), '-').unwrap_or(1);
            new = start(ranges.next(), '+').unwrap_or(1);
            in_hunk = true;
            diff.lines.push(DiffLine { kind: DiffLineKind::Hunk, old: None, new: None, text: line.to_owned() });
            continue;
        }
        if !in_hunk {
            continue;
        }
        let (kind, rest) = match line.chars().next() {
            Some('+') => (DiffLineKind::Added, &line[1..]),
            Some('-') => (DiffLineKind::Removed, &line[1..]),
            Some(' ') => (DiffLineKind::Context, &line[1..]),
            Some('\\') => (DiffLineKind::Note, line.trim_start_matches("\\ ")),
            // Ligne vide de contexte dont l'espace a été retiré.
            None => (DiffLineKind::Context, ""),
            Some(_) => continue,
        };
        let (old_no, new_no) = match kind {
            DiffLineKind::Added => (None, Some(new)),
            DiffLineKind::Removed => (Some(old), None),
            DiffLineKind::Context => (Some(old), Some(new)),
            _ => (None, None),
        };
        match kind {
            DiffLineKind::Added => new += 1,
            DiffLineKind::Removed => old += 1,
            DiffLineKind::Context => {
                old += 1;
                new += 1;
            }
            _ => {}
        }
        diff.lines.push(DiffLine { kind, old: old_no, new: new_no, text: expand_tabs(rest) });
    }
    diff
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

    #[test]
    fn parses_hunks_with_line_numbers() {
        let diff = parse_diff(concat!(
            "diff --git a/a.rs b/a.rs\n",
            "index 1..2 100644\n",
            "--- a/a.rs\n",
            "+++ b/a.rs\n",
            "@@ -3,3 +3,3 @@ fn main() {\n",
            " un\n",
            "-deux\n",
            "+2\n",
            " \ttrois\n",
            "\n",
            "\\ No newline at end of file\n",
        ));
        let got: Vec<_> = diff.lines.iter().map(|l| (l.kind, l.old, l.new, l.text.as_str())).collect();
        use DiffLineKind::*;
        assert_eq!(
            got,
            [
                (Hunk, None, None, "@@ -3,3 +3,3 @@ fn main() {"),
                (Context, Some(3), Some(3), "un"),
                (Removed, Some(4), None, "deux"),
                (Added, None, Some(4), "2"),
                (Context, Some(5), Some(5), "    trois"),
                (Context, Some(6), Some(6), ""),
                (Note, None, None, "No newline at end of file"),
            ]
        );
    }

    #[test]
    fn parses_status_and_numstat() {
        let files = parse_status(" M a.rs\0R  new.rs\0old.rs\0?? n.txt\0UU c.rs\0A  b.rs\0 D d.rs\0");
        let got: Vec<_> = files.iter().map(|f| (f.path.as_str(), f.old_path.as_deref(), f.change, f.staged)).collect();
        use FileChange::*;
        assert_eq!(
            got,
            [
                ("a.rs", None, Modified, false),
                ("new.rs", Some("old.rs"), Renamed, true),
                ("n.txt", None, Untracked, false),
                ("c.rs", None, Conflicted, true),
                ("b.rs", None, Added, true),
                ("d.rs", None, Deleted, false),
            ]
        );
        assert_eq!(
            parse_numstat("1\t2\ta.rs\0-\t-\timg.png\x003\t0\t\0old.rs\0new.rs\0"),
            [("a.rs".into(), Some(1), Some(2)), ("img.png".into(), None, None), ("new.rs".into(), Some(3), Some(0))]
        );
    }

    #[test]
    fn lists_and_diffs_a_real_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        run(root, &["init", "-q"]);
        std::fs::write(root.join("a.txt"), "un\ndeux\ntrois\n").unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/b.txt"), "b\n").unwrap();
        run(root, &["add", "."]);
        run(root, &["commit", "-q", "-m", "x"]);

        std::fs::write(root.join("a.txt"), "un\n2\ntrois\n").unwrap();
        std::fs::write(root.join("sub/b.txt"), "b\nindexé\n").unwrap();
        run(root, &["add", "sub/b.txt"]);
        std::fs::write(root.join("nouveau fichier.txt"), "x\ny\n").unwrap();

        // Depuis un sous-dossier : les chemins restent relatifs à la racine.
        let sub = root.join("sub");
        let files = changed_files(&sub).unwrap();
        let got: Vec<_> = files.iter().map(|f| (f.path.as_str(), f.change, f.added, f.removed)).collect();
        assert_eq!(
            got,
            [
                ("a.txt", FileChange::Modified, Some(1), Some(1)),
                ("nouveau fichier.txt", FileChange::Untracked, None, None),
                ("sub/b.txt", FileChange::Modified, Some(1), Some(0)),
            ]
        );

        let a = file_diff(&sub, &files[0]).unwrap();
        let changed: Vec<_> = a.lines.iter().filter(|l| l.kind != DiffLineKind::Context).map(|l| l.text.as_str()).collect();
        assert_eq!(changed, ["@@ -1,3 +1,3 @@", "deux", "2"]);
        // Les modifications indexées comptent aussi.
        let b = file_diff(&sub, &files[2]).unwrap();
        assert!(b.lines.iter().any(|l| l.kind == DiffLineKind::Added && l.text == "indexé"));
        let new = file_diff(&sub, &files[1]).unwrap();
        assert_eq!(new.lines.iter().map(|l| (l.kind, l.new)).collect::<Vec<_>>(), [
            (DiffLineKind::Added, Some(1)),
            (DiffLineKind::Added, Some(2))
        ]);
    }
}
