//! Côté hook : lancé par Claude Code, doit être rapide et ne jamais bloquer.
//! Toute erreur est ignorée en silence (code de sortie 0, rien sur stdout).

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use crate::protocol::{ENV_SOCKET, ENV_TAB_ID, ENV_USER_STATUSLINE, Message, MessageKind};

const SOCKET_TIMEOUT: Duration = Duration::from_millis(300);
const USER_STATUSLINE_TIMEOUT: Duration = Duration::from_secs(3);

/// `grove hook` : transmet l'événement à Grove.
///
/// N'écrit rien sur stdout : pour `SessionStart` et `UserPromptSubmit`,
/// Claude Code ajouterait cette sortie au contexte de la conversation.
pub fn run_hook() -> ExitCode {
    let mut input = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut input);
    send(MessageKind::Hook, &input);
    ExitCode::SUCCESS
}

/// `grove statusline` : transmet les métriques à Grove, puis affiche la
/// statusline de l'utilisateur s'il en a une, sinon une ligne minimale.
pub fn run_statusline() -> ExitCode {
    let mut input = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut input);
    send(MessageKind::Statusline, &input);

    let line = std::env::var(ENV_USER_STATUSLINE)
        .ok()
        .filter(|c| !c.trim().is_empty())
        .and_then(|command| chain(&command, &input))
        .unwrap_or_else(|| default_line(&input));
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(line.as_bytes());
    let _ = stdout.flush();
    ExitCode::SUCCESS
}

fn send(kind: MessageKind, input: &[u8]) {
    let (Ok(socket), Ok(tab)) = (std::env::var(ENV_SOCKET), std::env::var(ENV_TAB_ID)) else {
        return;
    };
    let Ok(payload) = serde_json::from_slice(input) else {
        return;
    };
    let Ok(mut line) = serde_json::to_vec(&Message { tab, kind, payload }) else {
        return;
    };
    line.push(b'\n');
    let Ok(mut stream) = UnixStream::connect(socket) else {
        return;
    };
    let _ = stream.set_write_timeout(Some(SOCKET_TIMEOUT));
    let _ = stream.write_all(&line);
}

/// Lance la statusline de l'utilisateur avec la même entrée et renvoie sa sortie.
fn chain(command: &str, input: &[u8]) -> Option<String> {
    let mut child = Command::new("sh")
        .args(["-c", command])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    let input = input.to_vec();
    // Écrit dans un thread : une sortie volumineuse ne doit pas bloquer l'écriture.
    std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        out
    });

    let deadline = Instant::now() + USER_STATUSLINE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    reader.join().ok()
}

fn default_line(input: &[u8]) -> String {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(input) else {
        return String::new();
    };
    let metrics = crate::session::SessionMetrics::from_statusline(&value);
    let mut parts = Vec::new();
    if let Some(model) = metrics.model {
        parts.push(model);
    }
    if let Some(cost) = metrics.cost_usd {
        parts.push(format!("${cost:.2}"));
    }
    if let Some(context) = metrics.context_percent {
        parts.push(format!("contexte {context:.0} %"));
    }
    parts.join(" · ")
}
