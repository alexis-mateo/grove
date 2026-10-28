//! Intégration GitHub (specs §12.1) : jeton, pull requests ouvertes qui me
//! concernent, dépôt d'un remote.

mod auth;
mod pulls;

pub use auth::{Token, TokenSource, mask, read_manual_token, remove_manual_token, resolve_token, save_manual_token};
pub use pulls::{AuthorState, CheckState, FetchError, Inbox, PullRequest, ReviewDecision, ReviewState, Thread, fetch_inbox};

/// `owner/repo` d'une URL de remote GitHub : `git@github.com:o/r.git`,
/// `https://github.com/o/r`, `ssh://git@github.com/o/r.git`. Un alias SSH
/// (`git@github-perso:o/r`) est accepté s'il contient « github ».
pub fn repo_from_remote(url: &str) -> Option<String> {
    let url = url.trim();
    let rest = match url.split_once("://") {
        Some((_, rest)) => rest.split_once('/').filter(|(host, _)| host.contains("github")).map(|(_, path)| path)?,
        None => url.split_once(':').filter(|(host, _)| host.contains("github")).map(|(_, path)| path)?,
    };
    let path = rest.trim_end_matches('/').trim_end_matches(".git");
    let mut parts = path.split('/');
    let (owner, name) = (parts.next()?, parts.next()?);
    if owner.is_empty() || name.is_empty() || parts.next().is_some() {
        return None;
    }
    Some(format!("{owner}/{name}"))
}

#[cfg(test)]
mod tests {
    use super::repo_from_remote;

    #[test]
    fn parses_github_remotes() {
        for url in [
            "git@github.com:rgsystemes/kb.git",
            "https://github.com/rgsystemes/kb",
            "https://github.com/rgsystemes/kb.git/",
            "ssh://git@github.com/rgsystemes/kb.git",
            "git@github-pro:rgsystemes/kb.git",
        ] {
            assert_eq!(repo_from_remote(url).as_deref(), Some("rgsystemes/kb"), "{url}");
        }
        assert_eq!(repo_from_remote("git@gitlab.com:a/b.git"), None);
        assert_eq!(repo_from_remote("https://github.com/a"), None);
        assert_eq!(repo_from_remote("/srv/git/app.git"), None);
    }
}
