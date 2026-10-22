//! Processus `claude -p` en stream-json, piloté par la vue native (specs §10.1).
//!
//! Lancé, comme en mode terminal, par le shell interactif de l'utilisateur pour
//! garder ses alias et fonctions. Le shell et ses enfants forment un groupe de
//! processus : l'interruption et l'arrêt s'adressent à tout le groupe.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};

/// Fin de stderr conservée pour expliquer un arrêt (commande introuvable…).
const STDERR_TAIL: usize = 4096;

#[derive(Clone, Debug, Default)]
pub struct NativeOptions {
    /// Shell interactif qui lance la commande (`$SHELL -i -c`).
    pub shell: String,
    /// Commande `claude` de l'utilisateur (`[claude] command`).
    pub command: String,
    pub cwd: Option<PathBuf>,
    pub env: HashMap<String, String>,
    /// Réglages de Grove (`--settings`) : hooks, donc badges d'état.
    pub settings: Option<PathBuf>,
    pub resume: Option<String>,
    /// `--permission-mode` : `default`, `acceptEdits`, `plan`…
    pub permission_mode: Option<String>,
    /// Serveur MCP et outil qui répondent aux demandes de permission.
    pub permission_prompt: Option<PermissionPrompt>,
}

#[derive(Clone, Debug)]
pub struct PermissionPrompt {
    /// Configuration MCP (JSON) passée à `--mcp-config`.
    pub mcp_config: String,
    /// Nom complet de l'outil (`mcp__<serveur>__<outil>`).
    pub tool: String,
}

impl NativeOptions {
    /// Ligne de commande passée au shell.
    pub fn command_line(&self) -> String {
        let mut line = format!(
            "{} -p --input-format stream-json --output-format stream-json --include-partial-messages --verbose",
            self.command
        );
        let mut arg = |flag: &str, value: &str| line.push_str(&format!(" {flag} {}", shell_quote(value)));
        if let Some(settings) = &self.settings {
            arg("--settings", &settings.to_string_lossy());
        }
        if let Some(id) = &self.resume {
            arg("--resume", id);
        }
        if let Some(mode) = &self.permission_mode {
            arg("--permission-mode", mode);
        }
        if let Some(prompt) = &self.permission_prompt {
            arg("--mcp-config", &prompt.mcp_config);
            arg("--permission-prompt-tool", &prompt.tool);
        }
        line
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativeEvent {
    /// Une ligne de stdout (JSON en principe, à lire avec `Conversation::apply_line`).
    Line(String),
    /// Le processus s'est terminé ; `stderr` : ses dernières lignes d'erreur.
    Exited { code: Option<i32>, stderr: String },
}

pub struct NativeProcess {
    stdin: Mutex<Option<ChildStdin>>,
    /// Groupe de processus (pid du shell).
    pgid: i32,
}

impl NativeProcess {
    /// `on_event` est appelé depuis un thread de lecture.
    pub fn spawn(options: &NativeOptions, on_event: impl Fn(NativeEvent) + Send + 'static) -> Result<Self> {
        let mut command = Command::new(&options.shell);
        command
            .args(["-i", "-c", &options.command_line()])
            .envs(&options.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        if let Some(cwd) = &options.cwd {
            command.current_dir(cwd);
        }
        let mut child = command.spawn().with_context(|| format!("lancement de {}", options.shell))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().context("stdout du processus")?;
        let stderr = child.stderr.take().context("stderr du processus")?;
        let pgid = child.id() as i32;

        let tail = Arc::new(Mutex::new(String::new()));
        let stderr_thread = std::thread::Builder::new().name("grove-claude-stderr".into()).spawn({
            let tail = tail.clone();
            move || {
                let mut buffer = [0u8; 1024];
                let mut stderr = stderr;
                while let Ok(n @ 1..) = stderr.read(&mut buffer) {
                    let mut tail = tail.lock().unwrap();
                    tail.push_str(&String::from_utf8_lossy(&buffer[..n]));
                    if tail.len() > STDERR_TAIL {
                        let cut = tail.len() - STDERR_TAIL;
                        let cut = (cut..tail.len()).find(|&i| tail.is_char_boundary(i)).unwrap_or(tail.len());
                        tail.drain(..cut);
                    }
                }
            }
        })?;
        std::thread::Builder::new().name("grove-claude-stdout".into()).spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => on_event(NativeEvent::Line(line)),
                    Err(err) => {
                        log::debug!("lecture de stdout : {err}");
                        break;
                    }
                }
            }
            let code = child.wait().ok().and_then(|status| status.code());
            let _ = stderr_thread.join();
            let stderr = tail.lock().unwrap().trim().to_owned();
            on_event(NativeEvent::Exited { code, stderr });
        })?;

        Ok(Self { stdin: Mutex::new(stdin), pgid })
    }

    /// Écrit une ligne sur stdin (message utilisateur en stream-json).
    pub fn send(&self, line: &str) -> Result<()> {
        let mut stdin = self.stdin.lock().unwrap();
        let stdin = stdin.as_mut().context("processus arrêté")?;
        stdin.write_all(line.as_bytes())?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
        Ok(())
    }

    /// Interrompt le tour en cours. En mode `-p`, Claude Code s'arrête : la
    /// session se reprend avec `--resume` au message suivant.
    pub fn interrupt(&self) {
        self.signal(libc::SIGINT);
    }

    fn signal(&self, signal: i32) {
        // SAFETY : `kill` n'a pas d'effet sur la mémoire ; pgid négatif = tout le groupe.
        unsafe {
            libc::kill(-self.pgid, signal);
        }
    }
}

impl Drop for NativeProcess {
    fn drop(&mut self) {
        self.stdin.lock().unwrap().take();
        self.signal(libc::SIGTERM);
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    #[test]
    fn command_line_quotes_arguments() {
        let options = NativeOptions {
            command: "claude".into(),
            settings: Some("/home/me/grove settings.json".into()),
            resume: Some("abc".into()),
            permission_mode: Some("acceptEdits".into()),
            ..Default::default()
        };
        assert_eq!(
            options.command_line(),
            "claude -p --input-format stream-json --output-format stream-json --include-partial-messages --verbose \
             --settings '/home/me/grove settings.json' --resume 'abc' --permission-mode 'acceptEdits'"
        );
    }

    /// Un faux `claude` (`cat`, qui renvoie ses entrées) : aller-retour stdin → stdout, puis arrêt.
    #[test]
    fn echoes_lines_and_reports_exit() {
        let options = NativeOptions { shell: "/bin/sh".into(), command: "cat; echo oups >&2; exit 4 #".into(), ..Default::default() };
        let (tx, rx) = mpsc::channel();
        let process = NativeProcess::spawn(&options, move |event| {
            let _ = tx.send(event);
        })
        .unwrap();
        process.send(r#"{"type":"user"}"#).unwrap();
        let timeout = Duration::from_secs(5);
        assert_eq!(rx.recv_timeout(timeout).unwrap(), NativeEvent::Line(r#"{"type":"user"}"#.into()));
        // Fermer stdin termine `cat`.
        process.stdin.lock().unwrap().take();
        // Le shell interactif sans terminal se plaint aussi du contrôle des tâches.
        let NativeEvent::Exited { code, stderr } = rx.recv_timeout(timeout).unwrap() else { panic!("fin attendue") };
        assert_eq!(code, Some(4));
        assert!(stderr.ends_with("oups"), "{stderr}");
    }
}
