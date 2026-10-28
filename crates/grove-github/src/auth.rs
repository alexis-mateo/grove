//! Jeton d'accès à l'API : celui saisi dans les Paramètres, sinon celui de la
//! CLI `gh` (`gh auth token`), sans rien lui demander d'autre.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenSource {
    /// Saisi dans les Paramètres, gardé dans un fichier lisible par l'utilisateur seul.
    Manual,
    /// Celui de la CLI GitHub (`gh auth login`).
    GhCli,
}

#[derive(Clone, PartialEq, Eq)]
pub struct Token {
    pub value: String,
    pub source: TokenSource,
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Token({:?}, {})", self.source, mask(&self.value))
    }
}

/// `ghp_abcd…wxyz` → `ghp_…wxyz` : de quoi reconnaître un jeton sans l'afficher.
pub fn mask(token: &str) -> String {
    let prefix = token.find('_').map_or("", |i| &token[..=i]);
    let tail: String = token.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("{prefix}…{tail}")
}

/// Le jeton saisi prime : l'utilisateur l'a choisi. Sinon, celui de `gh`,
/// lancé directement puis par le shell interactif (le `PATH` d'une appli
/// lancée depuis le bureau n'a pas toujours celui de Homebrew).
pub fn resolve_token(manual_file: &Path, shell: &str) -> Option<Token> {
    if let Some(value) = read_manual_token(manual_file) {
        return Some(Token { value, source: TokenSource::Manual });
    }
    let mut direct = Command::new("gh");
    direct.args(["auth", "token", "--hostname", "github.com"]);
    let value = run_gh(&mut direct).or_else(|| {
        let mut via_shell = Command::new(shell);
        via_shell.args(["-i", "-c", "gh auth token --hostname github.com"]);
        run_gh(&mut via_shell)
    })?;
    Some(Token { value, source: TokenSource::GhCli })
}

fn run_gh(command: &mut Command) -> Option<String> {
    let output = command.stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    if !output.status.success() {
        return None;
    }
    // Le shell interactif peut écrire avant : le jeton est la dernière ligne.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let token = stdout.lines().map(str::trim).rfind(|l| !l.is_empty())?;
    looks_like_token(token).then(|| token.to_owned())
}

fn looks_like_token(text: &str) -> bool {
    text.len() >= 20 && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub fn read_manual_token(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(text.trim().to_owned()).filter(|t| !t.is_empty())
}

/// Écrit le jeton, lisible par l'utilisateur seul.
pub fn save_manual_token(path: &Path, token: &str) -> Result<()> {
    let token = token.trim();
    if !looks_like_token(token) {
        bail!("ce n'est pas un jeton GitHub (ghp_…, github_pat_…)");
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("création de {}", dir.display()))?;
    }
    let tmp = path.with_extension("tmp");
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .with_context(|| format!("écriture de {}", tmp.display()))?;
        writeln!(file, "{token}")?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("écriture de {}", path.display()))
}

pub fn remove_manual_token(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e).with_context(|| format!("suppression de {}", path.display())),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn manual_token_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grove/github-token");
        assert_eq!(read_manual_token(&path), None);
        assert!(save_manual_token(&path, "pas un jeton").is_err());

        save_manual_token(&path, "  ghp_0123456789abcdefghijWXYZ\n").unwrap();
        assert_eq!(read_manual_token(&path).as_deref(), Some("ghp_0123456789abcdefghijWXYZ"));
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let token = resolve_token(&path, "/bin/false").unwrap();
        assert_eq!(token.source, TokenSource::Manual);

        remove_manual_token(&path).unwrap();
        remove_manual_token(&path).unwrap();
        assert_eq!(read_manual_token(&path), None);
    }

    #[test]
    fn masks_tokens() {
        assert_eq!(mask("ghp_0123456789abcdefghijWXYZ"), "ghp_…WXYZ");
        assert_eq!(mask("github_pat_11AAAA_zzzz9876"), "github_…9876");
        assert_eq!(format!("{:?}", Token { value: "ghp_secret1234".into(), source: TokenSource::GhCli }), "Token(GhCli, ghp_…1234)");
    }
}
