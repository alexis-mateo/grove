//! Profils de terminal (specs §11.1) : ce qu'un onglet lance. Shell local,
//! SSH (CLI `ssh` du système) ou commande libre.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    #[default]
    Shell,
    Ssh,
    Command,
}

/// Reconnexion d'un onglet SSH restauré au démarrage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reconnect {
    /// Bouton « Se connecter » : pas de rafale de connexions ni de passphrases.
    #[default]
    Manual,
    Auto,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub name: String,
    /// Absent : déduit des champs (`host` → SSH, `command` → commande, sinon shell).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<ProfileKind>,
    /// Commande libre, lancée par le shell de l'utilisateur (alias et fonctions compris).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Dossier de départ ; relatif à l'espace de travail, `~` accepté. Par défaut : l'espace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,

    /// Alias de `~/.ssh/config` ou nom d'hôte.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Clé privée (`-i`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    /// Rebond (`-J`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jump: Option<String>,
    /// Options `-o`, comme `ServerAliveInterval=30`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// Commande lancée sur l'hôte (`-t` ajouté).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_command: Option<String>,
    #[serde(skip_serializing_if = "is_default")]
    pub reconnect: Reconnect,
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// Programme et arguments à lancer dans le PTY.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
}

impl Profile {
    /// Profil SSH tiré d'une entrée `Host` de `~/.ssh/config`.
    pub fn ssh(host: &str) -> Self {
        Self { name: host.to_owned(), kind: Some(ProfileKind::Ssh), host: Some(host.to_owned()), ..Self::default() }
    }

    pub fn kind(&self) -> ProfileKind {
        self.kind.unwrap_or(if self.host.is_some() {
            ProfileKind::Ssh
        } else if self.command.is_some() {
            ProfileKind::Command
        } else {
            ProfileKind::Shell
        })
    }

    /// Ce que lance un onglet de ce profil ; `shell` : le shell de l'utilisateur.
    pub fn launch(&self, shell: &str) -> Launch {
        let shell_launch = |args: Vec<String>| Launch { program: shell.to_owned(), args };
        match self.kind() {
            ProfileKind::Shell => shell_launch(Vec::new()),
            ProfileKind::Command => match self.command.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
                Some(command) => shell_launch(vec!["-i".into(), "-c".into(), command.to_owned()]),
                None => shell_launch(Vec::new()),
            },
            ProfileKind::Ssh => Launch { program: "ssh".into(), args: self.ssh_args() },
        }
    }

    fn ssh_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        let mut push = |flag: &str, value: &str| {
            args.push(flag.to_owned());
            args.push(value.to_owned());
        };
        if let Some(port) = self.port {
            push("-p", &port.to_string());
        }
        if let Some(identity) = non_empty(&self.identity) {
            push("-i", &expand_home(identity).to_string_lossy());
        }
        if let Some(jump) = non_empty(&self.jump) {
            push("-J", jump);
        }
        for option in self.options.iter().map(|o| o.trim()).filter(|o| !o.is_empty()) {
            push("-o", option);
        }
        if let Some(user) = non_empty(&self.user) {
            push("-l", user);
        }
        let remote = non_empty(&self.remote_command);
        if remote.is_some() {
            args.push("-t".into());
        }
        // `--` : un hôte commençant par `-` ne peut pas passer pour une option.
        args.push("--".into());
        args.push(non_empty(&self.host).unwrap_or(&self.name).to_owned());
        args.extend(remote.map(str::to_owned));
        args
    }

    /// Dossier de départ d'un onglet ouvert dans `workspace`.
    pub fn start_dir(&self, workspace: &Path) -> PathBuf {
        match non_empty(&self.cwd) {
            Some(cwd) => workspace.join(expand_home(cwd)),
            None => workspace.to_owned(),
        }
    }
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|v| !v.is_empty())
}

pub fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ if path == "~" => std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| path.into()),
        _ => PathBuf::from(path),
    }
}

/// D'où vient un profil, du plus prioritaire au moins prioritaire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileSource {
    /// `.grove.toml` du projet : versionné, partagé avec l'équipe.
    Project(PathBuf),
    /// `config.toml` de l'utilisateur.
    Global,
    /// Entrée `Host` de `~/.ssh/config`, sans recopie.
    SshConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedProfile {
    pub profile: Profile,
    pub source: ProfileSource,
}

/// Profils proposés dans un projet : ceux du projet, puis les globaux, puis les
/// hôtes de `~/.ssh/config`. Un nom déjà pris masque les suivants ; un hôte SSH
/// déjà visé par un profil Grove (`host = "…"`) n'est pas repris.
pub fn resolve(project: &[Profile], project_file: &Path, global: &[Profile], ssh_hosts: &[String]) -> Vec<ResolvedProfile> {
    let mut resolved: Vec<ResolvedProfile> = Vec::new();
    let sources = project
        .iter()
        .map(|p| (p.clone(), ProfileSource::Project(project_file.to_owned())))
        .chain(global.iter().map(|p| (p.clone(), ProfileSource::Global)));
    for (profile, source) in sources {
        if !profile.name.trim().is_empty() && !resolved.iter().any(|r| r.profile.name == profile.name) {
            resolved.push(ResolvedProfile { profile, source });
        }
    }
    for host in ssh_hosts {
        let taken = resolved.iter().any(|r| r.profile.name == *host || r.profile.host.as_deref() == Some(host));
        if !taken {
            resolved.push(ResolvedProfile { profile: Profile::ssh(host), source: ProfileSource::SshConfig });
        }
    }
    resolved
}

/// Alias des entrées `Host` de `~/.ssh/config` (et des fichiers `Include`),
/// sans les motifs (`*`, `?`, `!`).
pub fn ssh_hosts(config: &Path) -> Vec<String> {
    let mut hosts = Vec::new();
    let dir = config.parent().unwrap_or(Path::new("."));
    read_ssh_config(config, dir, &mut hosts, 0);
    hosts
}

fn read_ssh_config(path: &Path, dir: &Path, hosts: &mut Vec<String>, depth: usize) {
    let Ok(text) = std::fs::read_to_string(path) else { return };
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        // `Mot-clé valeurs` ou `Mot-clé=valeurs`.
        let (keyword, rest) = match line.find(|c: char| c.is_whitespace() || c == '=') {
            Some(i) => (&line[..i], line[i + 1..].trim_start_matches(|c: char| c.is_whitespace() || c == '=')),
            None => continue,
        };
        if keyword.eq_ignore_ascii_case("host") {
            for alias in rest.split_whitespace().map(|a| a.trim_matches('"')) {
                if !alias.contains(['*', '?', '!']) && !hosts.iter().any(|h| h == alias) {
                    hosts.push(alias.to_owned());
                }
            }
        } else if keyword.eq_ignore_ascii_case("include") && depth < 8 {
            for pattern in rest.split_whitespace() {
                let pattern = expand_home(pattern);
                let pattern = if pattern.is_absolute() { pattern } else { dir.join(pattern) };
                for file in glob(&pattern) {
                    read_ssh_config(&file, dir, hosts, depth + 1);
                }
            }
        }
    }
}

/// Fichiers désignés par `pattern`, dont seul le dernier composant peut contenir `*`.
fn glob(pattern: &Path) -> Vec<PathBuf> {
    let name = pattern.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if !name.contains('*') {
        return vec![pattern.to_owned()];
    }
    let Some(parent) = pattern.parent() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(parent) else { return Vec::new() };
    let mut files: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|e| wildcard_match(&name, &e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .collect();
    files.sort();
    files
}

fn wildcard_match(pattern: &str, name: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == name,
        Some((prefix, rest)) => {
            let Some(name) = name.strip_prefix(prefix) else { return false };
            (0..=name.len()).filter(|&i| name.is_char_boundary(i)).any(|i| wildcard_match(rest, &name[i..]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_spec_examples_and_infers_kind() {
        #[derive(Deserialize)]
        struct Wrapper {
            profiles: Vec<Profile>,
        }
        let Wrapper { profiles } = toml::from_str(
            r#"
            [[profiles]]
            name = "staging"
            kind = "ssh"
            host = "staging-web-1"
            user = "deploy"
            remote_command = "cd /var/www/app && exec $SHELL -l"

            [[profiles]]
            name = "db"
            command = "docker compose exec db psql -U app"
            "#,
        )
        .unwrap();
        assert_eq!(profiles[0].kind(), ProfileKind::Ssh);
        assert_eq!(profiles[0].launch("/bin/zsh"), Launch {
            program: "ssh".into(),
            args: ["-l", "deploy", "-t", "--", "staging-web-1", "cd /var/www/app && exec $SHELL -l"]
                .map(String::from)
                .to_vec(),
        });
        assert_eq!(profiles[1].kind(), ProfileKind::Command);
        assert_eq!(profiles[1].launch("/bin/zsh").args, ["-i", "-c", "docker compose exec db psql -U app"]);
        assert_eq!(Profile::default().launch("/bin/zsh"), Launch { program: "/bin/zsh".into(), args: vec![] });
    }

    #[test]
    fn ssh_options() {
        let profile = Profile {
            port: Some(2222),
            jump: Some("bastion".into()),
            options: vec!["ServerAliveInterval=30".into(), " ".into()],
            ..Profile::ssh("web")
        };
        assert_eq!(profile.launch("sh").args, ["-p", "2222", "-J", "bastion", "-o", "ServerAliveInterval=30", "--", "web"]);
    }

    #[test]
    fn serializes_only_what_is_set() {
        let text = toml::to_string(&Profile::ssh("web")).unwrap();
        assert_eq!(text, "name = \"web\"\nkind = \"ssh\"\nhost = \"web\"\n");
    }

    #[test]
    fn resolution_order_and_overrides() {
        let project = [Profile { name: "db".into(), command: Some("psql".into()), ..Default::default() }];
        let global = [
            Profile { name: "db".into(), command: Some("mysql".into()), ..Default::default() },
            Profile { name: "prod".into(), user: Some("root".into()), ..Profile::ssh("web") },
        ];
        let hosts = ["web".to_owned(), "bastion".to_owned()];
        let resolved = resolve(&project, Path::new("/p/.grove.toml"), &global, &hosts);
        let names: Vec<_> = resolved.iter().map(|r| r.profile.name.as_str()).collect();
        assert_eq!(names, ["db", "prod", "bastion"], "`web` est surchargé par « prod »");
        assert_eq!(resolved[0].profile.command.as_deref(), Some("psql"));
        assert_eq!(resolved[0].source, ProfileSource::Project("/p/.grove.toml".into()));
        assert_eq!(resolved[2].source, ProfileSource::SshConfig);
    }

    #[test]
    fn reads_ssh_config_hosts_and_includes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("config.d")).unwrap();
        std::fs::write(
            dir.path().join("config"),
            "# commentaire\nInclude config.d/*.conf\nHost web web-2 *.internal\n  User deploy\nhost=\"db\"\nHost * !bastion\nMatch host x\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("config.d/a.conf"), "Host bastion\n").unwrap();
        std::fs::write(dir.path().join("config.d/ignored"), "Host nope\n").unwrap();
        assert_eq!(ssh_hosts(&dir.path().join("config")), ["bastion", "web", "web-2", "db"]);
        assert!(ssh_hosts(&dir.path().join("absent")).is_empty());
    }

    #[test]
    fn start_dir() {
        let workspace = Path::new("/code/app");
        assert_eq!(Profile::default().start_dir(workspace), workspace);
        let profile = Profile { cwd: Some("web".into()), ..Default::default() };
        assert_eq!(profile.start_dir(workspace), Path::new("/code/app/web"));
        let profile = Profile { cwd: Some("/srv".into()), ..Default::default() };
        assert_eq!(profile.start_dir(workspace), Path::new("/srv"));
    }
}
